// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! Smoke test for the `__expert` feature.
//!
//! Verifies that zenjxl re-exports the segmented internal-params types
//! (`LossyInternalParams`, `LosslessInternalParams`) and that
//! `LossyConfig::with_internal_params` / `LosslessConfig::with_internal_params`
//! actually propagate overrides through to the produced bitstreams.
//!
//! The exhaustive per-knob coverage lives in jxl-encoder's
//! `effort_expert_tests`; this test only confirms the forwarding wiring is
//! intact at the zenjxl boundary after the segmentation refactor
//! (jxl-encoder feat/expert-internal-params).

#![cfg(all(test, feature = "__expert"))]

use zenjxl::{
    JxlValidationError, LosslessConfig, LosslessInternalParams, LossyConfig, LossyInternalParams,
    PixelLayout, ValidationError,
};

const W: u32 = 96;
const H: u32 = 96;

/// Small synthetic 96×96 RGB8 image with mixed content so AC-strategy
/// search has block-size choices to make (i.e. `try_dct16`/`try_dct32`
/// decisions actually matter).
fn synthetic_rgb8() -> Vec<u8> {
    let mut out = Vec::with_capacity((W * H * 3) as usize);
    let mut state: u32 = 0x1357_9BDF;
    for y in 0..H {
        for x in 0..W {
            // Top half: smooth diagonal gradient — large-DCT friendly.
            // Bottom half: bars + speckle — small-DCT / IDENTITY friendly.
            let (r, g, b) = if y < H / 2 {
                let v = ((x + y) * 255 / (W + H / 2 - 2)) as u8;
                (v, v.wrapping_add(20), v.wrapping_sub(20))
            } else {
                let bars_g = if (x / 4) % 2 == 0 { 30 } else { 220 };
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let speckle = ((state >> 24) as u8) & 0x3F;
                let bx = ((x as u8) ^ 0x55).wrapping_add(speckle) | 0x10;
                ((x as u8) ^ 0x55, bars_g as u8, bx)
            };
            out.extend_from_slice(&[r, g, b]);
        }
    }
    out
}

fn baseline_lossy() -> LossyConfig {
    LossyConfig::new(1.5).with_effort(7).with_threads(1)
}

fn encode_lossy(cfg: &LossyConfig, pixels: &[u8]) -> Vec<u8> {
    cfg.clone()
        .encode(pixels, W, H, PixelLayout::Rgb8)
        .expect("lossy encode")
}

fn baseline_lossless() -> LosslessConfig {
    LosslessConfig::new().with_effort(7).with_threads(1)
}

fn encode_lossless(cfg: &LosslessConfig, pixels: &[u8]) -> Vec<u8> {
    cfg.clone()
        .encode(pixels, W, H, PixelLayout::Rgb8)
        .expect("lossless encode")
}

/// Build a custom `LossyInternalParams` via zenjxl's re-exports, set fields
/// known to affect lossy bytes (`try_dct16` + `try_dct32`, both override-
/// effective per jxl-encoder's `effort_expert_tests::lossy_override_try_dct16`),
/// apply via `LossyConfig::with_internal_params`, and confirm the produced
/// bitstream differs from the baseline.
#[test]
fn lossy_expert_override_propagates_through_zenjxl() {
    let pixels = synthetic_rgb8();

    let mut params = LossyInternalParams::default();
    // e7 default = both true. Disabling forces no DCT16x16 / DCT16x8 /
    // DCT32x32 etc. merges, which definitively changes the bitstream.
    params.try_dct16 = Some(false);
    params.try_dct32 = Some(false);

    let cfg_override = baseline_lossy().with_internal_params(params);
    let bytes_override = encode_lossy(&cfg_override, &pixels);
    let bytes_baseline = encode_lossy(&baseline_lossy(), &pixels);

    // Both must be valid JXL bitstreams.
    assert_eq!(&bytes_override[..2], &[0xFF, 0x0A]);
    assert_eq!(&bytes_baseline[..2], &[0xFF, 0x0A]);

    assert_ne!(
        bytes_override, bytes_baseline,
        "LossyInternalParams override (try_dct16=Some(false), try_dct32=Some(false)) must \
         change the bitstream when applied through zenjxl's re-exported \
         LossyConfig::with_internal_params"
    );
}

/// Build a custom `LosslessInternalParams` via zenjxl's re-exports, override
/// `nb_rcts_to_try` (e7 default = 7; forcing 0 skips the RCT search entirely
/// and falls back to the unconditional YCoCg pick, which definitively changes
/// the bitstream — matches jxl-encoder's
/// `effort_expert_tests::lossless_override_nb_rcts_to_try`), and confirm the
/// override propagates.
#[test]
fn lossless_expert_override_propagates_through_zenjxl() {
    let pixels = synthetic_rgb8();

    let mut params = LosslessInternalParams::default();
    params.nb_rcts_to_try = Some(0);

    let cfg_override = baseline_lossless().with_internal_params(params);
    let bytes_override = encode_lossless(&cfg_override, &pixels);
    let bytes_baseline = encode_lossless(&baseline_lossless(), &pixels);

    // Both must be valid JXL bitstreams.
    assert_eq!(&bytes_override[..2], &[0xFF, 0x0A]);
    assert_eq!(&bytes_baseline[..2], &[0xFF, 0x0A]);

    assert_ne!(
        bytes_override, bytes_baseline,
        "LosslessInternalParams override (nb_rcts_to_try=Some(0)) must change the bitstream \
         when applied through zenjxl's re-exported LosslessConfig::with_internal_params"
    );
}

/// Verify that a `LossyConfig::validate()` error from upstream jxl-encoder
/// propagates through zenjxl's `ValidationError::JxlEncoder` via the
/// `From<jxl_encoder::ValidationError>` impl, so callers can `?`-bubble
/// upstream validation errors into zenjxl's own error type.
///
/// `LossyConfig::new(0.0)` triggers `DistanceOutOfRange` upstream (lossy
/// distance must be > 0; lossless work goes through `LosslessConfig`).
#[test]
fn jxl_validation_error_propagates() {
    let cfg = LossyConfig::new(0.0);
    let upstream_err = cfg
        .validate()
        .expect_err("distance=0.0 must be rejected upstream");
    assert!(
        matches!(upstream_err, JxlValidationError::DistanceOutOfRange { .. }),
        "expected upstream DistanceOutOfRange, got {upstream_err:?}"
    );

    // The `?`-bubble path: From<jxl_encoder::ValidationError> for
    // zenjxl::ValidationError lands as ValidationError::JxlEncoder(_).
    let zen_err: ValidationError = LossyConfig::new(0.0).validate().unwrap_err().into();
    assert!(
        matches!(zen_err, ValidationError::JxlEncoder(_)),
        "expected ValidationError::JxlEncoder, got {zen_err:?}"
    );
}
