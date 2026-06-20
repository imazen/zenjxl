//! JPEG XL encoding and decoding with zencodec trait integration.
//!
//! Wraps `jxl` (jxl-rs) for decoding and `jxl-encoder` for encoding.
//! Both are feature-gated (`decode` and `encode` respectively).
//!
//! # zencodec traits
//!
//! With the `zencodec` feature, `JxlEncoderConfig` implements `EncoderConfig` (encode feature)
//! and `JxlDecoderConfig` implements `DecoderConfig` (decode feature).

#![forbid(unsafe_code)]
#![no_std]

extern crate alloc;

whereat::define_at_crate_info!();

#[cfg(feature = "zencodec")]
mod codec;
#[cfg(feature = "decode")]
mod decode;
mod error;
#[cfg(feature = "jpeg-lossy")]
pub mod jpeg_lossy;
mod validate;
// #[cfg(feature = "zennode")]
// pub mod zennode_defs;

/// Budgeted sweep-plan construction over the encoder knob space
/// (variant generation for calibration sweeps and picker training).
///
/// Ports zenjpeg's variant-generation patterns — mode-discriminated
/// variants, resolved-state fingerprints, budgeted main-effects-first
/// sweep plans — to JXL. See `docs/VARIANT_GENERATION.md` in this repo
/// and the module docs for the axis provenance table.
///
/// **Private — do not depend on this in production code.** Gated behind
/// `__expert` (it drives jxl-encoder's internal-params escape hatch);
/// anything here can change without a semver bump.
#[cfg(all(feature = "encode", feature = "__expert"))]
pub mod sweep;

pub use error::JxlError;
pub use validate::ValidationError;

#[cfg(feature = "decode")]
pub use decode::{
    JxlDecodeOutput, JxlExtraChannelInfo, JxlExtraChannelType, JxlInfo, JxlLimits, decode,
    decode_with_options, decode_with_parallel, probe,
};
#[cfg(feature = "decode")]
pub use jxl::api::GainMapBundle;

#[cfg(feature = "encode")]
pub use jxl_encoder::convenience::{
    encode_bgra8, encode_bgra8_lossless, encode_gray8, encode_gray8_lossless, encode_rgb8,
    encode_rgb8_lossless, encode_rgba8, encode_rgba8_lossless,
};

// zencodec trait types
#[cfg(all(feature = "zencodec", feature = "encode"))]
pub use codec::{
    GainMapData, JxlAnimationFrameEncoder, JxlEncodeJob, JxlEncoder, JxlEncoderConfig,
};

// Resolved-plan introspection (JxlEncoderConfig::resolve_plan). Same
// stability caveat as everything behind `__expert`.
#[cfg(all(feature = "zencodec", feature = "encode", feature = "__expert"))]
pub use codec::{DistanceSource, JxlEncodePlan, LosslessPlan, LossyPlan};

#[cfg(all(feature = "zencodec", feature = "decode"))]
pub use codec::{JxlAnimationFrameDecoder, JxlDecodeJob, JxlDecoder, JxlDecoderConfig};

// Re-export encoder config types for callers.
#[cfg(feature = "encode")]
pub use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};

// Re-export container utilities and quality mapping.
#[cfg(feature = "encode")]
pub use jxl_encoder::container::{append_gain_map_box, is_bare_codestream, is_container};
#[cfg(feature = "encode")]
pub use jxl_encoder::{calibrated_jxl_quality, quality_to_distance};

/// Expert / unstable escape hatch — forwards jxl-encoder's `__expert` feature.
///
/// Re-exports the segmented internal-params types (`LossyInternalParams` and
/// `LosslessInternalParams`) plus `EncoderMode` and `EntropyMulTable` so
/// callers driving picker training or codec calibration sweeps can construct
/// per-mode override knobs and apply them via
/// `LossyConfig::with_internal_params(LossyInternalParams)` /
/// `LosslessConfig::with_internal_params(LosslessInternalParams)` (those
/// builder methods live on the re-exported `LossyConfig` / `LosslessConfig`
/// and are gated behind `__expert` in jxl-encoder itself).
///
/// Both `*InternalParams` structs are `#[non_exhaustive]` with `Default` and
/// `Option`-typed fields: `Some(v)` overrides the corresponding effort-derived
/// default, `None` keeps it. New knobs land additively without breaking
/// callers.
///
/// `EntropyMulTable` is re-exported because it is the field type of
/// `LossyInternalParams::entropy_mul_table`. `EncoderMode` is the public
/// `Reference` / `Experimental` selector on `LossyConfig` / `LosslessConfig`
/// and is reachable from stable jxl-encoder regardless; we re-export it here
/// for convenience inside the `__expert` namespace.
///
/// The internal types, their fields, and override semantics live in
/// jxl-encoder; see its `effort` module documentation for the full knob list
/// and how each one flows through the encoder. This crate adds no semantics
/// beyond forwarding.
///
/// **Private — do not depend on this in production code.** Double-underscore
/// prefix signals that anything reachable through this feature can change
/// without a semver bump.
#[cfg(feature = "__expert")]
pub use jxl_encoder::{EncoderMode, EntropyMulTable, LosslessInternalParams, LossyInternalParams};

/// Additional `__expert` re-exports used by [`sweep`]'s public axis
/// types: the W44-128 improvements bundle ([`EncoderStrategy`]), the
/// progressive-rendering selector ([`ProgressiveMode`]), the RCT
/// selector for `LosslessInternalParams::forced_rct` ([`RctType`]), and
/// the ANS histogram-normalization strategy for
/// `LossyInternalParams::ans_histogram_strategy_vardct`
/// ([`ANSHistogramStrategy`]). Same stability caveat as everything
/// behind `__expert`: no semver guarantees.
#[cfg(all(feature = "encode", feature = "__expert"))]
pub use jxl_encoder::api::EncoderStrategy;
#[cfg(all(feature = "encode", feature = "__expert"))]
pub use jxl_encoder::entropy_coding::ans::ANSHistogramStrategy;
#[cfg(all(feature = "encode", feature = "__expert"))]
pub use jxl_encoder::{ProgressiveMode, RctType};

/// Re-export of [`jxl_encoder::ValidationError`] under an aliased name so it
/// sits as a sibling of zenjxl's own [`ValidationError`] without shadowing it.
///
/// This is the inner type wrapped by [`ValidationError::JxlEncoder`]; it is
/// re-exported so callers can match on specific upstream variants after a
/// `?`-bubble without pulling `jxl_encoder` into scope themselves. Gated
/// behind `__expert` because the validation surface it covers
/// (`*InternalParams`) is itself only reachable through that feature.
#[cfg(feature = "__expert")]
pub use jxl_encoder::ValidationError as JxlValidationError;
