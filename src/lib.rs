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

// Allocation helpers honoring `zencodec::AllocPreference` per call site —
// used only by the decode path (the untrusted output buffers).
#[cfg(feature = "decode")]
mod alloc_util;
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

// ---------------------------------------------------------------------------
// One-shot convenience functions
//
// The core encode/decode job in a single call with sane defaults, for callers
// who haven't read the rest of the API. Purely additive — the
// `LossyConfig`/`LosslessConfig` builder path (and the typed `encode_rgba8`/…
// `imgref::ImgRef` wrappers re-exported above) remain the power API.
//
// The encode input is a [`zenpixels::PixelSlice`]: it carries the pixel format,
// width, height, and row stride, so the dimensions ride *with* the pixels —
// there is no separate `width`/`height` to keep in sync and no buffer-length
// mismatch to guard against (strided slices are packed before encode). The
// typed `encode_rgba8` name is taken by the `imgref::ImgRef` re-export above, so
// the slice one-shots are simply `encode` / `encode_lossless`. All three reuse
// the crate's natural error type, `whereat::At<JxlError>`.
// ---------------------------------------------------------------------------

// Map a zenpixels PixelDescriptor onto the jxl-encoder PixelLayout. Mirrors the
// zencodec encode adapter's mapping (src/codec.rs), but lives here so the
// `encode` / `encode_lossless` one-shots work under the default `encode` feature
// without requiring `zencodec`.
#[cfg(feature = "encode")]
fn layout_for_descriptor(
    desc: zenpixels::PixelDescriptor,
) -> Result<crate::PixelLayout, whereat::At<JxlError>> {
    use crate::PixelLayout;
    use zenpixels::{ChannelLayout, ChannelType};
    Ok(match (desc.layout(), desc.channel_type()) {
        // 8-bit
        (ChannelLayout::Rgb, ChannelType::U8) => PixelLayout::Rgb8,
        (ChannelLayout::Rgba, ChannelType::U8) => PixelLayout::Rgba8,
        (ChannelLayout::Bgra, ChannelType::U8) => PixelLayout::Bgra8,
        (ChannelLayout::Gray, ChannelType::U8) => PixelLayout::Gray8,
        (ChannelLayout::GrayAlpha, ChannelType::U8) => PixelLayout::GrayAlpha8,
        // 16-bit
        (ChannelLayout::Rgb, ChannelType::U16) => PixelLayout::Rgb16,
        (ChannelLayout::Rgba, ChannelType::U16) => PixelLayout::Rgba16,
        (ChannelLayout::Gray, ChannelType::U16) => PixelLayout::Gray16,
        (ChannelLayout::GrayAlpha, ChannelType::U16) => PixelLayout::GrayAlpha16,
        // f32 (jxl-encoder treats float input as linear-light)
        (ChannelLayout::Rgb, ChannelType::F32) => PixelLayout::RgbLinearF32,
        (ChannelLayout::Rgba, ChannelType::F32) => PixelLayout::RgbaLinearF32,
        (ChannelLayout::Gray, ChannelType::F32) => PixelLayout::GrayLinearF32,
        (ChannelLayout::GrayAlpha, ChannelType::F32) => PixelLayout::GrayAlphaLinearF32,
        _ => {
            return Err(whereat::at!(JxlError::UnsupportedOperation(
                zencodec::UnsupportedOperation::PixelFormat,
            )));
        }
    })
}

/// Encode a [`zenpixels::PixelSlice`] to a lossy JPEG XL codestream in one call.
///
/// The slice is self-describing: it carries the pixel format (RGBA8, RGB8,
/// BGRA8, grayscale, 16-bit, linear-f32, …), the dimensions, and the row stride,
/// so there is no separate `width`/`height` argument to keep in sync and no
/// buffer-length mismatch to guard against — strided slices are packed before
/// encode. Encodes at the default butteraugli distance `1.0` (≈ visually
/// lossless — the same target as `cjxl -d 1.0`). For a different quality, a
/// specific effort, embedded metadata, gain maps, or cancellation, build a
/// [`LossyConfig`] (map a 0..=100 quality with [`quality_to_distance`] /
/// [`calibrated_jxl_quality`]).
///
/// # Errors
/// Returns [`JxlError::UnsupportedOperation`] if the slice's pixel format is not
/// one the JXL encoder accepts, or [`JxlError::Encode`] for any encoder error.
///
#[cfg_attr(all(feature = "encode", feature = "decode"), doc = "```")]
#[cfg_attr(not(all(feature = "encode", feature = "decode")), doc = "```ignore")]
/// use zenjxl::{decode_rgba8, encode};
/// use zenpixels::{PixelDescriptor, PixelSlice};
///
/// // 2×2 RGBA, tightly packed — dims + stride + format ride with the pixels.
/// let (width, height) = (2u32, 2u32);
/// let rgba = vec![
///     255, 0, 0, 255, 0, 255, 0, 255, //
///     0, 0, 255, 255, 255, 255, 255, 255,
/// ];
/// let stride = width as usize * 4; // bytes per row (tightly packed)
/// let img = PixelSlice::new(&rgba, width, height, stride, PixelDescriptor::RGBA8_SRGB)
///     .expect("valid 2x2 RGBA8 slice");
///
/// let jxl = encode(img)?; // lossy, butteraugli distance 1.0 — no separate w/h
/// let (pixels, w, h) = decode_rgba8(&jxl)?;
///
/// assert_eq!((w, h), (width, height));
/// // Lossy is not bit-exact — check dimensions and length, not pixel values.
/// assert_eq!(pixels.len(), (width * height * 4) as usize);
/// # Ok::<(), whereat::At<zenjxl::JxlError>>(())
/// ```
#[cfg(feature = "encode")]
pub fn encode(
    img: zenpixels::PixelSlice<'_>,
) -> Result<alloc::vec::Vec<u8>, whereat::At<JxlError>> {
    let width = img.width();
    let height = img.rows();
    let layout = layout_for_descriptor(img.descriptor())?;
    let bytes = img.contiguous_bytes();
    crate::LossyConfig::new(1.0)
        .encode(&bytes, width, height, layout)
        .map_err(|e| e.map_error(JxlError::Encode))
}

/// Encode a [`zenpixels::PixelSlice`] to a lossless JPEG XL codestream in one
/// call.
///
/// As with [`encode`], the slice carries its own format, dimensions, and row
/// stride. Uses the default [`LosslessConfig`] (modular mode); the round-trip is
/// bit-exact. For a specific effort or embedded metadata, use [`LosslessConfig`]
/// directly.
///
/// # Errors
/// Returns [`JxlError::UnsupportedOperation`] if the slice's pixel format is not
/// one the JXL encoder accepts, or [`JxlError::Encode`] for any encoder error.
///
#[cfg_attr(all(feature = "encode", feature = "decode"), doc = "```")]
#[cfg_attr(not(all(feature = "encode", feature = "decode")), doc = "```ignore")]
/// use zenjxl::{decode_rgba8, encode_lossless};
/// use zenpixels::{PixelDescriptor, PixelSlice};
///
/// let (width, height) = (2u32, 2u32);
/// let rgba = vec![
///     255, 0, 0, 255, 0, 255, 0, 255, //
///     0, 0, 255, 255, 255, 255, 255, 255,
/// ];
/// let stride = width as usize * 4; // bytes per row (tightly packed)
/// let img = PixelSlice::new(&rgba, width, height, stride, PixelDescriptor::RGBA8_SRGB)
///     .expect("valid 2x2 RGBA8 slice");
///
/// let jxl = encode_lossless(img)?;
/// let (pixels, w, h) = decode_rgba8(&jxl)?;
///
/// assert_eq!((w, h), (width, height));
/// assert_eq!(pixels, rgba); // lossless — exact round-trip
/// # Ok::<(), whereat::At<zenjxl::JxlError>>(())
/// ```
#[cfg(feature = "encode")]
pub fn encode_lossless(
    img: zenpixels::PixelSlice<'_>,
) -> Result<alloc::vec::Vec<u8>, whereat::At<JxlError>> {
    let width = img.width();
    let height = img.rows();
    let layout = layout_for_descriptor(img.descriptor())?;
    let bytes = img.contiguous_bytes();
    crate::LosslessConfig::new()
        .encode(&bytes, width, height, layout)
        .map_err(|e| e.map_error(JxlError::Encode))
}

/// Decode a JPEG XL image (any color type / bit depth) to tightly-packed 8-bit
/// RGBA in one call.
///
/// Returns `(rgba, width, height)` where `rgba` is exactly `width * height * 4`
/// bytes (`R, G, B, A` per pixel, no stride padding). Grayscale, RGB, 16-bit and
/// HDR sources are all normalized to 8-bit RGBA; opaque sources get `A = 255`.
/// Uses the decoder's built-in defaults (no caller-supplied resource limits).
/// For the native pixel buffer, 16-bit output, metadata, gain maps, resource
/// limits, or cancellation, use [`decode`] / [`probe`].
///
/// # Errors
/// Returns a [`JxlError`] if `jxl` is not a valid JPEG XL codestream or a
/// resource limit is exceeded.
///
#[cfg_attr(all(feature = "encode", feature = "decode"), doc = "```")]
#[cfg_attr(not(all(feature = "encode", feature = "decode")), doc = "```ignore")]
/// use zenjxl::{decode_rgba8, encode_lossless};
/// use zenpixels::{PixelDescriptor, PixelSlice};
///
/// let (width, height) = (2u32, 2u32);
/// let rgba = vec![
///     255, 0, 0, 255, 0, 255, 0, 255, //
///     0, 0, 255, 255, 255, 255, 255, 255,
/// ];
/// let stride = width as usize * 4; // bytes per row (tightly packed)
/// let img = PixelSlice::new(&rgba, width, height, stride, PixelDescriptor::RGBA8_SRGB)
///     .expect("valid 2x2 RGBA8 slice");
///
/// let jxl = encode_lossless(img)?;
/// let (pixels, w, h) = decode_rgba8(&jxl)?;
///
/// assert_eq!((w, h), (width, height));
/// assert_eq!(pixels, rgba);
/// # Ok::<(), whereat::At<zenjxl::JxlError>>(())
/// ```
#[cfg(feature = "decode")]
pub fn decode_rgba8(jxl: &[u8]) -> Result<(alloc::vec::Vec<u8>, u32, u32), whereat::At<JxlError>> {
    use zenpixels_convert::PixelBufferConvertTypedExt as _;
    // Prefer a direct RGBA8 output (no conversion for 8-bit sources); the
    // decoder falls back to its native format when it can't, and `to_rgba8()`
    // normalizes whatever comes back.
    let output = crate::decode::decode(jxl, None, &[zenpixels::PixelDescriptor::RGBA8_SRGB])?;
    let width = output.info.width;
    let height = output.info.height;
    // `to_rgba8()` normalizes any native color type to RGBA8;
    // `copy_to_contiguous_bytes()` strips any stride padding.
    let rgba = output.pixels.to_rgba8().copy_to_contiguous_bytes();
    Ok((rgba, width, height))
}

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
