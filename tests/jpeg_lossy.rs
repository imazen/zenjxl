//! Integration tests for the lossy JPEG → JXL recompression closed loop
//! (`zenjxl::jpeg_lossy`). Proves the in-process encode → decode → score loop:
//! zenjxl can coarsen a JPEG (jxl-encoder PreserveJxl), decode the result
//! (zenjxl-decoder), and drive a quality target with a caller-supplied scorer.
//!
//! Run: cargo test -p zenjxl --features jpeg-lossy --test jpeg_lossy
#![cfg(feature = "jpeg-lossy")]

use zenjxl::jpeg_lossy::{
    InferredMetric, JpegRecompressMethod, QualityTarget, predict_inferred_floor,
    recompress_jpeg_coarsen, recompress_jpeg_lossy, recompress_jpeg_lossy_relative,
    recompress_jpeg_lossy_target,
};

// A tiny real-photo baseline JPEG (96x96, 3-component, ~3.8 KB).
const TINY_JPEG: &[u8] = include_bytes!("fixtures/tiny.jpg");

/// Mean squared error over tightly-packed RGB8 (lower = better quality).
fn mse(a: &[u8], b: &[u8], _w: u32, _h: u32) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return f32::MAX;
    }
    let mut s = 0f64;
    for i in 0..n {
        let d = a[i] as f64 - b[i] as f64;
        s += d * d;
    }
    (s / n as f64) as f32
}

/// Decode a bare codestream to RGB8 + dims via the public decode API.
fn decode_dims(cs: &[u8]) -> (u32, u32) {
    let out = zenjxl::decode(cs, None, &[zenpixels::PixelDescriptor::RGB8])
        .expect("decode recompressed output");
    (out.info.width, out.info.height)
}

/// Decode a JXL codestream and return its embedded ICC profile (if any).
fn decode_icc(cs: &[u8]) -> Option<Vec<u8>> {
    let out = zenjxl::decode(cs, None, &[zenpixels::PixelDescriptor::RGB8])
        .expect("decode recompressed output");
    out.info.icc_profile
}

/// Build a real Display-P3 ICC profile (a non-sRGB, wide-gamut profile) via
/// moxcms — the same color library the `jpeg-lossy` feature already pulls.
fn display_p3_icc() -> Vec<u8> {
    moxcms::ColorProfile::new_display_p3()
        .encode()
        .expect("encode Display-P3 ICC")
}

/// Splice an ICC profile into a baseline JPEG as APP2 `ICC_PROFILE` marker
/// chunk(s), inserted right after SOI (the format `zenjpeg::extract_icc_profile`
/// reads back: 12-byte `ICC_PROFILE\0` signature + 1-based chunk index + total
/// chunk count + chunk bytes). `src` must start with SOI (`FF D8`).
fn inject_icc(src: &[u8], icc: &[u8]) -> Vec<u8> {
    assert_eq!(&src[..2], &[0xFF, 0xD8], "source must start with SOI");
    // APP2 segment length is a 16-bit field that counts itself; reserve room for
    // the length bytes (2) + signature (12) + chunk index (1) + total (1) = 16.
    const MAX_CHUNK_DATA: usize = 0xFFFF - 2 - 14;
    let chunks: Vec<&[u8]> = if icc.is_empty() {
        Vec::new()
    } else {
        icc.chunks(MAX_CHUNK_DATA).collect()
    };
    let total = chunks.len() as u8;

    let mut out = Vec::with_capacity(src.len() + icc.len() + chunks.len() * 18 + 4);
    out.extend_from_slice(&src[..2]); // SOI
    for (i, chunk) in chunks.iter().enumerate() {
        let seg_len = (2 + 14 + chunk.len()) as u16; // length field counts itself
        out.extend_from_slice(&[0xFF, 0xE2]);
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(b"ICC_PROFILE\0");
        out.push((i + 1) as u8); // chunk index is 1-based
        out.push(total);
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&src[2..]); // rest of the JPEG (DQT/SOF/.../SOS/scan)
    out
}

#[test]
fn coarsen_is_monotone_and_decodes() {
    let lossless = recompress_jpeg_coarsen(TINY_JPEG, 1.0, 5).expect("scale 1.0");
    let coarse = recompress_jpeg_coarsen(TINY_JPEG, 3.0, 5).expect("scale 3.0");
    // both decode to the source dimensions
    assert_eq!(decode_dims(&lossless), (96, 96));
    assert_eq!(decode_dims(&coarse), (96, 96));
    // coarsening shrinks the codestream
    assert!(
        coarse.len() < lossless.len(),
        "scale 3.0 ({}) must be smaller than lossless ({})",
        coarse.len(),
        lossless.len()
    );
}

#[test]
fn relative_loop_looser_target_is_smaller() {
    // MSE: lower is better, so higher_is_better = false.
    // Loose target (MSE <= 300) allows more coarsening than strict (MSE <= 30).
    let strict =
        recompress_jpeg_lossy_relative(TINY_JPEG, 30.0, false, &mse, 5).expect("strict target");
    let loose =
        recompress_jpeg_lossy_relative(TINY_JPEG, 300.0, false, &mse, 5).expect("loose target");
    assert_eq!(decode_dims(&strict), (96, 96));
    assert_eq!(decode_dims(&loose), (96, 96));
    assert!(
        loose.len() <= strict.len(),
        "looser target ({}) must be <= stricter target ({})",
        loose.len(),
        strict.len()
    );
}

#[test]
fn reencode_path_decodes_and_is_monotone() {
    // The pixel re-encode (VarDCT) path: looser MSE target -> <= stricter bytes,
    // and the output decodes to the source dimensions.
    let strict = recompress_jpeg_lossy(
        TINY_JPEG,
        JpegRecompressMethod::Reencode,
        30.0,
        false,
        &mse,
        5,
    )
    .expect("reencode strict");
    let loose = recompress_jpeg_lossy(
        TINY_JPEG,
        JpegRecompressMethod::Reencode,
        300.0,
        false,
        &mse,
        5,
    )
    .expect("reencode loose");
    assert_eq!(decode_dims(&strict), (96, 96));
    assert_eq!(decode_dims(&loose), (96, 96));
    assert!(
        loose.len() <= strict.len(),
        "reencode: looser ({}) must be <= stricter ({})",
        loose.len(),
        strict.len()
    );
}

#[test]
fn auto_router_picks_the_smaller_path() {
    // Auto = min(Coarsen, Reencode) at the same target. It must be no larger
    // than either single path, and decode to the source dimensions.
    let t = 120.0;
    let coarsen =
        recompress_jpeg_lossy(TINY_JPEG, JpegRecompressMethod::Coarsen, t, false, &mse, 5)
            .expect("coarsen");
    let reencode =
        recompress_jpeg_lossy(TINY_JPEG, JpegRecompressMethod::Reencode, t, false, &mse, 5)
            .expect("reencode");
    let auto = recompress_jpeg_lossy(TINY_JPEG, JpegRecompressMethod::Auto, t, false, &mse, 5)
        .expect("auto");
    assert_eq!(decode_dims(&auto), (96, 96));
    assert!(
        auto.len() <= coarsen.len() && auto.len() <= reencode.len(),
        "auto ({}) must be <= coarsen ({}) and reencode ({})",
        auto.len(),
        coarsen.len(),
        reencode.len()
    );
    assert!(auto.len() == coarsen.len() || auto.len() == reencode.len());
}

#[test]
fn inferred_floor_predictor_reads_source_quality() {
    // tiny.jpg was encoded at Q85; the IJG floor predictor should return a
    // zensim floor between the Q82 (76.5) and Q92 (88.2) table rows.
    let floor = predict_inferred_floor(TINY_JPEG, InferredMetric::ZensimA)
        .expect("IJG-scale source quality should be readable");
    assert!(
        (70.0..=90.0).contains(&floor),
        "Q85 zensim floor should sit between the table rows, got {floor}"
    );
}

#[test]
fn inferred_unreachable_target_clamps_to_lossless() {
    // Ask for an absolute quality BETTER than the source floor -> unreachable
    // (can't recover discarded detail) -> ships the lossless transcode.
    let floor = predict_inferred_floor(TINY_JPEG, InferredMetric::ZensimA).unwrap();
    let target = QualityTarget::Inferred {
        abs_level: floor + 8.0, // above floor (zensim higher = better) => unreachable
        floor,
        relative_target: 50.0,
        higher_is_better: true,
    };
    let out = recompress_jpeg_lossy_target(TINY_JPEG, JpegRecompressMethod::Auto, target, &mse, 5)
        .expect("inferred clamp");
    let lossless = recompress_jpeg_coarsen(TINY_JPEG, 1.0, 5).expect("lossless");
    assert_eq!(
        out.len(),
        lossless.len(),
        "unreachable abs target -> lossless floor"
    );
    assert_eq!(decode_dims(&out), (96, 96));
}

#[test]
fn inferred_preliminary_builds_and_runs() {
    // The preliminary constructor wires detect -> floor -> relative_target.
    // A reachable target (well below floor) should produce a decodable output.
    let target = QualityTarget::inferred_preliminary(TINY_JPEG, InferredMetric::ZensimA, 50.0)
        .expect("preliminary inferred target from IJG source");
    let out =
        recompress_jpeg_lossy_target(TINY_JPEG, JpegRecompressMethod::Coarsen, target, &mse, 5)
            .expect("inferred reachable");
    assert_eq!(decode_dims(&out), (96, 96));
}

#[test]
fn unreachable_target_returns_lossless_floor() {
    // An impossible target (MSE <= 0 = pixel-exact) can't be met by coarsening;
    // the loop must fall back to the lossless transcode (the floor), not error.
    let out = recompress_jpeg_lossy_relative(TINY_JPEG, 0.0, false, &mse, 5)
        .expect("unreachable target falls back to lossless");
    let lossless = recompress_jpeg_coarsen(TINY_JPEG, 1.0, 5).expect("lossless");
    assert_eq!(out.len(), lossless.len(), "unreachable -> lossless floor");
    assert_eq!(decode_dims(&out), (96, 96));
}

// ── Color preservation: the source JPEG's ICC must survive recompression ──

/// Sanity check on the fixtures: the Display-P3 profile is a real, non-sRGB
/// profile and the injected tag actually changes the decoded color. Without a
/// source ICC, JXL signals enum-sRGB (the decoder reconstructs a synthesized
/// sRGB ICC) — distinct from the injected P3 — so the tagged/untagged tests
/// below genuinely distinguish "preserved P3" from "defaulted sRGB".
#[test]
fn icc_fixture_changes_decoded_color() {
    let p3 = display_p3_icc();
    assert!(p3.len() > 100, "Display-P3 ICC should be a real profile");
    let tagged = inject_icc(TINY_JPEG, &p3);
    // Both decode/recompress to the source dimensions.
    let tagged_cs = recompress_jpeg_coarsen(&tagged, 1.0, 5).expect("tagged scale 1.0");
    let bare_cs = recompress_jpeg_coarsen(TINY_JPEG, 1.0, 5).expect("bare scale 1.0");
    assert_eq!(decode_dims(&tagged_cs), (96, 96));
    assert_eq!(decode_dims(&bare_cs), (96, 96));
    // The tagged output carries the P3 profile; the untagged one does not.
    assert_eq!(decode_icc(&tagged_cs).as_deref(), Some(p3.as_slice()));
    assert_ne!(
        decode_icc(&bare_cs).as_deref(),
        Some(p3.as_slice()),
        "an untagged JPEG must not decode as Display-P3"
    );
}

/// REGRESSION (the path that already worked): the coefficient-domain Coarsen
/// path lifts the source's APP2 ICC into the JXL codestream. Recompress a
/// Display-P3-tagged JPEG via Coarsen and confirm the profile survives.
#[test]
fn coarsen_preserves_source_icc() {
    let p3 = display_p3_icc();
    let tagged = inject_icc(TINY_JPEG, &p3);
    let cs = recompress_jpeg_coarsen(&tagged, 1.0, 5).expect("coarsen tagged");
    let got = decode_icc(&cs).expect("coarsen output must carry the source ICC");
    assert_eq!(
        got, p3,
        "Coarsen must preserve the source Display-P3 ICC byte-for-byte"
    );
}

/// THE BUG FIX: the pixel Reencode path must also carry the source's ICC. Force
/// the Reencode path on a Display-P3-tagged JPEG and confirm the profile is in
/// the output JXL (previously it was a bare codestream → silently relabeled sRGB).
#[test]
fn reencode_preserves_source_icc() {
    let p3 = display_p3_icc();
    let tagged = inject_icc(TINY_JPEG, &p3);
    // Reencode path, explicitly (a loose MSE target so the loop coarsens freely).
    let cs = recompress_jpeg_lossy(
        &tagged,
        JpegRecompressMethod::Reencode,
        300.0,
        false,
        &mse,
        5,
    )
    .expect("reencode tagged");
    assert_eq!(decode_dims(&cs), (96, 96));
    let got = decode_icc(&cs).expect("Reencode output must carry the source ICC");
    assert_eq!(
        got, p3,
        "Reencode must preserve the source Display-P3 ICC byte-for-byte"
    );
}

/// The Reencode path on an untagged JPEG must stay sRGB — not gain the wrong
/// (e.g. P3) profile. An untagged JPEG is sRGB by convention, and JXL's enum
/// color already signals sRGB (jxl-rs reconstructs a synthesized sRGB ICC on
/// decode). The untagged Reencode output must therefore match the untagged
/// Coarsen default sRGB and never the injected P3 — guarding the no-source-ICC
/// branch of the fix (where the bare convenience encode is still correct).
#[test]
fn reencode_untagged_jpeg_stays_srgb() {
    let p3 = display_p3_icc();
    let reencode = recompress_jpeg_lossy(
        TINY_JPEG,
        JpegRecompressMethod::Reencode,
        300.0,
        false,
        &mse,
        5,
    )
    .expect("reencode untagged");
    assert_eq!(decode_dims(&reencode), (96, 96));
    // Not relabeled to the wide-gamut profile.
    assert_ne!(
        decode_icc(&reencode).as_deref(),
        Some(p3.as_slice()),
        "untagged source must not be relabeled Display-P3"
    );
    // Same default sRGB the Coarsen path emits for an untagged source.
    let coarsen = recompress_jpeg_coarsen(TINY_JPEG, 1.0, 5).expect("coarsen untagged");
    assert_eq!(
        decode_icc(&reencode),
        decode_icc(&coarsen),
        "untagged Reencode must signal the same sRGB as untagged Coarsen"
    );
}
