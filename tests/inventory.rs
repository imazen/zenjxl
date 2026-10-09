//! Structural inventory (`DecodeJob::inventory`) for JPEG XL: container boxes.
//!
//! - `check_inventory` on encoder output and on synthetic containers,
//! - a pinned part list for a container carrying every box type the decoder
//!   distinguishes plus unknown and trailing bytes,
//! - the claims the inventory makes about what the decoder surfaces, checked
//!   against a real decode,
//! - the corpus sweep (`INVENTORY_CORPUS_JXL`) and the `jxlinfo`/`exiftool`
//!   cross-check (`INVENTORY_ORACLE_JXLINFO`, `INVENTORY_ORACLE_EXIFTOOL`),
//!   both driven by `just inventory-oracle`.

#![cfg(all(feature = "zencodec", feature = "encode", feature = "decode"))]

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
use zencodec::inventory::{Disposition, Inventory, MetadataKind, PartKind, PartTag};
use zenjxl::{GainMapBundle, JxlDecoderConfig, JxlEncoderConfig};

// ── builders ────────────────────────────────────────────────────────────────

fn lossless(w: u32, h: u32, noisy: bool) -> Vec<u8> {
    let mut x = 12345u32;
    let pixels: Vec<rgb::Rgb<u8>> = (0..w * h)
        .map(|i| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let v = if noisy { (x >> 24) as u8 } else { i as u8 };
            rgb::Rgb {
                r: v,
                g: v ^ 1,
                b: v.wrapping_add(3),
            }
        })
        .collect();
    let buf = zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, w, h).expect("buffer");
    JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .encoder()
        .expect("encoder")
        .encode(buf.as_slice().into())
        .expect("encode")
        .into_vec()
}

fn lossy(w: u32, h: u32) -> Vec<u8> {
    let pixels: Vec<rgb::Rgb<u8>> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            rgb::Rgb {
                r: (x * 255 / w) as u8,
                g: (y * 255 / h) as u8,
                b: ((x ^ y) & 0xff) as u8,
            }
        })
        .collect();
    let buf = zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, w, h).expect("buffer");
    JxlEncoderConfig::new()
        .with_generic_quality(60.0)
        .job()
        .encoder()
        .expect("encoder")
        .encode(buf.as_slice().into())
        .expect("encode")
        .into_vec()
}

const SIG: [u8; 12] = [0, 0, 0, 12, b'J', b'X', b'L', b' ', 13, 10, 0x87, 10];

fn ftyp() -> Vec<u8> {
    let mut o = Vec::new();
    bx(&mut o, b"ftyp", b"jxl \0\0\0\0jxl ");
    o
}

fn bx(out: &mut Vec<u8>, ty: &[u8; 4], payload: &[u8]) {
    out.extend((8 + payload.len() as u32).to_be_bytes());
    out.extend(ty);
    out.extend(payload);
}

fn jxlp(out: &mut Vec<u8>, idx: u32, last: bool, payload: &[u8]) {
    let mut p = (idx | if last { 0x8000_0000 } else { 0 })
        .to_be_bytes()
        .to_vec();
    p.extend(payload);
    bx(out, b"jxlp", &p);
}

fn container(boxes: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut o = SIG.to_vec();
    o.extend(ftyp());
    boxes(&mut o);
    o
}

fn inventory_of(data: &[u8]) -> Inventory {
    JxlDecoderConfig::new()
        .job()
        .inventory(data)
        .expect("inventory")
        .expect("declared")
}

fn exif_payload() -> Vec<u8> {
    let mut p = vec![0, 0, 0, 0];
    p.extend(b"EXIF-TEST");
    p
}

// ── check_inventory ─────────────────────────────────────────────────────────

#[test]
fn declares_the_capability() {
    assert!(<JxlDecoderConfig as zencodec::decode::DecoderConfig>::capabilities().inventory());
}

#[test]
fn encoder_output_passes_check_inventory() {
    for bytes in [
        lossless(16, 16, false),
        lossy(48, 40),
        lossless(64, 64, true),
    ] {
        zencodec_testkit::check_inventory(JxlDecoderConfig::new(), &bytes).expect("inventory");
    }
}

#[test]
fn synthetic_containers_pass_check_inventory() {
    let cs = lossless(16, 16, false);
    let ex = exif_payload();
    let cases: Vec<Vec<u8>> = vec![
        container(|o| {
            bx(o, b"jxlc", &cs);
            bx(o, b"Exif", &ex);
            bx(o, b"xml ", b"<x/>");
        }),
        container(|o| {
            bx(o, b"Exif", &ex);
            bx(o, b"jxlc", &cs);
        }),
        container(|o| {
            jxlp(o, 0, false, &cs[..100]);
            bx(o, b"zzzz", b"private");
            jxlp(o, 1, true, &cs[100..]);
        }),
        // size-0 jxlc runs to the end
        container(|o| {
            o.extend(0u32.to_be_bytes());
            o.extend(b"jxlc");
            o.extend(&cs);
        }),
        // extended (64-bit) box size
        container(|o| {
            o.extend(1u32.to_be_bytes());
            o.extend(b"jxlc");
            o.extend((16 + cs.len() as u64).to_be_bytes());
            o.extend(&cs);
        }),
        // bare codestream, with and without trailing bytes
        cs.clone(),
        [cs.clone(), b"trailing".to_vec()].concat(),
    ];
    for (i, bytes) in cases.iter().enumerate() {
        zencodec_testkit::check_inventory(JxlDecoderConfig::new(), bytes)
            .unwrap_or_else(|e| panic!("case {i}: {e:?}"));
    }
}

#[test]
fn degenerate_inputs_yield_inventories() {
    for data in [
        &b""[..],
        &b"\xFF"[..],
        &b"\xFF\x0A"[..],
        &b"not a jxl file at all"[..],
        &SIG[..5],
        &SIG[..],
        &[SIG.as_slice(), &[0, 0, 0, 1][..]].concat()[..],
        &[SIG.as_slice(), &[0, 0, 0, 4, b'f', b't', b'y', b'p'][..]].concat()[..],
    ] {
        let inv = inventory_of(data);
        inv.validate()
            .unwrap_or_else(|e| panic!("{data:?}: {e}\n{inv}"));
    }
}

// ── the pinned fixture ──────────────────────────────────────────────────────

/// A container with every box type the decoder distinguishes, one private box
/// and bytes that do not frame as a box after the codestream.
fn kitchen_sink() -> (Vec<u8>, Vec<u8>) {
    let cs = lossless(16, 16, false);
    let gain_cs = lossless(8, 8, false);
    let bundle = GainMapBundle {
        metadata: vec![1, 2, 3],
        color_encoding: None,
        alt_icc_compressed: None,
        gain_map_codestream: gain_cs,
    }
    .serialize();
    let mut jumb = Vec::new();
    {
        let mut jumd = vec![0u8; 16];
        jumd.push(0x03); // requestable | labelled
        jumd.extend(b"c2pa.test\0");
        bx(&mut jumb, b"jumd", &jumd);
    }
    let bytes = container(|o| {
        bx(o, b"jxll", &[10]);
        bx(o, b"jumb", &jumb);
        bx(o, b"Exif", &exif_payload());
        bx(o, b"xml ", b"<x:xmpmeta/>");
        bx(o, b"jxli", &[0, 0, 0, 0, 1, 0, 0, 0, 1]);
        bx(o, b"jhgm", &bundle);
        bx(o, b"zzzz", b"private bytes");
        bx(o, b"brob", b"Exif\x01\x02\x03");
        bx(o, b"jbrd", b"jb");
        jxlp(o, 0, false, &cs[..100]);
        jxlp(o, 1, true, &cs[100..]);
        bx(o, b"Exif", &exif_payload());
        o.extend(b"tail!");
    });
    (bytes, cs)
}

type Row = (
    PartKind,
    PartTag,
    std::ops::Range<u64>,
    Disposition,
    Option<String>,
);

fn rows(inv: &Inventory) -> Vec<Row> {
    inv.children(None)
        .into_iter()
        .map(|id| {
            let p = inv.get(id).unwrap();
            (
                p.kind,
                p.tag.clone(),
                p.range.clone(),
                p.disposition,
                p.label.as_ref().map(|l| l.to_string()),
            )
        })
        .collect()
}

#[test]
fn pinned_kitchen_sink_inventory() {
    let (bytes, cs) = kitchen_sink();
    let inv = inventory_of(&bytes);
    inv.validate().expect("valid");
    zencodec_testkit::check_inventory(JxlDecoderConfig::new(), &bytes).expect("check");

    let cc = |s: &[u8; 4]| PartTag::FourCc(*s);
    let mut at = 0u64;
    let mut next = |len: u64| {
        let r = at..at + len;
        at += len;
        r
    };
    let bundle_len = {
        let (b, _) = (inv.get(inv.children(None)[7]).unwrap(), 0);
        b.range.end - b.range.start
    };
    use Disposition::*;
    let expected: Vec<Row> = vec![
        (
            PartKind::Header,
            PartTag::None,
            next(12),
            Structure,
            Some("JXL ".into()),
        ),
        (
            PartKind::Box,
            cc(b"ftyp"),
            next(20),
            Structure,
            Some("jxl ".into()),
        ),
        (PartKind::Box, cc(b"jxll"), next(9), Skipped, None),
        (
            PartKind::Box,
            cc(b"jumb"),
            next(8 + 8 + 16 + 1 + 10),
            Skipped,
            Some("c2pa.test".into()),
        ),
        (
            PartKind::Box,
            cc(b"Exif"),
            next(8 + exif_payload().len() as u64),
            Metadata(MetadataKind::Exif),
            None,
        ),
        (
            PartKind::Box,
            cc(b"xml "),
            next(8 + 12),
            Metadata(MetadataKind::Xmp),
            None,
        ),
        (PartKind::Box, cc(b"jxli"), next(17), Dropped, None),
        (PartKind::Box, cc(b"jhgm"), next(bundle_len), Dropped, None),
        (PartKind::Box, cc(b"zzzz"), next(8 + 13), Unknown, None),
        (
            PartKind::Box,
            cc(b"brob"),
            next(8 + 7),
            Skipped,
            Some("Exif".into()),
        ),
        (PartKind::Box, cc(b"jbrd"), next(10), Skipped, None),
        (PartKind::Box, cc(b"jxlp"), next(12 + 100), ImageData, None),
        (
            PartKind::Box,
            cc(b"jxlp"),
            next(12 + cs.len() as u64 - 100),
            ImageData,
            None,
        ),
        // after the last codestream box: not parsed for a jxlp codestream
        (
            PartKind::Box,
            cc(b"Exif"),
            next(8 + exif_payload().len() as u64),
            Skipped,
            None,
        ),
        (PartKind::Trailer, PartTag::None, next(5), Trailing, None),
    ];
    assert_eq!(rows(&inv), expected, "\n{inv}");
    assert_eq!(at, bytes.len() as u64);

    // details that carry information
    let by_tag = |tag: &[u8; 4]| -> Vec<_> {
        inv.parts()
            .iter()
            .filter(|p| p.tag == PartTag::FourCc(*tag))
            .collect()
    };
    let jxlps = by_tag(b"jxlp");
    assert_eq!(jxlps[0].detail.as_deref(), Some("jxlp index 0"));
    assert_eq!(jxlps[1].detail.as_deref(), Some("jxlp index 1 (last)"));
    assert!(by_tag(b"brob")[0].detail.as_ref().unwrap().contains("jpeg"));
}

#[test]
fn kitchen_sink_dispositions_match_a_real_decode() {
    let (bytes, _) = kitchen_sink();
    let out = JxlDecoderConfig::new()
        .job()
        .decoder(Cow::Owned(bytes.clone()), &[])
        .expect("decoder")
        .decode()
        .expect("the fixture decodes");
    let info = out.info();
    assert_eq!(
        info.embedded_metadata.exif.as_deref().map(|e| e.to_vec()),
        Some(exif_payload()[4..].to_vec()),
        "the pre-codestream Exif box is the one that surfaces"
    );
    assert_eq!(
        info.embedded_metadata.xmp.as_deref().map(|e| e.to_vec()),
        Some(b"<x:xmpmeta/>".to_vec())
    );
}

// ── claims about the decode path ────────────────────────────────────────────

fn surfaces(bytes: &[u8]) -> (bool, bool) {
    let out = JxlDecoderConfig::new()
        .job()
        .decoder(Cow::Owned(bytes.to_vec()), &[])
        .expect("decoder")
        .decode()
        .expect("decodes");
    let m = &out.info().embedded_metadata;
    (m.exif.is_some(), m.xmp.is_some())
}

fn disposition_of(inv: &Inventory, tag: &[u8; 4], nth: usize) -> Disposition {
    inv.parts()
        .iter()
        .filter(|p| p.tag == PartTag::FourCc(*tag))
        .nth(nth)
        .unwrap()
        .disposition
}

/// The model for boxes after the codestream: the inventory says they are read
/// exactly when a real decode surfaces them (single `jxlc`, codestream of at
/// most 4096 bytes), and a `jxlp` codestream is never claimed as read.
#[test]
fn trailing_box_model_matches_decoder() {
    let base = lossless(62, 62, true);
    let mut cases: Vec<(String, Vec<u8>)> = vec![];
    for target in [4000usize, 4095, 4096, 4097, 4098, 8192] {
        let mut cs = base.clone();
        cs.resize(target.max(base.len()), 0);
        cases.push((format!("padded jxlc {}", cs.len()), cs));
    }
    cases.push(("16x16".into(), lossless(16, 16, false)));
    cases.push(("256x256 noisy".into(), lossless(256, 256, true)));
    cases.push(("lossy 64x64".into(), lossy(64, 64)));
    cases.push(("lossy 512x512".into(), lossy(512, 512)));
    for (name, cs) in cases {
        let bytes = container(|o| {
            bx(o, b"jxlc", &cs);
            bx(o, b"Exif", &exif_payload());
            bx(o, b"xml ", b"<x/>");
        });
        let inv = inventory_of(&bytes);
        let (exif, xmp) = surfaces(&bytes);
        let claimed_exif = matches!(disposition_of(&inv, b"Exif", 0), Disposition::Metadata(_));
        let claimed_xmp = matches!(disposition_of(&inv, b"xml ", 0), Disposition::Metadata(_));
        assert_eq!((claimed_exif, claimed_xmp), (exif, xmp), "{name}\n{inv}");
    }
    // jxlp: the inventory never claims a trailing box is read; a real decode
    // may still surface it (it did for a 1000 + 2938 byte split), which the
    // detail text states.
    let cs = lossless(16, 16, false);
    let bytes = container(|o| {
        jxlp(o, 0, false, &cs[..100]);
        jxlp(o, 1, true, &cs[100..]);
        bx(o, b"Exif", &exif_payload());
    });
    let inv = inventory_of(&bytes);
    assert_eq!(disposition_of(&inv, b"Exif", 0), Disposition::Skipped);
}

#[test]
fn boxes_between_and_before_codestream_boxes_are_read() {
    let cs = lossless(62, 62, true);
    let bytes = container(|o| {
        bx(o, b"xml ", b"<x/>");
        jxlp(o, 0, false, &cs[..3000]);
        bx(o, b"Exif", &exif_payload());
        jxlp(o, 1, true, &cs[3000..]);
    });
    let inv = inventory_of(&bytes);
    assert_eq!(
        disposition_of(&inv, b"Exif", 0),
        Disposition::Metadata(MetadataKind::Exif)
    );
    assert_eq!(
        disposition_of(&inv, b"xml ", 0),
        Disposition::Metadata(MetadataKind::Xmp)
    );
    assert_eq!(surfaces(&bytes), (true, true));
}

#[test]
fn only_the_last_exif_box_wins() {
    let cs = lossless(16, 16, false);
    let mut first = vec![0, 0, 0, 0];
    first.extend(b"FIRST");
    let mut second = vec![0, 0, 0, 0];
    second.extend(b"SECOND");
    let bytes = container(|o| {
        bx(o, b"Exif", &first);
        bx(o, b"Exif", &second);
        bx(o, b"jxlc", &cs);
    });
    let inv = inventory_of(&bytes);
    assert_eq!(disposition_of(&inv, b"Exif", 0), Disposition::Skipped);
    assert_eq!(
        disposition_of(&inv, b"Exif", 1),
        Disposition::Metadata(MetadataKind::Exif)
    );
    let out = JxlDecoderConfig::new()
        .job()
        .decoder(Cow::Owned(bytes), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(
        out.info()
            .embedded_metadata
            .exif
            .as_deref()
            .map(|e| e.to_vec()),
        Some(b"SECOND".to_vec())
    );
}

#[test]
fn policy_and_gain_map_settings_change_dispositions() {
    use zencodec::decode::DecodePolicy;
    let (bytes, _) = kitchen_sink();
    let strict = JxlDecoderConfig::new()
        .job()
        .with_policy(DecodePolicy::strict())
        .inventory(&bytes)
        .unwrap()
        .unwrap();
    assert_eq!(disposition_of(&strict, b"Exif", 0), Disposition::Dropped);
    assert_eq!(disposition_of(&strict, b"xml ", 0), Disposition::Dropped);

    let with_gain_map = JxlDecoderConfig::new()
        .job()
        .with_extract_gain_map(true)
        .inventory(&bytes)
        .unwrap()
        .unwrap();
    assert_eq!(
        disposition_of(&with_gain_map, b"jhgm", 0),
        Disposition::Metadata(MetadataKind::GainMap)
    );
}

/// `brob` stays skipped only while zenjxl-decoder is built without its `jpeg`
/// feature. A valid brotli stream carrying `Exif` would surface if that
/// changed; this test then fails and the inventory needs updating.
#[test]
fn brob_wrapped_exif_is_skipped_by_the_decoder() {
    // brotli stream for the 8 bytes "\0\0\0\0EXIF" -- see the comment in docs/INVENTORY.md
    const BROTLI_EXIF: &[u8] = &[
        0x0b, 0x03, 0x80, 0x00, 0x00, 0x00, 0x00, 0x45, 0x58, 0x49, 0x46, 0x03,
    ];
    let cs = lossless(16, 16, false);
    let bytes = container(|o| {
        let mut p = b"Exif".to_vec();
        p.extend(BROTLI_EXIF);
        bx(o, b"brob", &p);
        bx(o, b"jxlc", &cs);
    });
    let inv = inventory_of(&bytes);
    assert_eq!(disposition_of(&inv, b"brob", 0), Disposition::Skipped);
    assert_eq!(surfaces(&bytes), (false, false));
}

#[test]
fn decoder_rejections_are_malformed() {
    let cs = lossless(16, 16, false);
    // jxlc before ftyp
    let mut bytes = SIG.to_vec();
    bx(&mut bytes, b"jxlc", &cs);
    let inv = inventory_of(&bytes);
    assert_eq!(disposition_of(&inv, b"jxlc", 0), Disposition::Malformed);
    // second ftyp
    let bytes = container(|o| {
        o.extend(ftyp());
        bx(o, b"jxlc", &cs);
    });
    let inv = inventory_of(&bytes);
    assert_eq!(disposition_of(&inv, b"ftyp", 1), Disposition::Malformed);
    assert_eq!(disposition_of(&inv, b"jxlc", 0), Disposition::Skipped);
}

#[test]
fn bare_codestream_is_one_part_after_the_signature() {
    let cs = lossless(16, 16, false);
    let inv = inventory_of(&cs);
    let r = rows(&inv);
    assert_eq!(r.len(), 2, "{inv}");
    assert_eq!(r[0].2, 0..2);
    assert_eq!(r[0].3, Disposition::Structure);
    assert_eq!(r[1].2, 2..cs.len() as u64);
    assert_eq!(r[1].3, Disposition::ImageData);
}

// ── corpus sweep and external oracles ───────────────────────────────────────

fn corpus_files() -> Vec<PathBuf> {
    let root = std::env::var("INVENTORY_CORPUS_JXL")
        .expect("INVENTORY_CORPUS_JXL names the codec-corpus jxl directory");
    let mut out = Vec::new();
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read corpus dir") {
            let p = entry.unwrap().path();
            if p.is_dir() {
                visit(&p, out);
            } else if p.extension().is_some_and(|e| e == "jxl") {
                out.push(p);
            }
        }
    }
    visit(Path::new(&root), &mut out);
    out.sort();
    assert!(!out.is_empty(), "no .jxl files under {root}");
    out
}

/// Run by `just inventory-oracle` (and CI jobs that have the corpus).
#[test]
fn corpus_files_pass_check_inventory() {
    if std::env::var_os("INVENTORY_CORPUS_JXL").is_none() {
        return; // decided by the caller: the justfile/CI sets the variable
    }
    let files = corpus_files();
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        zencodec_testkit::check_inventory(JxlDecoderConfig::new(), &bytes)
            .unwrap_or_else(|e| panic!("{}: {e:?}", f.display()));
    }
    eprintln!("check_inventory passed on {} corpus files", files.len());
}

/// `jxlinfo -v -v` lists every box with its size; the inventory must list the
/// same boxes in the same order with the same lengths. A size field of 0 is
/// compared against the bytes to the end of the file.
#[test]
fn corpus_boxes_match_jxlinfo() {
    let Some(jxlinfo) = std::env::var_os("INVENTORY_ORACLE_JXLINFO") else {
        return; // decided by the caller
    };
    let mut compared = 0usize;
    let mut boxes = 0usize;
    for f in corpus_files() {
        let bytes = std::fs::read(&f).unwrap();
        let inv = inventory_of(&bytes);
        let out = std::process::Command::new(&jxlinfo)
            .args(["-v", "-v"])
            .arg(&f)
            .output()
            .expect("run jxlinfo");
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        let theirs: Vec<(String, u64)> = text
            .lines()
            .filter_map(|l| {
                let l = l.strip_prefix("box: type: \"")?;
                let (ty, rest) = l.split_once("\" size: ")?;
                let (size, _) = rest.split_once(',')?;
                Some((ty.to_string(), size.parse().ok()?))
            })
            .collect();
        if theirs.is_empty() {
            continue; // bare codestream: no boxes to compare
        }
        let mine: Vec<(String, u64, u64)> = inv
            .children(None)
            .into_iter()
            .map(|id| inv.get(id).unwrap())
            .filter(|p| matches!(p.kind, PartKind::Box | PartKind::Header))
            .map(|p| {
                let ty = match &p.tag {
                    PartTag::FourCc(cc) => String::from_utf8_lossy(cc).to_string(),
                    _ => "JXL ".to_string(),
                };
                (ty, p.range.end - p.range.start, p.range.end)
            })
            .collect();
        assert_eq!(
            theirs.len(),
            mine.len(),
            "{}: box count\njxlinfo: {theirs:?}\nmine: {mine:?}",
            f.display()
        );
        for ((tt, ts), (mt, ml, me)) in theirs.iter().zip(&mine) {
            assert_eq!(tt, mt, "{}: box type", f.display());
            let expect = if *ts == 0 { *ml } else { *ts };
            assert_eq!(expect, *ml, "{}: length of {tt}", f.display());
            if *ts == 0 {
                assert_eq!(
                    *me,
                    bytes.len() as u64,
                    "{}: size-0 box ends at EOF",
                    f.display()
                );
            }
        }
        compared += 1;
        boxes += theirs.len();
    }
    eprintln!("jxlinfo cross-check: {compared} containers, {boxes} boxes");
    assert!(compared >= 20, "only {compared} containers compared");
}

/// `exiftool -v3` prints `Tag 'xxxx' (N bytes)` followed by a hex dump line
/// whose offset is the first payload byte. Every box it lists must appear in
/// the inventory with that payload offset and length.
#[test]
fn corpus_boxes_match_exiftool() {
    let Some(exiftool) = std::env::var_os("INVENTORY_ORACLE_EXIFTOOL") else {
        return; // decided by the caller
    };
    let mut compared = 0usize;
    let mut listed = 0usize;
    for f in corpus_files() {
        let bytes = std::fs::read(&f).unwrap();
        let inv = inventory_of(&bytes);
        let out = std::process::Command::new(&exiftool)
            .arg("-v3")
            .arg(&f)
            .output()
            .expect("run exiftool");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let lines: Vec<&str> = text.lines().collect();
        let mut tags: Vec<(String, Option<u64>, u64)> = Vec::new(); // type, len, payload offset
        for (i, l) in lines.iter().enumerate() {
            let Some(rest) = l.strip_prefix("  | - Tag '") else {
                continue;
            };
            let Some((ty, rest)) = rest.split_once("' (") else {
                continue;
            };
            if ty.len() != 4 {
                continue;
            }
            if let Some(off) = rest.strip_prefix("offset 0x") {
                let off = off.split_whitespace().next().unwrap();
                tags.push((ty.to_string(), None, u64::from_str_radix(off, 16).unwrap()));
            } else if let Some((n, _)) = rest.split_once(" bytes)") {
                let n: u64 = n.parse().unwrap();
                let dump = lines.get(i + 1).copied().unwrap_or("");
                let off = dump
                    .trim_start_matches(|c: char| c == ' ' || c == '|')
                    .split(':')
                    .next()
                    .and_then(|h| u64::from_str_radix(h.trim(), 16).ok());
                if let Some(off) = off {
                    tags.push((ty.to_string(), Some(n), off));
                }
            }
        }
        for (ty, n, off) in &tags {
            let cc: [u8; 4] = ty.as_bytes().try_into().unwrap();
            let found = inv.parts().iter().any(|p| {
                p.tag == PartTag::FourCc(cc) && {
                    let hdr = if bytes[p.range.start as usize + 3] == 1
                        && bytes[p.range.start as usize..][..4] == [0, 0, 0, 1]
                    {
                        16
                    } else {
                        8
                    };
                    let payload = p.range.start + hdr;
                    payload == *off && n.is_none_or(|n| p.range.end - payload == n)
                }
            });
            assert!(
                found,
                "{}: exiftool lists {ty} at {off:#x} ({n:?} bytes)\n{inv}",
                f.display()
            );
        }
        listed += tags.len();
        compared += 1;
    }
    eprintln!("exiftool cross-check: {compared} files, {listed} boxes listed");
    assert!(listed >= 100, "exiftool listed only {listed} boxes");
}
