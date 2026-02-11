//! JPEG XL encoding and decoding with zencodec-types trait integration.
//!
//! Wraps `jxl` (jxl-rs) for decoding and `jxl-encoder` for encoding.
//! Both are feature-gated (`decode` and `encode` respectively).
//!
//! # zencodec-types traits
//!
//! [`JxlEncoding`] implements [`zencodec_types::Encoding`] (encode feature)
//! and [`JxlDecoding`] implements [`zencodec_types::Decoding`] (decode feature).

#![forbid(unsafe_code)]
#![no_std]

extern crate alloc;

#[cfg(feature = "decode")]
mod decode;
#[cfg(feature = "encode")]
mod encode;
mod error;
mod zencodec;

pub use error::JxlError;

#[cfg(feature = "decode")]
pub use decode::{JxlDecodeOutput, JxlInfo, JxlLimits, decode, probe};

#[cfg(feature = "encode")]
pub use encode::{
    encode_gray8, encode_gray8_lossless, encode_rgb8, encode_rgb8_lossless, encode_rgba8,
    encode_rgba8_lossless,
};

// zencodec-types trait types
#[cfg(feature = "encode")]
pub use zencodec::{JxlEncodeJob, JxlEncoding};

#[cfg(feature = "decode")]
pub use zencodec::{JxlDecodeJob, JxlDecoding};

// Re-export encoder config types for callers.
#[cfg(feature = "encode")]
pub use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};
