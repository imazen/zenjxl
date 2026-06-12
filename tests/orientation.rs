//! zencodec adapter orientation handling for JPEG XL.
//!
//! Verifies that the `JxlDecodeJob` adapter honors `OrientationHint` by routing
//! it into the decoder's `adjust_orientation` (the native intrinsic bake) plus,
//! for the transform hints, a pixel-exact post-decode transform — and that
//! `probe`, `output_info`, and `decode` all agree under each hint.
//!
//! The pixel-sacredness oracle (`correct_equals_preserve_*`) is the core check:
//! the baked (Correct) output must equal the un-baked (Preserve) output remapped
//! through the stored orientation, **bit for bit** — no resampling, no
//! off-by-one, no color drift. Modeled on the decoder's own
//! `correct_equals_preserve_under_exact_orientation_transform`.
//!
//! Fixture: `tests/fixtures/orientation5_transpose.jxl` — a 52-byte JXL whose
//! codestream carries orientation 5 (Transpose, a *transposing* orientation, so
//! display dims = coded dims with width/height swapped). Copied verbatim from
//! the decoder's committed seed corpus
//! (`zenjxl-decoder/fuzz/seed_corpus/decode/orientation5_transpose.jxl`); the
//! JXL encoder has no public API to write a non-Identity codestream orientation,
//! so a committed fixture is the only way to exercise this path.

#![cfg(all(feature = "decode", feature = "zencodec"))]

use std::borrow::Cow;
use std::path::PathBuf;

use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::{Orientation, OrientationHint};
use zenjxl::JxlDecoderConfig;

/// EXIF orientation the fixture carries (5 = Transpose).
const FIXTURE_EXIF: u8 = 5;

fn transpose_fixture() -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/orientation5_transpose.jxl");
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("missing committed fixture {}: {e}", path.display()))
}

/// Decode the fixture through the adapter with the given hint, returning the
/// `DecodeOutput`.
fn decode_with_hint(hint: OrientationHint) -> zencodec::decode::DecodeOutput {
    let data = transpose_fixture();
    JxlDecoderConfig::new()
        .job()
        .with_orientation(hint)
        .decoder(Cow::Owned(data), &[])
        .expect("build decoder")
        .decode()
        .expect("decode")
}

/// Map a destination pixel `(dx, dy)` (in the output of `orientation` applied to
/// a `src`-sized buffer) back to the source pixel it came from. Built
/// independently of the production `apply_orientation` via the zenpixels
/// `forward_map`, so the oracle does not merely re-run the implementation.
///
/// `src` is the source `(w, h)` (pre-orientation) geometry.
fn source_pixel_for_dest(
    orientation: Orientation,
    dest: (u32, u32),
    src: (u32, u32),
) -> (u32, u32) {
    // forward_map sends a source pixel to its destination; invert by scanning is
    // O(N), but for the small fixture we instead invert analytically per the
    // group. Easiest robust route: build the forward map and look the dest up.
    // The fixture is tiny, so an O(N) inverse is fine and keeps this independent
    // of any hand-derived inverse table.
    let (w, h) = src;
    for sy in 0..h {
        for sx in 0..w {
            if orientation.forward_map(sx, sy, w, h) == dest {
                return (sx, sy);
            }
        }
    }
    panic!("no source maps to dest {dest:?} for {orientation:?} on {src:?}");
}

/// Assert that `transformed` equals `base` remapped through `orientation`, bit
/// for bit. Reads both buffers row-by-row via `PixelSlice::row`, so it is
/// correct for strided buffers (per the strided-rows discipline) and does not
/// assume a contiguous backing.
fn assert_equals_under_orientation(
    base: &zencodec::decode::DecodeOutput,
    transformed: &zencodec::decode::DecodeOutput,
    orientation: Orientation,
) {
    let channels = channels_of(transformed);
    assert_eq!(channels, channels_of(base), "channel count must match");
    let (bw, bh) = (base.width(), base.height());
    let (tw, th) = (transformed.width(), transformed.height());
    // Geometry must be the orientation's source->dest size mapping.
    assert_eq!(
        (tw, th),
        orientation.output_dimensions(bw, bh),
        "transformed geometry must be the oriented base geometry"
    );
    let base_px = base.pixels();
    let tr_px = transformed.pixels();
    let mut mismatches = 0usize;
    for dy in 0..th {
        let tr_row = tr_px.row(dy);
        for dx in 0..tw {
            let (sx, sy) = source_pixel_for_dest(orientation, (dx, dy), (bw, bh));
            let base_row = base_px.row(sy);
            let t = &tr_row[dx as usize * channels..][..channels];
            let b = &base_row[sx as usize * channels..][..channels];
            if t != b {
                mismatches += 1;
                if mismatches <= 4 {
                    eprintln!(
                        "mismatch at dest ({dx},{dy}) <- src ({sx},{sy}): \
                         transformed {t:?} vs base {b:?}"
                    );
                }
            }
        }
    }
    assert_eq!(
        mismatches, 0,
        "transformed output must equal base mapped through {orientation:?}, \
         bit-for-bit ({mismatches} mismatched pixels)"
    );
}

/// Assert two decode outputs have identical pixels (same geometry + bytes),
/// row-by-row (strided-safe).
fn assert_pixels_identical(
    a: &zencodec::decode::DecodeOutput,
    b: &zencodec::decode::DecodeOutput,
    msg: &str,
) {
    assert_eq!(
        (a.width(), a.height()),
        (b.width(), b.height()),
        "{msg}: geometry"
    );
    let channels = channels_of(a);
    assert_eq!(channels, channels_of(b), "{msg}: channels");
    let (ap, bp) = (a.pixels(), b.pixels());
    let row_bytes = a.width() as usize * channels;
    for y in 0..a.height() {
        assert_eq!(
            &ap.row(y)[..row_bytes],
            &bp.row(y)[..row_bytes],
            "{msg}: row {y} differs"
        );
    }
}

fn channels_of(out: &zencodec::decode::DecodeOutput) -> usize {
    out.descriptor().channels()
}

// ── Preserve ─────────────────────────────────────────────────────────────

#[test]
fn preserve_reports_coded_dims_and_intrinsic_tag() {
    let data = transpose_fixture();
    // probe
    let info = JxlDecoderConfig::new()
        .job()
        .with_orientation(OrientationHint::Preserve)
        .probe(&data)
        .expect("probe");
    assert_eq!(
        info.orientation,
        Orientation::from_exif(FIXTURE_EXIF).unwrap(),
        "Preserve must surface the intrinsic (Transpose) orientation"
    );
    // decode agrees with probe
    let out = decode_with_hint(OrientationHint::Preserve);
    assert_eq!(
        (out.width(), out.height()),
        (info.width, info.height),
        "decode geometry must match probe under Preserve"
    );
    assert_eq!(
        out.info().orientation,
        info.orientation,
        "decode orientation tag must match probe under Preserve"
    );
}

#[test]
fn preserve_output_info_matches() {
    let data = transpose_fixture();
    let job = JxlDecoderConfig::new()
        .job()
        .with_orientation(OrientationHint::Preserve);
    let oi = job.output_info(&data).expect("output_info");
    let out = decode_with_hint(OrientationHint::Preserve);
    assert_eq!(
        (oi.width, oi.height),
        (out.width(), out.height()),
        "output_info geometry must match the decoded buffer (Preserve)"
    );
    assert_eq!(
        oi.orientation_applied,
        Orientation::Identity,
        "Preserve applies nothing — orientation_applied is Identity"
    );
}

// ── Correct ──────────────────────────────────────────────────────────────

#[test]
fn correct_reports_display_dims_and_identity() {
    let data = transpose_fixture();
    let info = JxlDecoderConfig::new()
        .job()
        .with_orientation(OrientationHint::Correct)
        .probe(&data)
        .expect("probe");
    assert_eq!(
        info.orientation,
        Orientation::Identity,
        "Correct bakes the orientation — residual must be Identity"
    );

    // Display dims are the coded dims transposed (the fixture is Transpose).
    let preserve = JxlDecoderConfig::new()
        .job()
        .with_orientation(OrientationHint::Preserve)
        .probe(&data)
        .expect("probe preserve");
    assert_eq!(
        (info.width, info.height),
        Orientation::Transpose.output_dimensions(preserve.width, preserve.height),
        "Correct must report display (transposed) dimensions"
    );

    let out = decode_with_hint(OrientationHint::Correct);
    assert_eq!(
        (out.width(), out.height()),
        (info.width, info.height),
        "decode geometry must match probe under Correct"
    );
    assert_eq!(out.info().orientation, Orientation::Identity);
}

#[test]
fn correct_output_info_reports_intrinsic_applied() {
    let data = transpose_fixture();
    let job = JxlDecoderConfig::new()
        .job()
        .with_orientation(OrientationHint::Correct);
    let oi = job.output_info(&data).expect("output_info");
    let out = decode_with_hint(OrientationHint::Correct);
    assert_eq!(
        (oi.width, oi.height),
        (out.width(), out.height()),
        "output_info geometry must match the decoded buffer (Correct)"
    );
    assert_eq!(
        oi.orientation_applied,
        Orientation::from_exif(FIXTURE_EXIF).unwrap(),
        "Correct applies the intrinsic orientation"
    );
}

// ── Pixel oracle: Correct == Preserve remapped through the stored orientation ──

#[test]
fn correct_equals_preserve_under_exact_orientation_transform() {
    let preserve = decode_with_hint(OrientationHint::Preserve);
    let correct = decode_with_hint(OrientationHint::Correct);

    assert_eq!(
        channels_of(&preserve),
        channels_of(&correct),
        "same channel layout, only geometry differs"
    );

    let intrinsic = preserve.info().orientation;
    assert_ne!(
        intrinsic,
        Orientation::Identity,
        "this oracle only proves anything for a non-Identity orientation"
    );

    // The baked (Correct) buffer must be the un-baked (Preserve) buffer with the
    // intrinsic orientation applied — bit for bit.
    assert_equals_under_orientation(&preserve, &correct, intrinsic);
}

// ── ExactTransform ─────────────────────────────────────────────────────────

#[test]
fn exact_transform_identity_equals_preserve() {
    // ExactTransform(Identity) ignores EXIF and applies nothing → identical to
    // Preserve (stored pixels, stored dims). The reported tag becomes Identity
    // because the pixels are declared final.
    let preserve = decode_with_hint(OrientationHint::Preserve);
    let exact_id = decode_with_hint(OrientationHint::ExactTransform(Orientation::Identity));

    assert_eq!(
        (exact_id.width(), exact_id.height()),
        (preserve.width(), preserve.height()),
        "ExactTransform(Identity) keeps stored geometry"
    );
    assert_eq!(
        exact_id.info().orientation,
        Orientation::Identity,
        "ExactTransform declares the pixels final → Identity tag"
    );
    assert_pixels_identical(&exact_id, &preserve, "ExactTransform(Identity) vs Preserve");
}

#[test]
fn exact_transform_intrinsic_equals_correct() {
    // The fixture's intrinsic orientation is Transpose. ExactTransform(Transpose)
    // applies Transpose to the *stored* pixels, ignoring EXIF — which lands on
    // exactly the same pixels as Correct (which bakes the intrinsic Transpose).
    let correct = decode_with_hint(OrientationHint::Correct);
    let intrinsic = Orientation::from_exif(FIXTURE_EXIF).unwrap();
    let exact = decode_with_hint(OrientationHint::ExactTransform(intrinsic));

    assert_eq!(
        (exact.width(), exact.height()),
        (correct.width(), correct.height()),
        "ExactTransform(intrinsic) geometry must match Correct"
    );
    assert_eq!(exact.info().orientation, Orientation::Identity);
    assert_pixels_identical(&exact, &correct, "ExactTransform(intrinsic) vs Correct");
}

#[test]
fn exact_transform_output_info_matches_decode() {
    let data = transpose_fixture();
    let t = Orientation::Rotate90;
    let job = JxlDecoderConfig::new()
        .job()
        .with_orientation(OrientationHint::ExactTransform(t));
    let oi = job.output_info(&data).expect("output_info");
    let out = decode_with_hint(OrientationHint::ExactTransform(t));
    assert_eq!(
        (oi.width, oi.height),
        (out.width(), out.height()),
        "output_info geometry must match decode for ExactTransform"
    );
    assert_eq!(
        oi.orientation_applied, t,
        "ExactTransform applies exactly t (EXIF ignored)"
    );
}

// ── CorrectAndTransform ────────────────────────────────────────────────────

#[test]
fn correct_and_transform_composes_on_correct() {
    // CorrectAndTransform(t) = correct the intrinsic (→ upright), then apply t.
    // So its output must equal Correct's output with t applied on top, bit for
    // bit. Pick t = Rotate90 (axis-swapping) to exercise dimension swaps.
    let t = Orientation::Rotate90;
    let correct = decode_with_hint(OrientationHint::Correct);
    let cat = decode_with_hint(OrientationHint::CorrectAndTransform(t));

    assert_eq!(
        cat.info().orientation,
        Orientation::Identity,
        "CorrectAndTransform declares the pixels final → Identity tag"
    );
    assert_equals_under_orientation(&correct, &cat, t);
}

#[test]
fn correct_and_transform_output_info_reports_composed() {
    let data = transpose_fixture();
    let t = Orientation::Rotate90;
    let job = JxlDecoderConfig::new()
        .job()
        .with_orientation(OrientationHint::CorrectAndTransform(t));
    let oi = job.output_info(&data).expect("output_info");
    let out = decode_with_hint(OrientationHint::CorrectAndTransform(t));
    assert_eq!(
        (oi.width, oi.height),
        (out.width(), out.height()),
        "output_info geometry must match decode for CorrectAndTransform"
    );
    let intrinsic = Orientation::from_exif(FIXTURE_EXIF).unwrap();
    assert_eq!(
        oi.orientation_applied,
        intrinsic.then(t),
        "CorrectAndTransform applies intrinsic then t"
    );
}

// ── Default hint is Preserve ───────────────────────────────────────────────

#[test]
fn default_hint_is_preserve() {
    // Without any with_orientation call, the adapter must behave as Preserve
    // (the zencodec ecosystem default): stored dims + intrinsic tag.
    let data = transpose_fixture();
    let default_info = JxlDecoderConfig::new().job().probe(&data).expect("probe");
    let preserve_info = JxlDecoderConfig::new()
        .job()
        .with_orientation(OrientationHint::Preserve)
        .probe(&data)
        .expect("probe");
    assert_eq!(default_info.width, preserve_info.width);
    assert_eq!(default_info.height, preserve_info.height);
    assert_eq!(default_info.orientation, preserve_info.orientation);
    assert_eq!(
        default_info.orientation,
        Orientation::from_exif(FIXTURE_EXIF).unwrap(),
        "default (Preserve) must surface the intrinsic orientation"
    );
}
