//! Structural inventory walker for JPEG XL files (container level).
//!
//! Walks the ISO/IEC 18181-2 box structure without decoding pixels and records
//! what the zencodec decode path (`JxlDecodeJob`) does with each box. The
//! codestream itself is one opaque `ImageData` part: frame headers, extra
//! channel names and header extension bits are not walked (see
//! `docs/INVENTORY.md`).
//!
//! The box rules mirror `zenjxl-decoder`'s `BoxParser`
//! (`src/api/inner/box_parser.rs`, `get_more_codestream_impl`), because the
//! inventory has to describe what that parser does, not what the specification
//! allows. The end of the codestream comes from the decoder's header-only
//! frame scan (`scan_frames_only`), which allocates no per-frame pixel buffers.
//!
//! Reading model (with the extra poll added in `decode.rs`, zenjxl#22):
//!
//! 1. **Main phase.** Every box up to and including the codestream box that
//!    holds the last codestream byte is parsed. A rejected box there fails the
//!    decode.
//! 2. **Drain phase.** If that box ends exactly where the codestream ends, the
//!    boxes after it are parsed until the end of the input, a rejected box, or
//!    the next non-empty codestream box. A rejected box there only stops the
//!    walk: the image has already decoded.
//! 3. Everything after the point where parsing stops is never read.

extern crate std;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;
use std::sync::OnceLock;

use zencodec::ImageFormat;
use zencodec::inventory::{
    Disposition, Inventory, InventoryError, MetadataKind, Part, PartId, PartKind, PartTag,
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
/// Memory cap for the header scan when the caller sets none.
const DEFAULT_SCAN_MEMORY: u64 = 256 * 1024 * 1024;
/// Longest label copied from the file.
const LABEL_MAX: usize = 64;
/// Out-of-order `jxlp` boxes the decoder buffers (`OooJxlp::MAX_BUFFERED_BOXES`).
const MAX_OOO: usize = 1 << 20;

/// What a decode job does with a `jhgm` gain-map bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GainMapMode {
    /// `BaseOnly` without `extract_gain_map`: parsed, not surfaced.
    Off,
    /// `extract_gain_map`: the bundle is attached to the output.
    Surface,
    /// `Components`: the gain-map codestream is decoded; a failure fails the decode.
    Components,
    /// Native `ReconstructHdr` (feature `reconstruct-hdr`): the ISO 21496-1
    /// metadata must parse and the gain map is applied to the base.
    Reconstruct,
}

/// What the caller's decode job would surface.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WalkConfig {
    /// `DecodePolicy::resolve_exif(true)`.
    pub exif: bool,
    /// `DecodePolicy::resolve_xmp(true)`.
    pub xmp: bool,
    /// What the job does with a `jhgm` gain-map bundle.
    pub gain_map: GainMapMode,
    /// The caller's `max_memory_bytes`, applied to the header scan.
    pub max_memory: Option<u64>,
}

/// Why a walk produced no inventory.
#[derive(Debug)]
pub(crate) enum WalkError {
    /// The part cap was reached.
    Parts(InventoryError),
    /// The caller's stop token fired.
    Stopped(enough::StopReason),
    /// The caller's memory limit stopped the header scan.
    Limit(String),
}

impl From<InventoryError> for WalkError {
    fn from(e: InventoryError) -> Self {
        Self::Parts(e)
    }
}

// ── header scan ─────────────────────────────────────────────────────────────

struct ScanOut {
    /// Codestream bytes the frames used (for a bare codestream: the offset
    /// just past the last byte), or why that is unknown.
    end: Result<u64, String>,
    /// The scan had parsed an `Exif` box by the time it finished.
    exif_seen: bool,
    /// More than one visible frame: a one-shot decode stops after the first.
    animated: bool,
}

/// Drive `zenjxl-decoder` through a header-only scan: frame headers and TOCs
/// are parsed, section data is skipped, no pixels or per-frame buffers exist.
fn scan(
    data: &[u8],
    max_memory: Option<u64>,
    stop: Option<&dyn enough::Stop>,
) -> Result<ScanOut, WalkError> {
    use jxl::api::{ErrorClass, JxlDecoder, JxlDecoderOptions, ProcessingResult, states};

    let mut options = JxlDecoderOptions::default().with_scan_frames_only(true);
    options.limits.max_icc_size = Some(16 << 20);
    options.limits.max_memory_bytes = Some(max_memory.unwrap_or(DEFAULT_SCAN_MEMORY));
    let mut input: &[u8] = data;
    let fail = |what: &str, kind: ErrorClass, msg: String| -> Result<ScanOut, WalkError> {
        // Only a refused memory budget is a resource error of the caller's
        // limit; every other failure (bad ICC, too many pixels, malformed
        // header) is malformed input and leaves the end "not located".
        let memory = kind == ErrorClass::OutOfMemory
            || (kind == ErrorClass::LimitExceeded && msg.contains("memory_bytes"));
        if max_memory.is_some() && memory {
            return Err(WalkError::Limit(format!("{what}: {msg}")));
        }
        Ok(ScanOut {
            end: Err(format!("{what}: {msg}")),
            exif_seen: false,
            animated: false,
        })
    };
    let truncated = |what: &str| ScanOut {
        end: Err(format!("truncated in {what}")),
        exif_seen: false,
        animated: false,
    };
    let decoder = JxlDecoder::<states::Initialized>::new(options);
    let mut decoder = match decoder.process(&mut input) {
        Ok(ProcessingResult::Complete { result }) => result,
        Ok(ProcessingResult::NeedsMoreInput { .. }) => return Ok(truncated("the image header")),
        Err(e) => return fail("image header", e.error().kind(), e.error().to_string()),
    };
    let mut frames = 0u32;
    while decoder.has_more_frames() {
        if let Some(s) = stop {
            s.check().map_err(WalkError::Stopped)?;
        }
        let with_frame = match decoder.process(&mut input) {
            Ok(ProcessingResult::Complete { result }) => result,
            Ok(ProcessingResult::NeedsMoreInput { .. }) => {
                return Ok(truncated(&format!("frame {frames}'s header")));
            }
            Err(e) => {
                return fail(
                    &format!("frame {frames} header"),
                    e.error().kind(),
                    e.error().to_string(),
                );
            }
        };
        decoder = match with_frame.skip_frame(&mut input) {
            Ok(ProcessingResult::Complete { result }) => result,
            Ok(ProcessingResult::NeedsMoreInput { .. }) => {
                return Ok(truncated(&format!("frame {frames}'s sections")));
            }
            Err(e) => {
                return fail(
                    &format!("frame {frames} sections"),
                    e.error().kind(),
                    e.error().to_string(),
                );
            }
        };
        frames = frames.saturating_add(1);
    }
    let end = decoder
        .codestream_length()
        .filter(|&n| n <= data.len() as u64)
        .ok_or_else(|| "the decoder reported no codestream length".to_string());
    Ok(ScanOut {
        end,
        exif_seen: decoder.exif().is_some(),
        animated: decoder.scanned_frames().len() > 1,
    })
}

/// Whether the linked `zenjxl-decoder` was built with its `jpeg` feature, which
/// makes it buffer `jbrd` and decompress `brob` boxes. Cargo unifies features
/// across the dependency graph, so this cannot be a compile-time constant of
/// this crate: ask the decoder itself, once, with a tiny container whose
/// brotli-wrapped Exif box precedes the codestream.
fn decoder_reads_brob() -> bool {
    static CELL: OnceLock<bool> = OnceLock::new();
    *CELL.get_or_init(|| {
        // A 1x1 lossless codestream written by jxl-encoder.
        const CS: [u8; 23] = [
            0xff, 0x0a, 0x00, 0x10, 0x10, 0x09, 0x08, 0x02, 0x01, 0x00, 0x2c, 0x00, 0x4b, 0x12,
            0x08, 0x16, 0x07, 0xd8, 0x43, 0xee, 0x32, 0x4a, 0x30,
        ];
        // brotli: one uncompressed meta-block holding "\0\0\0\0EXIFPROBE".
        const BROTLI: [u8; 17] = [
            0xc0, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x45, 0x58, 0x49, 0x46, 0x50, 0x52, 0x4f,
            0x42, 0x45, 0x03,
        ];
        let mut f = SIGNATURE.to_vec();
        push_box(&mut f, b"ftyp", b"jxl \0\0\0\0jxl ");
        let mut brob = b"Exif".to_vec();
        brob.extend(BROTLI);
        push_box(&mut f, b"brob", &brob);
        push_box(&mut f, b"jxlc", &CS);
        scan(&f, None, None).is_ok_and(|s| s.exif_seen)
    })
}

fn push_box(out: &mut Vec<u8>, ty: &[u8; 4], payload: &[u8]) {
    out.extend((8 + payload.len() as u32).to_be_bytes());
    out.extend(ty);
    out.extend(payload);
}

// ── box framing ─────────────────────────────────────────────────────────────

/// One box as framed on disk, before any interpretation.
struct RawBox {
    start: u64,
    end: u64,
    header: u64,
    ty: [u8; 4],
    /// The declared size ran past the input; `end` is the input length.
    truncated: Option<u64>,
    /// Size field 0 (or 1 with a 64-bit size of 0): runs to the end of the input.
    to_eof: bool,
}

/// Why a box header cannot be framed.
enum FrameFail {
    /// Fewer bytes than a header needs: the decoder runs out of input.
    Short(String),
    /// The size is smaller than the header: the decoder rejects the box.
    Invalid(String),
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

fn frame_box(data: &[u8], at: u64) -> Result<RawBox, FrameFail> {
    let len = data.len() as u64;
    let left = len - at;
    let (Some(size32), Some(ty)) = (
        be32(data, at),
        data.get(at as usize + 4..at as usize + 8)
            .and_then(|s| <[u8; 4]>::try_from(s).ok()),
    ) else {
        return Err(FrameFail::Short(format!(
            "{left} bytes left, a box header needs 8"
        )));
    };
    let (header, total, to_eof) = match size32 {
        0 => (8, left, true),
        1 => match be64(data, at + 8) {
            Some(0) => (16, left, true),
            Some(total) => (16, total, false),
            None => {
                return Err(FrameFail::Short("truncated extended box size".to_string()));
            }
        },
        n => (8, u64::from(n), false),
    };
    if total < header {
        return Err(FrameFail::Invalid(format!(
            "box size {total} is smaller than its {header}-byte header"
        )));
    }
    let (end, truncated) = match at.checked_add(total) {
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

// ── the walk ────────────────────────────────────────────────────────────────

/// Which codestream box the walk is in, mirroring `CodestreamBoxType`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cs {
    None,
    Jxlc,
    Jxlp(u32),
    Last,
}

/// Where the decoder's box parsing stands.
enum Phase {
    /// Parsing boxes on the way to the end of the codestream.
    Main,
    /// The codestream is complete; trailing boxes are still being parsed.
    Drain,
    /// Parsing stopped; the string is the `detail` of every later box.
    Dead(String),
}

/// A child part inside a split box.
struct Child {
    range: Range<u64>,
    disposition: Disposition,
    label: Option<&'static str>,
    detail: &'static str,
}

/// What the walk decided about one box.
struct Verdict {
    disposition: Disposition,
    label: Option<String>,
    detail: Option<String>,
    /// Box-level rejection (the string is why).
    reject: Option<String>,
    /// Body start and children, for a box split into used and unused bytes.
    split: Option<(u64, Vec<Child>)>,
}

impl Verdict {
    fn new(disposition: Disposition) -> Self {
        Self {
            disposition,
            label: None,
            detail: None,
            reject: None,
            split: None,
        }
    }
    fn with_detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    fn rejected(why: impl Into<String>) -> Self {
        let mut v = Self::new(Disposition::Malformed);
        v.reject = Some(why.into());
        v
    }
}

// Nested gain-map scans report a descriptive failure to the walker. Remember a
// stop reason so that classification cannot turn a one-shot timeout into success.
struct LatchedStop<'a> {
    inner: &'a dyn enough::Stop,
    reason: OnceLock<enough::StopReason>,
}
impl enough::Stop for LatchedStop<'_> {
    fn check(&self) -> Result<(), enough::StopReason> {
        if let Some(reason) = self.reason.get() {
            return Err(*reason);
        }
        self.inner
            .check()
            .map_err(|reason| *self.reason.get_or_init(|| reason))
    }
}

struct Walker<'a> {
    inv: Inventory,
    data: &'a [u8],
    stop: Option<&'a dyn enough::Stop>,
    cfg: WalkConfig,
    brob: bool,
    end: Option<u64>,
    end_note: Option<String>,
    phase: Phase,
    state: Cs,
    ooo: BTreeMap<u32, bool>,
    ooo_seen: bool,
    version: u32,
    index: usize,
    complete: bool,
    /// More than one visible frame (see [`ScanOut::animated`]).
    animated: bool,
    /// Codestream bytes carried by the codestream boxes walked so far.
    cs_before: u64,
    fail_at: Option<(u64, String)>,
    consumed: Vec<PartId>,
    last_exif: Option<PartId>,
    last_xmp: Option<PartId>,
    last_gain_map: Option<PartId>,
    /// The latest `jhgm` would fail the decode in the configured gain-map mode.
    gain_map_fatal: Option<(u64, String)>,
}

/// Walk `data` and describe every byte.
pub(crate) fn walk(
    data: &[u8],
    cfg: WalkConfig,
    stop: Option<&dyn enough::Stop>,
) -> Result<Inventory, WalkError> {
    let latched = stop.map(|inner| LatchedStop {
        inner,
        reason: OnceLock::new(),
    });
    let stop = latched.as_ref().map(|s| s as &dyn enough::Stop);

    if let Some(stop) = stop {
        stop.check().map_err(WalkError::Stopped)?;
    }
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
            let out = scan(data, cfg.max_memory, stop)?;
            let (end, note) = match out.end {
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
        let why = if data == &[0xFF][..] || SIGNATURE.starts_with(data) {
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

    let out = scan(data, cfg.max_memory, stop)?;
    let animated = out.animated;
    let (end, end_note) = match out.end {
        Ok(e) => (Some(e), None),
        Err(why) => (None, Some(format!("codestream end not located ({why})"))),
    };
    let mut w = Walker {
        inv,
        data,
        stop,
        cfg,
        brob: decoder_reads_brob(),
        end,
        end_note,
        phase: Phase::Main,
        state: Cs::None,
        ooo: BTreeMap::new(),
        ooo_seen: false,
        version: 0,
        index: 0,
        complete: false,
        animated,
        cs_before: 0,
        fail_at: None,
        consumed: Vec::new(),
        last_exif: None,
        last_xmp: None,
        last_gain_map: None,
        gain_map_fatal: None,
    };
    w.run(stop)?;
    if let Some(stop) = stop {
        stop.check().map_err(WalkError::Stopped)?;
    }
    w.finish()
}

impl Walker<'_> {
    fn run(&mut self, stop: Option<&dyn enough::Stop>) -> Result<(), WalkError> {
        let len = self.data.len() as u64;
        let mut at = 12u64;
        while at < len {
            if self.index.is_multiple_of(1024)
                && let Some(s) = stop
            {
                s.check().map_err(WalkError::Stopped)?;
            }
            let raw = match frame_box(self.data, at) {
                Ok(raw) => raw,
                Err(fail) => return self.tail(at, fail),
            };
            at = raw.end;
            self.box_part(raw)?;
            self.index += 1;
        }
        Ok(())
    }

    /// Bytes at `at` that do not frame as a box.
    fn tail(&mut self, at: u64, fail: FrameFail) -> Result<(), WalkError> {
        let len = self.data.len() as u64;
        let (FrameFail::Short(why) | FrameFail::Invalid(why)) = fail;
        let (kind, disposition, detail) = if matches!(self.phase, Phase::Main) {
            // The decoder still needs boxes to reach the end of the codestream.
            self.fail_at.get_or_insert((at, why.clone()));
            (
                PartKind::Gap,
                Disposition::Malformed,
                format!("decode fails here: {why}"),
            )
        } else {
            (
                PartKind::Trailer,
                Disposition::Trailing,
                format!("does not frame as a box: {why}"),
            )
        };
        self.inv.push(
            None,
            Part::new(kind, PartTag::None, at..len, disposition).with_detail(detail),
        )?;
        Ok(())
    }

    fn box_part(&mut self, raw: RawBox) -> Result<(), WalkError> {
        let data = self.data;
        let ps = raw.start + raw.header;
        let plen = raw.end - ps;
        let body = &data[ps as usize..raw.end as usize];
        let ty = raw.ty;
        let is_codestream = matches!(&ty, b"jxlc" | b"jxlp");

        let was_dead = matches!(self.phase, Phase::Dead(_));
        let mut v = if let Phase::Dead(why) = &self.phase {
            // A codestream box still counts as unread payload, not as a skipped type.
            let d = if is_codestream && plen > 0 && self.complete {
                Disposition::Unreferenced
            } else {
                Disposition::Skipped
            };
            Verdict::new(d).with_detail(why.clone())
        } else {
            self.classify(&raw, ps, plen, body)
        };

        if let Some(full) = raw.truncated {
            let past = raw
                .start
                .saturating_add(full)
                .saturating_sub(data.len() as u64);
            let note = format!("declared size {full} runs {past} bytes past the end of the input");
            if is_codestream || was_dead {
                v.detail = Some(match v.detail.take() {
                    Some(d) => format!("{d}; {note}"),
                    None => note,
                });
            } else if v.reject.is_none() {
                // The decoder runs out of input inside this box.
                v = Verdict::rejected("the box runs past the end of the input");
                v.detail = Some(note);
            }
        }
        if raw.to_eof && v.detail.is_none() && v.reject.is_none() {
            v.detail = Some("size field 0: runs to the end of the input".to_string());
        }

        // Reject bookkeeping: fail the decode (main phase) or stop the walk.
        if let Some(why) = v.reject.clone() {
            // Once the last codestream box has been entered, a box that fails
            // to parse stops the walk without failing the decode.
            let fatal =
                matches!(self.phase, Phase::Main) && !matches!(self.state, Cs::Jxlc | Cs::Last);
            match self.phase {
                Phase::Main if fatal => {
                    self.fail_at = Some((raw.start, why.clone()));
                    v.detail = Some(match v.detail.take() {
                        Some(d) => format!("decode fails here: {why} ({d})"),
                        None => format!("decode fails here: {why}"),
                    });
                    self.phase = Phase::Dead(format!(
                        "never read: the decoder rejects the file at offset {}",
                        raw.start
                    ));
                }
                Phase::Main | Phase::Drain => {
                    v.detail = Some(format!(
                        "the decoder stops reading boxes here: {why}; the codestream is \
                         whole, so the decode is not affected"
                    ));
                    self.phase = Phase::Dead(format!(
                        "never read: the decoder stops reading boxes at offset {}",
                        raw.start
                    ));
                }
                Phase::Dead(_) => {}
            }
        }
        self.emit(&raw, v)
    }

    fn emit(&mut self, raw: &RawBox, v: Verdict) -> Result<(), WalkError> {
        let mut part = Part::new(
            PartKind::Box,
            PartTag::FourCc(raw.ty),
            raw.start..raw.end,
            v.disposition,
        );
        if let Some(l) = v.label {
            part = part.with_label(l);
        }
        if let Some(d) = v.detail {
            part = part.with_detail(d);
        }
        if let Some((body_start, _)) = &v.split {
            part = part.with_body(*body_start..raw.end);
        }
        let id = self.inv.push(None, part)?;
        if is_consumed_content(v.disposition) {
            self.consumed.push(id);
        }
        if let Some((_, children)) = v.split {
            for c in children {
                let mut p = Part::new(PartKind::Chunk, PartTag::None, c.range, c.disposition)
                    .with_detail(c.detail);
                if let Some(l) = c.label {
                    p = p.with_label(l);
                }
                let cid = self.inv.push(Some(id), p)?;
                if is_consumed_content(c.disposition) {
                    self.consumed.push(cid);
                }
            }
        }
        self.record_winner(v.disposition, id);
        Ok(())
    }

    /// "Last box wins": the decoder keeps only the last Exif / XMP / gain map.
    fn record_winner(&mut self, d: Disposition, id: PartId) {
        let slot = match d {
            Disposition::Metadata(MetadataKind::Exif) => &mut self.last_exif,
            Disposition::Metadata(MetadataKind::Xmp) => &mut self.last_xmp,
            Disposition::Metadata(MetadataKind::GainMap) => &mut self.last_gain_map,
            _ => return,
        };
        if let Some(prev) = slot.replace(id) {
            self.inv.set_disposition(prev, Disposition::Skipped);
            self.inv.set_detail(
                prev,
                "superseded: the decoder keeps only the last box of this kind",
            );
        }
    }

    fn classify(&mut self, raw: &RawBox, ps: u64, plen: u64, body: &[u8]) -> Verdict {
        let ty = raw.ty;
        let is_ftyp = &ty == b"ftyp";
        if (self.index == 0) != is_ftyp {
            return Verdict::rejected(if self.index == 0 {
                "`ftyp` must be the first box after the signature"
            } else {
                "a second `ftyp` box"
            });
        }
        match &ty {
            b"ftyp" => self.ftyp(raw, plen, body),
            b"jxlc" => self.jxlc(raw, ps, plen),
            b"jxlp" => self.jxlp(raw, ps, plen, body),
            b"jbrd" => {
                if self.brob {
                    Verdict::new(Disposition::Dropped).with_detail(
                        "JPEG reconstruction data: buffered by the decoder, never surfaced \
                         through zencodec",
                    )
                } else {
                    Verdict::new(Disposition::Skipped)
                        .with_detail("JPEG reconstruction data; the decoder skips it")
                }
            }
            b"jhgm" => {
                if raw.to_eof {
                    return Verdict::rejected("a size-0 `jhgm` box");
                }
                if plen > GAIN_MAP_BOX_CAP {
                    return Verdict::new(Disposition::Skipped)
                        .with_detail("larger than the decoder's 256 MiB gain-map cap");
                }
                let bundle = match jxl::api::GainMapBundle::parse(body) {
                    Ok(b) => b,
                    Err(e) => {
                        return Verdict::rejected(format!(
                            "unparseable `jhgm` bundle ({})",
                            e.error()
                        ));
                    }
                };
                // The latest bundle decides: an earlier failing one is replaced.
                self.gain_map_fatal = None;
                match self.cfg.gain_map {
                    GainMapMode::Off => Verdict::new(Disposition::Dropped).with_detail(
                        "parsed by the decoder; zencodec surfaces it only with \
                         extract_gain_map or a GainMapRender other than BaseOnly",
                    ),
                    mode => {
                        if let Some(why) = self.gain_map_failure(mode, &bundle) {
                            self.gain_map_fatal = Some((raw.start, why));
                        }
                        Verdict::new(Disposition::Metadata(MetadataKind::GainMap)).with_detail(
                            "only the gain-map codestream and the parsed parameters reach the \
                             caller; the bundle's color encoding and alternate ICC do not",
                        )
                    }
                }
            }
            b"jxli" => {
                if raw.to_eof {
                    return Verdict::rejected("a size-0 `jxli` box");
                }
                if plen > METADATA_BOX_CAP {
                    return Verdict::new(Disposition::Skipped)
                        .with_detail("larger than the decoder's 16 MiB cap");
                }
                if let Err(e) = jxl::api::FrameIndexBox::parse(body) {
                    return Verdict::rejected(format!("unparseable `jxli` box ({})", e.error()));
                }
                Verdict::new(Disposition::Dropped).with_detail(
                    "frame index: parsed by the decoder (native frame_index() only), not \
                     surfaced through zencodec",
                )
            }
            b"Exif" => self.metadata(raw, ps, plen, true),
            b"xml " => self.metadata(raw, ps, plen, false),
            b"brob" => {
                let inner = body.get(..4);
                let mut v = if self.brob && plen <= METADATA_BOX_CAP && !raw.to_eof {
                    match inner {
                        Some(b"Exif") if self.cfg.exif => {
                            Verdict::new(Disposition::Metadata(MetadataKind::Exif)).with_detail(
                                "brotli-compressed; the inventory does not decompress it, so a \
                                 corrupt stream would be dropped instead",
                            )
                        }
                        Some(b"xml ") if self.cfg.xmp => {
                            Verdict::new(Disposition::Metadata(MetadataKind::Xmp)).with_detail(
                                "brotli-compressed; the inventory does not decompress it, so a \
                                 corrupt stream would be dropped instead",
                            )
                        }
                        Some(b"Exif" | b"xml ") => Verdict::new(Disposition::Dropped)
                            .with_detail("disabled by the decode policy"),
                        _ => Verdict::new(Disposition::Skipped)
                            .with_detail("brotli box of a type the decoder does not route"),
                    }
                } else {
                    Verdict::new(Disposition::Skipped)
                        .with_detail("brotli-compressed box; this decoder build skips brob boxes")
                };
                v.label = inner.map(label_of);
                v
            }
            b"jumb" => {
                let mut v = Verdict::new(Disposition::Skipped)
                    .with_detail("JUMBF superbox; the decoder skips it");
                v.label = jumb_label(body);
                v
            }
            b"jxll" => Verdict::new(Disposition::Skipped)
                .with_detail("codestream level box; the decoder skips it"),
            _ => Verdict::new(Disposition::Unknown),
        }
    }

    /// Why decoding this bundle in `mode` would fail the whole decode, if it would.
    fn gain_map_failure(
        &self,
        mode: GainMapMode,
        bundle: &jxl::api::GainMapBundle,
    ) -> Option<String> {
        if matches!(mode, GainMapMode::Components | GainMapMode::Reconstruct) {
            // `Components` decodes the gain-map codestream: probe its headers.
            match scan(&bundle.gain_map_codestream, self.cfg.max_memory, self.stop) {
                Ok(ScanOut { end: Err(why), .. }) => {
                    return Some(format!("the gain-map codestream does not decode ({why})"));
                }
                Err(WalkError::Limit(why)) => {
                    return Some(format!(
                        "the gain-map codestream exceeds the limits ({why})"
                    ));
                }
                _ => {}
            }
        }
        if mode == GainMapMode::Reconstruct
            && zencodec::gainmap::parse_iso21496_fmt(
                &bundle.metadata,
                zencodec::gainmap::Iso21496Format::JxlJhgm,
            )
            .is_err()
        {
            return Some("the ISO 21496-1 metadata does not parse".to_string());
        }
        None
    }

    fn ftyp(&mut self, raw: &RawBox, plen: u64, body: &[u8]) -> Verdict {
        if raw.to_eof || plen < 8 || &body[..4] != b"jxl " {
            return Verdict::rejected("`ftyp` must carry brand `jxl `");
        }
        let version = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
        if version > 1 {
            return Verdict::rejected("`ftyp` version above 1");
        }
        self.version = version;
        let mut v = Verdict::new(Disposition::Structure)
            .with_detail(format!("file format version {version}"));
        v.label = Some("jxl ".to_string());
        v
    }

    fn jxlc(&mut self, raw: &RawBox, ps: u64, plen: u64) -> Verdict {
        if matches!(self.state, Cs::Jxlp(_) | Cs::Last) {
            return Verdict::rejected("`jxlc` after `jxlp`");
        }
        self.state = Cs::Jxlc;
        self.codestream_box(raw, ps, plen, None)
    }

    fn jxlp(&mut self, raw: &RawBox, ps: u64, plen: u64, body: &[u8]) -> Verdict {
        if plen < 4 {
            return Verdict::rejected("`jxlp` box without its 4-byte index");
        }
        let word = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
        let last = word & 0x8000_0000 != 0;
        let idx = word & 0x7fff_ffff;
        let wanted = match self.state {
            Cs::None => 0,
            Cs::Jxlp(i) => i.wrapping_add(1),
            Cs::Jxlc | Cs::Last => return Verdict::rejected("`jxlp` after a complete codestream"),
        };
        if idx < wanted {
            return Verdict::rejected("`jxlp` index out of order");
        }
        let mut note = format!("jxlp index {idx}");
        if last {
            note.push_str(" (last)");
        }
        if idx > wanted {
            if self.version < 1
                || self.ooo.contains_key(&idx)
                || self.ooo.len() >= MAX_OOO
                || raw.to_eof
            {
                return Verdict::rejected("`jxlp` index out of order");
            }
            self.ooo.insert(idx, last);
            self.ooo_seen = true;
            note.push_str(", out of order (ftyp version 1)");
            return Verdict::new(Disposition::ImageData).with_detail(note);
        }
        self.state = if last { Cs::Last } else { Cs::Jxlp(idx) };
        // Buffered out-of-order parts splice in as soon as their turn comes.
        loop {
            let next = match self.state {
                Cs::None => 0,
                Cs::Jxlp(i) => i.wrapping_add(1),
                _ => break,
            };
            match self.ooo.remove(&next) {
                Some(l) => self.state = if l { Cs::Last } else { Cs::Jxlp(next) },
                None => break,
            }
        }
        self.codestream_box(raw, ps + 4, plen - 4, Some(note))
    }

    /// A codestream box that passed the order rules. `ps`/`plen` describe the
    /// codestream bytes only (after a `jxlp` index word).
    fn codestream_box(
        &mut self,
        raw: &RawBox,
        ps: u64,
        plen: u64,
        note: Option<String>,
    ) -> Verdict {
        let pe = ps + plen;
        let join = |a: Option<String>, b: &str| match a {
            Some(a) => format!("{a}; {b}"),
            None => b.to_string(),
        };
        let mut v = Verdict::new(Disposition::ImageData);
        v.detail = note.clone();

        if self.complete {
            // The codestream already ended; the decoder enters this box (a
            // drain poll) but reads none of its payload.
            if plen == 0 {
                v.disposition = Disposition::Structure;
                return v;
            }
            v.disposition = Disposition::Unreferenced;
            v.detail = Some(join(
                note,
                "the codestream had already ended; the decoder skips this box's payload and \
                 reads the boxes after it",
            ));
            return v;
        }

        // Out-of-order parts make file offsets and codestream order disagree,
        // so completion is then judged by the box type sequence alone.
        let before = self.cs_before;
        self.cs_before = before.saturating_add(plen);
        let completes = match self.end {
            Some(e) if !self.ooo_seen => before < e && e <= before + plen,
            _ => matches!(self.state, Cs::Jxlc | Cs::Last),
        };
        if !completes {
            if let Some(n) = self.end_note.take() {
                v.detail = Some(join(v.detail.take(), &n));
            }
            return v;
        }
        self.complete = true;
        let used_end = match self.end {
            Some(e) if !self.ooo_seen => ps + (e - before),
            _ => pe,
        };
        if used_end < pe {
            // Junk after the codestream inside the box: the decoder skips it
            // and carries on with the boxes after this one.
            v.split = Some((
                raw.start + raw.header,
                split_children(raw, ps, used_end, pe),
            ));
        }
        self.phase = Phase::Drain;
        v
    }

    /// `Exif` (`exif == true`) or `xml `.
    fn metadata(&mut self, raw: &RawBox, ps: u64, plen: u64, exif: bool) -> Verdict {
        let (enabled, kind, name) = if exif {
            (self.cfg.exif, MetadataKind::Exif, "Exif")
        } else {
            (self.cfg.xmp, MetadataKind::Xmp, "XMP")
        };
        if plen > METADATA_BOX_CAP && !raw.to_eof {
            return Verdict::new(Disposition::Skipped)
                .with_detail("larger than the decoder's 16 MiB cap");
        }
        if exif && plen < 4 {
            return Verdict::new(Disposition::Dropped)
                .with_detail("shorter than the 4-byte TIFF offset; the decoder ignores it");
        }
        if !enabled {
            return Verdict::new(Disposition::Dropped)
                .with_detail(format!("{name} disabled by the decode policy"));
        }
        let mut v = Verdict::new(Disposition::Metadata(kind));
        if raw.to_eof && plen > METADATA_BOX_CAP {
            // fill_metadata_box keeps the first 16 MiB of a to-EOF box.
            let cut = ps + METADATA_BOX_CAP;
            v.split = Some((
                ps,
                vec![
                    Child {
                        range: ps..cut,
                        disposition: Disposition::Metadata(kind),
                        label: None,
                        detail: "the first 16 MiB of a size-0 box are kept",
                    },
                    Child {
                        range: cut..raw.end,
                        disposition: Disposition::Skipped,
                        label: None,
                        detail: "past the 16 MiB cap; the decoder then parses these bytes as \
                                 boxes",
                    },
                ],
            ));
        }
        v
    }

    fn finish(mut self) -> Result<Inventory, WalkError> {
        if let Some((at, why)) = self.gain_map_fatal.take()
            && self.fail_at.is_none()
        {
            if let Some(id) = self.last_gain_map {
                self.inv.set_disposition(id, Disposition::Malformed);
                self.inv.set_detail(id, format!("decode fails here: {why}"));
                self.consumed.retain(|c| *c != id);
            }
            self.fail_at = Some((at, why));
        }
        if let Some((at, why)) = self.fail_at.take() {
            let detail = format!("not used: decode fails at offset {at} ({why})");
            for id in core::mem::take(&mut self.consumed) {
                self.inv.set_disposition(id, Disposition::Skipped);
                self.inv.set_detail(id, detail.clone());
            }
        }
        self.inv.fill_gaps(None, Disposition::Trailing)?;
        Ok(self.inv)
    }
}

fn split_children(raw: &RawBox, ps: u64, used_end: u64, pe: u64) -> Vec<Child> {
    let mut kids = Vec::new();
    if ps > raw.start + raw.header {
        kids.push(Child {
            range: raw.start + raw.header..ps,
            disposition: Disposition::Structure,
            label: None,
            detail: "jxlp index word",
        });
    }
    kids.push(Child {
        range: ps..used_end,
        disposition: Disposition::ImageData,
        label: Some("codestream"),
        detail: "the codestream ends at the end of this part",
    });
    kids.push(Child {
        range: used_end..pe,
        disposition: Disposition::Unreferenced,
        label: None,
        detail: "bytes after the end of the codestream; the decoder never reads them",
    });
    kids
}

fn is_consumed_content(d: Disposition) -> bool {
    matches!(d, Disposition::ImageData | Disposition::Metadata(_))
}
