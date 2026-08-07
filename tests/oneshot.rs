//! Behavior pins for the one-shot free functions (`encode`,
//! `encode_with_fidelity`, `encode_lossless`, `decode_rgba8`).
//!
//! The two load-bearing contracts:
//!
//! 1. **Strided input is not a second-class citizen** — a `PixelSlice` with
//!    row padding must produce a codestream byte-identical to the same pixels
//!    tightly packed. The strided path row-streams through
//!    `LossyConfig::encoder()` / `LosslessConfig::encoder()` instead of
//!    repacking the whole image, so this test is what keeps the two paths
//!    honest (per the zero-divergence rule: two paths for the same operation
//!    must produce the same bytes).
//!
//! 2. **The fidelity knob is real** — `encode_with_fidelity` must produce the
//!    same bytes as hand-building the equivalent `LossyConfig` /
//!    `LosslessConfig`, for every `Fidelity` arm.
#![cfg(feature = "encode")]

use zenpixels::{PixelDescriptor, PixelSlice};

/// Deterministic non-trivial RGBA8 test image (gradients + a diagonal edge so
/// lossy actually has structure to code).
fn test_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let on_diag = (x % height == y) as u8;
            px.extend_from_slice(&[
                (x * 255 / width.max(1)) as u8,
                (y * 255 / height.max(1)) as u8,
                128u8.wrapping_add(on_diag * 100),
                255,
            ]);
        }
    }
    px
}

/// The same pixels as `test_rgba`, but with `pad` garbage bytes appended to
/// every row (stride = width*4 + pad). The padding is 0xAB on purpose: if any
/// code path reads it, the byte-identity assertions below catch it.
fn strided_copy(tight: &[u8], width: u32, height: u32, pad: usize) -> (Vec<u8>, usize) {
    let row = width as usize * 4;
    let stride = row + pad;
    let mut buf = vec![0xABu8; stride * height as usize];
    for y in 0..height as usize {
        buf[y * stride..y * stride + row].copy_from_slice(&tight[y * row..(y + 1) * row]);
    }
    (buf, stride)
}

fn tight_slice<'a>(px: &'a [u8], w: u32, h: u32) -> PixelSlice<'a> {
    PixelSlice::new(px, w, h, w as usize * 4, PixelDescriptor::RGBA8_SRGB)
        .expect("valid tight RGBA8 slice")
}

fn strided_slice<'a>(px: &'a [u8], w: u32, h: u32, stride: usize) -> PixelSlice<'a> {
    PixelSlice::new(px, w, h, stride, PixelDescriptor::RGBA8_SRGB)
        .expect("valid strided RGBA8 slice")
}

/// Contract 1a: lossy — strided input encodes byte-identical to tight input.
#[test]
fn strided_and_tight_encode_byte_identical() {
    let (w, h) = (32u32, 24u32);
    let tight = test_rgba(w, h);
    let (strided, stride) = strided_copy(&tight, w, h, 12);

    let from_tight = zenjxl::encode(tight_slice(&tight, w, h)).expect("tight encode");
    let from_strided = zenjxl::encode(strided_slice(&strided, w, h, stride)).expect("strided");
    assert_eq!(
        from_tight, from_strided,
        "strided vs tight lossy one-shot encodes diverged"
    );
}

/// Contract 1b: lossless — strided input encodes byte-identical to tight input.
#[test]
fn strided_and_tight_encode_lossless_byte_identical() {
    let (w, h) = (32u32, 24u32);
    let tight = test_rgba(w, h);
    let (strided, stride) = strided_copy(&tight, w, h, 20);

    let from_tight = zenjxl::encode_lossless(tight_slice(&tight, w, h)).expect("tight encode");
    let from_strided =
        zenjxl::encode_lossless(strided_slice(&strided, w, h, stride)).expect("strided");
    assert_eq!(
        from_tight, from_strided,
        "strided vs tight lossless one-shot encodes diverged"
    );
}

/// Contract 2a: `Fidelity::butteraugli(d)` == hand-built `LossyConfig::new(d)`,
/// and a different distance actually changes the output (the knob is wired).
#[test]
fn fidelity_butteraugli_matches_lossy_config() {
    use zencodec::encode::Fidelity;
    let (w, h) = (32u32, 24u32);
    let tight = test_rgba(w, h);

    let via_fidelity =
        zenjxl::encode_with_fidelity(tight_slice(&tight, w, h), Fidelity::butteraugli(3.0))
            .expect("fidelity encode");
    let via_config = zenjxl::LossyConfig::new(3.0)
        .encode(&tight, w, h, zenjxl::PixelLayout::Rgba8)
        .expect("config encode");
    assert_eq!(
        via_fidelity, via_config,
        "butteraugli arm diverged from LossyConfig"
    );

    let default_distance = zenjxl::encode(tight_slice(&tight, w, h)).expect("default encode");
    assert_ne!(
        via_fidelity, default_distance,
        "distance 3.0 produced the same bytes as distance 1.0 — fidelity knob inert"
    );
}

/// Contract 2b: `Fidelity::codec_quality(q)` rides the calibrated quality dial —
/// the same `calibrated_jxl_quality → quality_to_distance` chain as the
/// zencodec adapter.
#[test]
fn fidelity_codec_quality_uses_calibrated_dial() {
    use zencodec::encode::Fidelity;
    let (w, h) = (32u32, 24u32);
    let tight = test_rgba(w, h);

    let via_fidelity =
        zenjxl::encode_with_fidelity(tight_slice(&tight, w, h), Fidelity::codec_quality(60.0))
            .expect("fidelity encode");
    let distance = zenjxl::quality_to_distance(zenjxl::calibrated_jxl_quality(60.0));
    let via_config = zenjxl::LossyConfig::new(distance)
        .encode(&tight, w, h, zenjxl::PixelLayout::Rgba8)
        .expect("config encode");
    assert_eq!(
        via_fidelity, via_config,
        "codec_quality arm diverged from the dial chain"
    );
}

/// Contract 2c: `Fidelity::Lossless` == `encode_lossless`, and round-trips
/// bit-exactly (strided input included).
#[cfg(feature = "decode")]
#[test]
fn fidelity_lossless_roundtrips_exactly() {
    use zencodec::encode::Fidelity;
    let (w, h) = (32u32, 24u32);
    let tight = test_rgba(w, h);
    let (strided, stride) = strided_copy(&tight, w, h, 8);

    let via_fidelity =
        zenjxl::encode_with_fidelity(strided_slice(&strided, w, h, stride), Fidelity::Lossless)
            .expect("fidelity lossless encode");
    let via_lossless = zenjxl::encode_lossless(tight_slice(&tight, w, h)).expect("lossless encode");
    assert_eq!(
        via_fidelity, via_lossless,
        "Fidelity::Lossless diverged from encode_lossless"
    );

    let (rgba, dw, dh) = zenjxl::decode_rgba8(&via_fidelity).expect("decode");
    assert_eq!((dw, dh), (w, h));
    assert_eq!(rgba, tight, "lossless round-trip not bit-exact");
}
