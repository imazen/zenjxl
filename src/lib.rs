//! JPEG XL encoding and decoding with zencodec-types trait integration.
//!
//! Wraps `jxl` (jxl-rs) for decoding and `jxl-encoder` for encoding.
//! Both are feature-gated (`decode` and `encode` respectively).
//!
//! # zencodec-types traits
//!
//! [`JxlEncoderConfig`] implements [`zencodec_types::EncoderConfig`] (encode feature)
//! and [`JxlDecoderConfig`] implements [`zencodec_types::DecoderConfig`] (decode feature).

#![forbid(unsafe_code)]
#![no_std]

extern crate alloc;

#[cfg(feature = "decode")]
mod decode;
#[cfg(feature = "encode")]
mod encode;
mod error;
#[cfg(feature = "zencodec")]
mod zencodec;

pub use error::JxlError;

#[cfg(feature = "decode")]
pub use decode::{JxlDecodeOutput, JxlInfo, JxlLimits, decode, probe};

#[cfg(feature = "encode")]
pub use encode::{
    encode_bgra8, encode_bgra8_lossless, encode_gray8, encode_gray8_lossless, encode_rgb8,
    encode_rgb8_lossless, encode_rgba8, encode_rgba8_lossless,
};

// zencodec-types trait types
#[cfg(all(feature = "zencodec", feature = "encode"))]
pub use zencodec::{JxlEncodeJob, JxlEncoder, JxlEncoderConfig, JxlFrameEncoder};

#[cfg(all(feature = "zencodec", feature = "decode"))]
pub use zencodec::{
    JxlDecodeJob, JxlDecoder, JxlDecoderConfig, JxlFrameDecoder, JxlStreamingDecoder,
};

// Re-export encoder config types for callers.
#[cfg(feature = "encode")]
pub use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};
