//! Fail-fast validation for zenjxl-owned [`Config`](crate) types.
//!
//! Every public Config setter on zenjxl already clamps out-of-range values for
//! convenience (e.g. [`JxlEncoderConfig::with_distance`] clamps to
//! `0.0..=25.0`). That suits one-off encode calls but hides typos and bugs in
//! batch jobs that fan a single config out across many encodes. This module
//! adds an opt-in `validate()` method that returns `Err(ValidationError)` for
//! the same out-of-range inputs the setters silently clamp. Callers who want
//! the existing forgiving behaviour change nothing.
//!
//! Scope is limited to zenjxl's own Config types ([`JxlEncoderConfig`] /
//! [`JxlDecoderConfig`]). Validation of the re-exported `LossyConfig` /
//! `LosslessConfig` / `*InternalParams` types lives in jxl-encoder; once that
//! lands, it'll be reachable through `validate()` on the upstream types and
//! re-exported here as `ValidationError::JxlEncoder` (the variant is already
//! reserved behind `__expert`).
//!
//! [`JxlEncoderConfig`]: crate::JxlEncoderConfig
//! [`JxlDecoderConfig`]: crate::JxlDecoderConfig
//! [`JxlEncoderConfig::with_distance`]: crate::JxlEncoderConfig::with_distance

use core::ops::RangeInclusive;

/// Errors returned by `validate()` on zenjxl-owned Config types.
///
/// Each variant corresponds to a single out-of-range or otherwise rejected
/// field on a zenjxl-owned Config type. Variants are added in a non-breaking
/// way; the enum is `#[non_exhaustive]`.
#[non_exhaustive]
#[derive(Debug, Clone, thiserror::Error)]
pub enum ValidationError {
    /// `generic_quality` was set outside the documented `0.0..=100.0` range,
    /// or to NaN.
    #[error("generic_quality {value} out of valid range {valid:?}")]
    GenericQualityOutOfRange {
        value: f32,
        valid: RangeInclusive<f32>,
    },

    /// `distance` was set outside the documented `0.0..=25.0` range, or to
    /// NaN. (Encoder setters clamp on input, but `validate()` still checks the
    /// stored value so it's safe to use on raw configs constructed by other
    /// means.)
    #[error("distance {value} out of valid range {valid:?}")]
    DistanceOutOfRange {
        value: f32,
        valid: RangeInclusive<f32>,
    },

    /// `effort` was set outside the documented `1..=10` range. (Encoder
    /// setters clamp on input.)
    #[error("effort {value} out of valid range {valid:?}")]
    EffortOutOfRange {
        value: i32,
        valid: RangeInclusive<i32>,
    },

    /// Validation error from jxl-encoder propagated through a re-exported
    /// upstream Config (`LossyConfig`, `LosslessConfig`, or any
    /// `*InternalParams`).
    ///
    /// Wraps [`jxl_encoder::ValidationError`] (re-exported at the crate root
    /// as [`crate::JxlValidationError`] for the same `__expert` gate). The
    /// `#[from]` impl lets callers `?`-bubble upstream validation errors
    /// straight into [`ValidationError`]. The variant is gated behind
    /// `__expert` because that is the feature that exposes the
    /// `*InternalParams` types whose validation it reflects; once upstream
    /// `LossyConfig::validate()` is stable, this gate can be widened.
    #[cfg(feature = "__expert")]
    #[error(transparent)]
    JxlEncoder(#[from] jxl_encoder::ValidationError),
}

/// Internal validation helpers — only exist when there's a Config that
/// uses them. Today that's `zencodec`'s `JxlEncoderConfig`. Gating the
/// whole block on `feature = "zencodec"` avoids dead-code warnings on
/// builds that have neither validation consumer enabled.
#[cfg(feature = "zencodec")]
mod helpers {
    use super::ValidationError;
    use core::ops::RangeInclusive;

    /// Inclusive valid range for `generic_quality` (libjpeg-turbo scale).
    pub(crate) const GENERIC_QUALITY_RANGE: RangeInclusive<f32> = 0.0..=100.0;
    /// Inclusive valid range for butteraugli `distance`.
    pub(crate) const DISTANCE_RANGE: RangeInclusive<f32> = 0.0..=25.0;
    /// Inclusive valid range for `effort`.
    pub(crate) const EFFORT_RANGE: RangeInclusive<i32> = 1..=10;

    /// Validate an `Option<f32>` field against an inclusive range.
    ///
    /// Returns `Err` if `Some(v)` is NaN or falls outside `range`.
    pub(crate) fn check_optional_f32_range(
        value: Option<f32>,
        range: &RangeInclusive<f32>,
        mk_err: impl FnOnce(f32, RangeInclusive<f32>) -> ValidationError,
    ) -> Result<(), ValidationError> {
        if let Some(v) = value
            && (v.is_nan() || !range.contains(&v))
        {
            return Err(mk_err(v, range.clone()));
        }
        Ok(())
    }

    /// Validate an `Option<i32>` field against an inclusive range.
    pub(crate) fn check_optional_i32_range(
        value: Option<i32>,
        range: &RangeInclusive<i32>,
        mk_err: impl FnOnce(i32, RangeInclusive<i32>) -> ValidationError,
    ) -> Result<(), ValidationError> {
        if let Some(v) = value
            && !range.contains(&v)
        {
            return Err(mk_err(v, range.clone()));
        }
        Ok(())
    }
}

#[cfg(feature = "zencodec")]
pub(crate) use helpers::{
    DISTANCE_RANGE, EFFORT_RANGE, GENERIC_QUALITY_RANGE, check_optional_f32_range,
    check_optional_i32_range,
};
