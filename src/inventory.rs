//! Structural inventory walker for JPEG XL files (container level).
//!
//! Walks the ISO/IEC 18181-2 box structure without decoding pixels and records
//! what the zencodec decode path (`JxlDecodeJob`) does with each box. The
//! codestream itself is one opaque `ImageData` part: frame headers, extra
//! channel names and header extension bits are not walked yet (see
//! `docs/INVENTORY.md`).
//!
//! The box framing rules follow `zenjxl-decoder`'s `BoxParser`
//! (`src/api/inner/box_parser.rs`), because the inventory has to describe what
//! that parser accepts, not what the specification allows.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ops::Range;

use zencodec::ImageFormat;
use zencodec::inventory::{
    Disposition, Inventory, InventoryError, MetadataKind, Part, PartKind, PartTag,
};

/// The 12-byte container signature box.
const SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// Metadata box size above which the decoder skips the payload instead of
/// buffering it (`box_parser.rs`: Exif, xml, jxli, brob).
const METADATA_BOX_CAP: u64 = 16 * 1024 * 1024;
/// Same for the gain-map bundle (`jhgm`).
const GAIN_MAP_BOX_CAP: u64 = 256 * 1024 * 1024;
/// Largest single-`jxlc` codestream after which the decoder still reads the
/// boxes that follow it. Measured against `zenjxl-decoder` 0.4.0 (see the
/// `trailing_box_model_matches_decoder` test): with codestream payloads up to
/// and including 4096 bytes a trailing `Exif`/`xml ` box surfaces, from 4097
/// bytes on it does not.
const TRAILING_POLL_LIMIT: u64 = 4096;
/// Longest label copied from the file.
const LABEL_MAX: usize = 64;

/// What the caller's decode job would surface.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WalkConfig {
    /// `DecodePolicy::resolve_exif(true)`.
    pub exif: bool,
    /// `DecodePolicy::resolve_xmp(true)`.
    pub xmp: bool,
    /// The job surfaces the gain map (`extract_gain_map` or a render mode
    /// other than `BaseOnly`).
    pub gain_map: bool,
}

/// Why a walk produced no inventory.
#[derive(Debug)]
pub(crate) enum WalkError {
    /// The part cap was reached.
    Parts(InventoryError),
    /// The caller's stop token fired.
    Stopped(enough::StopReason),
}

impl From<InventoryError> for WalkError {
    fn from(e: InventoryError) -> Self {
        Self::Parts(e)
    }
}

/// One box as framed on disk, before any interpretation.
struct RawBox {
    start: u64,
    end: u64,
    header: u64,
    ty: [u8; 4],
    /// The declared size ran past the input; `end` is the input length.
    truncated: Option<u64>,
    /// Size field 0: the box runs to the end of the input.
    to_eof: bool,
}

impl RawBox {
    fn payload(&self) -> Range<u64> {
        self.start + self.header..self.end
    }
}

/// A box with its disposition decided, waiting for cross-box fix-ups.
struct Planned {
    raw: RawBox,
    disposition: Disposition,
    label: Option<String>,
    detail: Option<String>,
}

/// Which codestream box the walk is in, mirroring `CodestreamBoxType`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cs {
    None,
    Jxlc,
    Jxlp(u32),
    Last,
}

fn be32(d: &[u8], at: u64) -> Option<u32> {
    let at = usize::try_from(at).ok()?;
    Some(u32::from_be_bytes(
        d.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn be64(d: &[u8], at: u64) -> Option<u64> {
    let at = usize::try_from(at).ok()?;
    Some(u64::from_be_bytes(
        d.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// Printable-ASCII copy of up to [`LABEL_MAX`] bytes.
fn label_of(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(LABEL_MAX)
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

/// Frame the box starting at `at`. `Err` carries why it cannot be framed.
fn frame_box(data: &[u8], at: u64) -> Result<RawBox, String> {
    let len = data.len() as u64;
    let Some(size32) = be32(data, at) else {
        return Err(format!("{} bytes left, a box header needs 8", len - at));
    };
    let Some(ty) = data
        .get(at as usize + 4..at as usize + 8)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
    else {
        return Err(format!("{} bytes left, a box header needs 8", len - at));
    };
    let (header, total, to_eof) = match size32 {
        0 => (8, len - at, true),
        1 => match be64(data, at + 8) {
            Some(total) => (16, total, false),
            None => return Err("truncated extended box size".to_string()),
        },
        n => (8, u64::from(n), false),
    };
    if total < header {
        return Err(format!(
            "box size {total} is smaller than its {header}-byte header"
        ));
    }
    let declared_end = at.checked_add(total);
    let (end, truncated) = match declared_end {
        Some(e) if e <= len => (e, None),
        _ => (len, Some(total)),
    };
    Ok(RawBox {
        start: at,
        end,
        header,
        ty,
        truncated,
        to_eof,
    })
}

/// The first-level `jumd` label of a `jumb` superbox, when it carries one.
fn jumb_label(payload: &[u8]) -> Option<String> {
    if payload.get(4..8)? != b"jumd" {
        return None;
    }
    // jumd: 16-byte type UUID, 1 toggles byte, then (bit 1) a NUL-terminated label.
    let toggles = *payload.get(8 + 16)?;
    if toggles & 0x02 == 0 {
        return None;
    }
    let rest = payload.get(8 + 17..)?;
    let n = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Some(label_of(&rest[..n]))
}

/// Locate the end of the codestream by driving `zenjxl-decoder` through a
/// header-only scan (`scan_frames_only`): frame headers and TOCs are parsed,
/// section data is skipped, no pixels are produced. On success the decoder's
/// `file_length()` is the offset just past the last codestream byte.
fn codestream_end(
    data: &[u8],
    stop: Option<&dyn enough::Stop>,
) -> Result<Result<u64, String>, WalkError> {
    use jxl::api::{JxlDecoder, JxlDecoderOptions, ProcessingResult, states};

    let mut options = JxlDecoderOptions::default().with_scan_frames_only(true);
    options.limits.max_icc_size = Some(16 << 20);
    options.limits.max_memory_bytes = Some(256 << 20);
    let mut input: &[u8] = data;
    let decoder = JxlDecoder::<states::Initialized>::new(options);
    let mut decoder = match decoder.process(&mut input) {
        Ok(ProcessingResult::Complete { result }) => result,
        Ok(ProcessingResult::NeedsMoreInput { .. }) => {
            return Ok(Err("truncated before the image header ends".to_string()));
        }
        Err(e) => return Ok(Err(format!("image header: {}", e.error()))),
    };
    let mut frames = 0u32;
    while decoder.has_more_frames() {
        if let Some(s) = stop {
            s.check().map_err(WalkError::Stopped)?;
        }
        let with_frame = match decoder.process(&mut input) {
            Ok(ProcessingResult::Complete { result }) => result,
            Ok(ProcessingResult::NeedsMoreInput { .. }) => {
                return Ok(Err(format!("truncated in frame {frames}'s header")));
            }
            Err(e) => return Ok(Err(format!("frame {frames} header: {}", e.error()))),
        };
        decoder = match with_frame.skip_frame(&mut input) {
            Ok(ProcessingResult::Complete { result }) => result,
            Ok(ProcessingResult::NeedsMoreInput { .. }) => {
                return Ok(Err(format!("truncated in frame {frames}'s sections")));
            }
            Err(e) => return Ok(Err(format!("frame {frames} sections: {}", e.error()))),
        };
        frames = frames.saturating_add(1);
    }
    Ok(decoder
        .file_length()
        .filter(|&n| n <= data.len() as u64)
        .ok_or_else(|| "the decoder reported no file length".to_string()))
}

/// Walk `data` and describe every byte.
pub(crate) fn walk(
    data: &[u8],
    cfg: WalkConfig,
    stop: Option<&dyn enough::Stop>,
) -> Result<Inventory, WalkError> {
    let len = data.len() as u64;
    let mut inv = Inventory::new(ImageFormat::Jxl, len);
    if len == 0 {
        return Ok(inv);
    }

    if data.starts_with(&[0xFF, 0x0A]) {
        inv.push(
            None,
            Part::new(
                PartKind::Header,
                PartTag::Marker(0x0A),
                0..2,
                Disposition::Structure,
            )
            .with_detail("bare codestream signature"),
        )?;
        if len > 2 {
            let (end, note) = match codestream_end(data, stop)? {
                Ok(end) if end > 2 => (end, None),
                Ok(_) => (
                    len,
                    Some("the decoder reported an empty codestream".to_string()),
                ),
                Err(why) => (len, Some(format!("end not located ({why})"))),
            };
            let mut part = Part::new(
                PartKind::Chunk,
                PartTag::None,
                2..end,
                Disposition::ImageData,
            )
            .with_label("codestream");
            if let Some(n) = note {
                part = part.with_detail(format!(
                    "{n}; this part runs to the end of the input and any trailing bytes are \
                     inside it"
                ));
            }
            inv.push(None, part)?;
            if end < len {
                inv.push(
                    None,
                    Part::new(
                        PartKind::Trailer,
                        PartTag::None,
                        end..len,
                        Disposition::Trailing,
                    )
                    .with_detail("after the last frame of the bare codestream"),
                )?;
            }
        }
        return Ok(inv);
    }

    if !data.starts_with(&SIGNATURE) {
        let truncated_sig = data == &[0xFF][..] || SIGNATURE.starts_with(data);
        let why = if truncated_sig {
            "truncated JPEG XL signature"
        } else {
            "no JPEG XL signature"
        };
        inv.push(
            None,
            Part::new(PartKind::Gap, PartTag::None, 0..len, Disposition::Malformed)
                .with_detail(why),
        )?;
        return Ok(inv);
    }

    inv.push(
        None,
        Part::new(
            PartKind::Header,
            PartTag::None,
            0..12,
            Disposition::Structure,
        )
        .with_label("JXL ")
        .with_detail("container signature box"),
    )?;

    // ── pass 1: frame and classify every box ────────────────────────────────
    let mut planned: Vec<Planned> = Vec::new();
    let mut tail: Option<(u64, String, bool)> = None; // (start, why, after codestream)
    let mut at = 12u64;
    let mut index = 0usize;
    let mut state = Cs::None;
    let mut trailing_read = false;
    let mut version = 0u32;
    let mut ooo: Vec<u32> = Vec::new();
    let mut dead: Option<u64> = None;

    while at < len {
        if index.is_multiple_of(1024)
            && let Some(s) = stop
        {
            s.check().map_err(WalkError::Stopped)?;
        }
        let after_cs = matches!(state, Cs::Jxlc | Cs::Last);
        let raw = match frame_box(data, at) {
            Ok(raw) => raw,
            Err(why) => {
                tail = Some((at, why, after_cs));
                break;
            }
        };
        let payload = raw.payload();
        let plen = payload.end - payload.start;
        let body = &data[payload.start as usize..payload.end as usize];
        let ty = raw.ty;
        let readable = !after_cs || trailing_read;

        let mut label = None;
        let mut detail = None;
        let mut reject: Option<String> = None;

        // The decoder demands `ftyp` as the second box, exactly once.
        let is_ftyp = &ty == b"ftyp";
        if (index == 0) != is_ftyp {
            reject = Some(if index == 0 {
                "the decoder requires `ftyp` as the first box after the signature".to_string()
            } else {
                "a second `ftyp` box".to_string()
            });
        }

        let mut disposition = Disposition::Unknown;
        if let Some(why) = &reject {
            disposition = Disposition::Malformed;
            detail = Some(format!("the decoder rejects the file here: {why}"));
        } else if let Some(offender) = dead {
            disposition = Disposition::Skipped;
            detail = Some(format!(
                "never read: the decoder rejects the file at offset {offender}"
            ));
        } else if raw.truncated.is_some() {
            disposition = Disposition::Malformed;
        } else {
            match &ty {
                b"ftyp" => {
                    let ok = !raw.to_eof
                        && plen >= 8
                        && &body[..4] == b"jxl "
                        && u32::from_be_bytes([body[4], body[5], body[6], body[7]]) <= 1;
                    if ok {
                        version = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                        disposition = Disposition::Structure;
                        label = Some("jxl ".to_string());
                        detail = Some(format!("file format version {version}"));
                    } else {
                        disposition = Disposition::Malformed;
                        reject = Some("`ftyp` must carry brand `jxl ` and version 0 or 1".into());
                        detail = Some(
                            "the decoder rejects the file here: `ftyp` must carry brand \
                             `jxl ` and version 0 or 1"
                                .to_string(),
                        );
                    }
                }
                b"jxlc" => {
                    if matches!(state, Cs::Jxlp(_) | Cs::Last) {
                        disposition = Disposition::Malformed;
                        reject = Some("`jxlc` after `jxlp`".into());
                        detail =
                            Some("the decoder rejects the file here: `jxlc` after `jxlp`".into());
                    } else {
                        disposition = Disposition::ImageData;
                        state = Cs::Jxlc;
                        trailing_read = plen <= TRAILING_POLL_LIMIT;
                    }
                }
                b"jxlp" => {
                    let idx_word = be32(data, payload.start);
                    match idx_word {
                        Some(word) if plen >= 4 => {
                            let last = word & 0x8000_0000 != 0;
                            let idx = word & 0x7fff_ffff;
                            let wanted = match state {
                                Cs::None => Some(0),
                                Cs::Jxlp(i) => Some(i + 1),
                                _ => None,
                            };
                            let bad = match wanted {
                                None => Some("`jxlp` after a complete codestream"),
                                Some(w) if idx < w => Some("`jxlp` index out of order"),
                                Some(w)
                                    if idx > w
                                        && (version < 1 || ooo.contains(&idx) || raw.to_eof) =>
                                {
                                    Some("`jxlp` index out of order")
                                }
                                _ => None,
                            };
                            if let Some(why) = bad {
                                disposition = Disposition::Malformed;
                                reject = Some(why.into());
                                detail = Some(format!("the decoder rejects the file here: {why}"));
                            } else {
                                disposition = Disposition::ImageData;
                                let mut d = format!("jxlp index {idx}");
                                if last {
                                    d.push_str(" (last)");
                                }
                                if Some(idx) != wanted {
                                    d.push_str(", out of order (ftyp version 1)");
                                    ooo.push(idx);
                                } else {
                                    state = if last { Cs::Last } else { Cs::Jxlp(idx) };
                                    // Buffered out-of-order parts splice in right after.
                                    while let Cs::Jxlp(i) = state {
                                        if ooo.contains(&(i + 1)) {
                                            state = Cs::Jxlp(i + 1);
                                        } else {
                                            break;
                                        }
                                    }
                                }
                                detail = Some(d);
                            }
                        }
                        _ => {
                            disposition = Disposition::Malformed;
                            reject = Some("`jxlp` box without its 4-byte index".into());
                            detail = Some(
                                "the decoder rejects the file here: `jxlp` box without its \
                                 4-byte index"
                                    .into(),
                            );
                        }
                    }
                }
                b"jbrd" => {
                    disposition = Disposition::Skipped;
                    detail = Some(
                        "JPEG reconstruction data; zenjxl-decoder is built without its `jpeg` \
                         feature, which skips this box"
                            .into(),
                    );
                }
                b"jhgm" => {
                    if plen > GAIN_MAP_BOX_CAP {
                        disposition = Disposition::Skipped;
                        detail = Some("larger than the decoder's 256 MiB gain-map cap".into());
                    } else if !readable {
                        disposition = Disposition::Skipped;
                        detail = Some(post_codestream_detail());
                    } else if cfg.gain_map {
                        disposition = Disposition::Metadata(MetadataKind::GainMap);
                    } else {
                        disposition = Disposition::Dropped;
                        detail = Some(
                            "parsed by the decoder; zencodec surfaces it only with \
                             extract_gain_map or a GainMapRender other than BaseOnly"
                                .into(),
                        );
                    }
                }
                b"jxli" => {
                    if plen > METADATA_BOX_CAP {
                        disposition = Disposition::Skipped;
                        detail = Some("larger than the decoder's 16 MiB cap".into());
                    } else if !readable {
                        disposition = Disposition::Skipped;
                        detail = Some(post_codestream_detail());
                    } else {
                        disposition = Disposition::Dropped;
                        detail = Some(
                            "frame index: parsed by the decoder (native frame_index() only), \
                             not surfaced through zencodec"
                                .into(),
                        );
                    }
                }
                b"Exif" | b"xml " => {
                    let (enabled, kind, name) = if &ty == b"Exif" {
                        (cfg.exif, MetadataKind::Exif, "Exif")
                    } else {
                        (cfg.xmp, MetadataKind::Xmp, "XMP")
                    };
                    if plen > METADATA_BOX_CAP {
                        disposition = Disposition::Skipped;
                        detail = Some("larger than the decoder's 16 MiB cap".into());
                    } else if !readable {
                        disposition = Disposition::Skipped;
                        detail = Some(post_codestream_detail());
                    } else if &ty == b"Exif" && plen < 4 {
                        disposition = Disposition::Dropped;
                        detail = Some(
                            "shorter than the 4-byte TIFF offset; the decoder ignores it".into(),
                        );
                    } else if !enabled {
                        disposition = Disposition::Dropped;
                        detail = Some(format!("{name} disabled by the decode policy"));
                    } else {
                        disposition = Disposition::Metadata(kind);
                    }
                }
                b"brob" => {
                    disposition = Disposition::Skipped;
                    let inner = body.get(..4).map(label_of);
                    detail = Some(
                        "brotli-compressed box; zenjxl-decoder is built without its `jpeg` \
                         feature, which skips brob boxes, so wrapped Exif/XMP never surfaces"
                            .into(),
                    );
                    label = inner;
                }
                b"jumb" => {
                    disposition = Disposition::Skipped;
                    label = jumb_label(body);
                    detail = Some("JUMBF superbox; the decoder skips it".into());
                }
                b"jxll" => {
                    disposition = Disposition::Skipped;
                    detail = Some("codestream level box; the decoder skips it".into());
                }
                _ => {}
            }
        }

        if let Some(why) = reject {
            // Boxes before this one were read normally; nothing after is.
            dead = Some(raw.start);
            let _ = why;
        }
        if let Some(full) = raw.truncated {
            detail = Some(format!(
                "declared size {full} runs {} bytes past the end of the input",
                raw.start.saturating_add(full).saturating_sub(len)
            ));
        }
        if raw.to_eof && detail.is_none() {
            detail = Some("size field 0: runs to the end of the input".into());
        }

        planned.push(Planned {
            raw,
            disposition,
            label,
            detail,
        });
        if planned.last().is_some_and(|p| p.raw.truncated.is_some()) {
            break;
        }
        at = planned.last().map_or(len, |p| p.raw.end);
        index += 1;
    }

    // ── where the codestream really ends ────────────────────────────────────
    // A size-0 `jxlc` claims everything to the end of the input, and any
    // codestream box may carry bytes past the codestream: ask the decoder's
    // header-only scan where the last frame ends.
    let is_cs = |p: &Planned| {
        p.disposition == Disposition::ImageData && matches!(&p.raw.ty, b"jxlc" | b"jxlp")
    };
    if planned.iter().any(is_cs)
        && let Ok(end) = codestream_end(data, stop)?
        && let Some(p) = planned.iter_mut().rev().find(|p| is_cs(p))
    {
        let skip = if &p.raw.ty == b"jxlp" { 4 } else { 0 };
        let payload_start = p.raw.start + p.raw.header + skip;
        if end > payload_start && end < p.raw.end {
            if p.raw.to_eof {
                p.raw.end = end;
                p.detail = Some(
                    "size field 0: runs to the end of the input; the codestream ends here"
                        .to_string(),
                );
            } else {
                let extra = p.raw.end - end;
                let note = format!("the codestream ends {extra} bytes before the box does");
                p.detail = Some(match p.detail.take() {
                    Some(d) => format!("{d}; {note}"),
                    None => note,
                });
            }
        }
    }

    // ── pass 2: "last Exif / xml box wins" ──────────────────────────────────
    for kind in [MetadataKind::Exif, MetadataKind::Xmp] {
        let name = if kind == MetadataKind::Exif {
            "Exif"
        } else {
            "XMP"
        };
        let mut winner: Option<usize> = None;
        for i in 0..planned.len() {
            if planned[i].disposition == Disposition::Metadata(kind) {
                if let Some(w) = winner {
                    planned[w].disposition = Disposition::Skipped;
                    planned[w].detail = Some(format!(
                        "superseded: the decoder keeps only the last {name} box"
                    ));
                }
                winner = Some(i);
            }
        }
    }

    // ── emit ────────────────────────────────────────────────────────────────
    for p in planned {
        let mut part = Part::new(
            PartKind::Box,
            PartTag::FourCc(p.raw.ty),
            p.raw.start..p.raw.end,
            p.disposition,
        );
        if let Some(l) = p.label {
            part = part.with_label(l);
        }
        if let Some(d) = p.detail {
            part = part.with_detail(d);
        }
        inv.push(None, part)?;
    }
    if let Some((start, why, after_cs)) = tail {
        let (kind, disposition) = if after_cs {
            (PartKind::Trailer, Disposition::Trailing)
        } else {
            (PartKind::Gap, Disposition::Malformed)
        };
        inv.push(
            None,
            Part::new(kind, PartTag::None, start..len, disposition)
                .with_detail(format!("does not frame as a box: {why}")),
        )?;
    }
    inv.fill_gaps(None, Disposition::Trailing)?;
    Ok(inv)
}

fn post_codestream_detail() -> String {
    "follows the last codestream box: zenjxl reads metadata right after the pixels \
     (decode.rs:935-940, codec.rs:3069-3073), when the decoder has only parsed boxes up to the \
     end of the codestream; measured, it has also parsed these only for single-`jxlc` files \
     with a codestream of at most 4096 bytes"
        .to_string()
}
