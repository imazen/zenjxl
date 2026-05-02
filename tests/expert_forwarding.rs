// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! Smoke test for the `__expert` feature.
//!
//! Verifies that zenjxl re-exports the expert escape-hatch types
//! (`EffortProfile`, `EncoderMode`) and that `LossyConfig::with_effort_profile_override`
//! actually propagates an override through to the produced bitstream.
//!
//! The exhaustive per-knob coverage lives in jxl-encoder's
//! `effort_expert_tests`; this test only confirms the forwarding wiring is
//! intact at the zenjxl boundary.

#![cfg(all(test, feature = "__expert"))]

use zenjxl::{EffortProfile, EncoderMode, LossyConfig, PixelLayout};

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

fn encode(cfg: &LossyConfig, pixels: &[u8]) -> Vec<u8> {
    cfg.clone()
        .encode(pixels, W, H, PixelLayout::Rgb8)
        .expect("lossy encode")
}

/// Build a custom `EffortProfile` via zenjxl's re-exports, mutate a field
/// known to affect lossy bytes (`try_dct16` + `try_dct32`, both override-
/// effective per jxl-encoder's `effort_expert_tests::lossy_override_try_dct16`),
/// apply via `LossyConfig::with_effort_profile_override`, and confirm the
/// produced bitstream differs from the baseline.
#[test]
fn expert_override_propagates_through_zenjxl() {
    let pixels = synthetic_rgb8();

    let mut profile = EffortProfile::lossy(7, EncoderMode::Reference);
    // e7 default = both true. Disabling forces no DCT16x16 / DCT16x8 /
    // DCT32x32 etc. merges, which definitively changes the bitstream.
    profile.try_dct16 = false;
    profile.try_dct32 = false;

    let cfg_override = baseline_lossy().with_effort_profile_override(profile);
    let bytes_override = encode(&cfg_override, &pixels);
    let bytes_baseline = encode(&baseline_lossy(), &pixels);

    // Both must be valid JXL bitstreams.
    assert_eq!(&bytes_override[..2], &[0xFF, 0x0A]);
    assert_eq!(&bytes_baseline[..2], &[0xFF, 0x0A]);

    assert_ne!(
        bytes_override, bytes_baseline,
        "EffortProfile override (try_dct16=false, try_dct32=false) must \
         change the bitstream when applied through zenjxl's re-exported \
         LossyConfig::with_effort_profile_override"
    );
}
