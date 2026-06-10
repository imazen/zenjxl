// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! `JxlEncoderConfig::resolve_plan()` introspection contract.
//!
//! The plan must report exactly what the encode path will run, because
//! it reads the same stored `LossyConfig`/`LosslessConfig` the encode
//! consumes — these tests pin that the quality→distance chain, effort
//! resolution, and mode discrimination agree with the sweep module's
//! resolution helper and with `validate()`.

#![cfg(all(feature = "zencodec", feature = "encode", feature = "__expert"))]

use zencodec::encode::EncoderConfig;
use zenjxl::sweep::resolve_distance_for_quality;
use zenjxl::{DistanceSource, JxlEncodePlan, JxlEncoderConfig, ValidationError};

#[test]
fn default_plan_is_lossy_distance_1_effort_7() {
    let JxlEncodePlan::Lossy(plan) = JxlEncoderConfig::new().resolve_plan() else {
        panic!("default config must plan lossy");
    };
    assert_eq!(plan.distance, 1.0);
    assert_eq!(plan.distance_source, DistanceSource::Default);
    assert_eq!(plan.effort, 7, "upstream constructor default");
    assert!(!plan.noise);
    assert!(!plan.container_forced);
    assert_eq!(plan.generic_quality, None);
}

#[test]
fn quality_resolves_through_the_same_chain_as_the_sweep_module() {
    for q in [5.0_f32, 20.0, 35.0, 50.0, 72.0, 85.0, 95.0, 100.0] {
        let JxlEncodePlan::Lossy(plan) = JxlEncoderConfig::new()
            .with_generic_quality(q)
            .resolve_plan()
        else {
            panic!("lossy expected");
        };
        assert_eq!(
            plan.distance,
            resolve_distance_for_quality(q),
            "plan and sweep resolution diverged at q{q}"
        );
        assert_eq!(plan.distance_source, DistanceSource::CalibratedQuality);
        assert_eq!(plan.generic_quality, Some(q));
        assert!(plan.calibrated_native_quality.is_some());
    }
}

#[test]
fn distance_override_bypasses_calibration() {
    let JxlEncodePlan::Lossy(plan) = JxlEncoderConfig::new()
        .with_generic_quality(85.0)
        .with_distance(2.5)
        .resolve_plan()
    else {
        panic!("lossy expected");
    };
    assert_eq!(plan.distance, 2.5);
    assert_eq!(plan.distance_source, DistanceSource::Override);
    // with_distance clears quality state (it takes priority).
    assert_eq!(plan.generic_quality, None);
}

#[test]
fn effort_resolves_off_the_stored_config() {
    let JxlEncodePlan::Lossy(plan) = JxlEncoderConfig::new()
        .with_generic_effort(9)
        .resolve_plan()
    else {
        panic!("lossy expected");
    };
    assert_eq!(plan.effort, 9);

    let JxlEncodePlan::Lossless(plan) = JxlEncoderConfig::new()
        .with_lossless(true)
        .with_generic_effort(3)
        .resolve_plan()
    else {
        panic!("lossless expected");
    };
    assert_eq!(plan.effort, 3);
}

#[test]
fn lossless_plan_reports_dead_knobs_and_validate_rejects_noise() {
    let cfg = JxlEncoderConfig::new()
        .with_generic_quality(80.0)
        .with_noise(true)
        .with_lossless(true);
    let JxlEncodePlan::Lossless(plan) = cfg.resolve_plan() else {
        panic!("lossless expected");
    };
    assert!(plan.inert_knobs.contains(&"noise"));
    assert!(plan.inert_knobs.contains(&"generic_quality"));
    // The noise combination has no defined meaning — rejected, not
    // remapped (the quality knobs are tolerated for generic zencodec
    // pipelines and only reported).
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::NoiseInLosslessMode)
    ));

    // Dropping noise makes the same config valid again.
    let cfg = cfg.with_noise(false);
    cfg.validate().expect("lossless without noise validates");
    let JxlEncodePlan::Lossless(plan) = cfg.resolve_plan() else {
        panic!("lossless expected");
    };
    assert!(!plan.inert_knobs.contains(&"noise"));
}

#[test]
fn noise_in_lossy_mode_is_live_and_valid() {
    let cfg = JxlEncoderConfig::new().with_noise(true);
    cfg.validate().expect("noise in lossy mode is live");
    let JxlEncodePlan::Lossy(plan) = cfg.resolve_plan() else {
        panic!("lossy expected");
    };
    assert!(plan.noise);
}

#[test]
fn gain_map_forces_container_in_the_plan() {
    let cfg = JxlEncoderConfig::new().with_gain_map(vec![0u8; 16]);
    let JxlEncodePlan::Lossy(plan) = cfg.resolve_plan() else {
        panic!("lossy expected");
    };
    assert!(plan.container_forced);
}
