// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! Smoke test for fail-fast `validate()` on zenjxl's own Config types.
//!
//! Encoder setters clamp out-of-range values silently. `validate()` reports
//! the same out-of-range inputs as `Err(ValidationError)` so batch jobs can
//! refuse to silently encode at unintended quality.

#![cfg(all(feature = "zencodec", feature = "encode", feature = "decode"))]

use zencodec::encode::EncoderConfig;
use zenjxl::{JxlDecoderConfig, JxlEncoderConfig, ValidationError};

#[test]
fn happy_path_default_encoder_validates() {
    let cfg = JxlEncoderConfig::new();
    cfg.validate().expect("default encoder config validates");
}

#[test]
fn happy_path_decoder_validates() {
    let cfg = JxlDecoderConfig::new();
    cfg.validate().expect("default decoder config validates");
}

#[test]
fn happy_path_in_range_quality_validates() {
    let cfg = JxlEncoderConfig::new().with_generic_quality(80.0);
    cfg.validate().expect("quality=80 validates");
}

#[test]
fn out_of_range_generic_quality_rejected() {
    // Setter does NOT clamp generic_quality (it stores the raw value
    // verbatim for roundtrip fidelity, then maps to a clamped distance
    // internally). validate() catches the out-of-range stored value.
    let cfg = JxlEncoderConfig::new().with_generic_quality(150.0);
    let err = cfg.validate().expect_err("quality=150 should be rejected");
    match err {
        ValidationError::GenericQualityOutOfRange { value, valid } => {
            assert_eq!(value, 150.0);
            assert_eq!(valid, 0.0..=100.0);
        }
        other => panic!("expected GenericQualityOutOfRange, got {other:?}"),
    }
}

#[test]
fn nan_generic_quality_rejected() {
    let cfg = JxlEncoderConfig::new().with_generic_quality(f32::NAN);
    let err = cfg.validate().expect_err("NaN quality should be rejected");
    assert!(matches!(
        err,
        ValidationError::GenericQualityOutOfRange { .. }
    ));
}

#[test]
fn negative_generic_quality_rejected() {
    let cfg = JxlEncoderConfig::new().with_generic_quality(-1.0);
    let err = cfg.validate().expect_err("quality=-1 should be rejected");
    assert!(matches!(
        err,
        ValidationError::GenericQualityOutOfRange { .. }
    ));
}

#[test]
fn distance_setter_clamps_so_validate_passes() {
    // with_distance clamps to 0.0..=25.0 on input, so even an absurd
    // request stores an in-range value and validate() returns Ok.
    let cfg = JxlEncoderConfig::new().with_distance(99.0);
    cfg.validate().expect("clamped distance validates");
}

#[test]
fn effort_setter_clamps_so_validate_passes() {
    let cfg = JxlEncoderConfig::new().with_generic_effort(99);
    cfg.validate().expect("clamped effort validates");
}
