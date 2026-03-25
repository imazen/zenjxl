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

#[cfg(feature = "zencodec")]
mod codec;
#[cfg(feature = "encode")]
pub mod container;
#[cfg(feature = "decode")]
mod decode;
#[cfg(feature = "encode")]
mod encode;
mod error;
#[cfg(feature = "zennode")]
pub mod zennode_defs;

pub use error::JxlError;

#[cfg(feature = "decode")]
pub use decode::{
    JxlDecodeOutput, JxlExtraChannelInfo, JxlExtraChannelType, JxlInfo, JxlLimits, decode,
    decode_with_parallel, probe,
};
#[cfg(feature = "decode")]
pub use jxl::api::GainMapBundle;

#[cfg(feature = "encode")]
pub use encode::{
    encode_bgra8, encode_bgra8_lossless, encode_gray8, encode_gray8_lossless, encode_rgb8,
    encode_rgb8_lossless, encode_rgba8, encode_rgba8_lossless,
};

// zencodec trait types
#[cfg(all(feature = "zencodec", feature = "encode"))]
pub use codec::{
    GainMapData, JxlAnimationFrameEncoder, JxlEncodeJob, JxlEncoder, JxlEncoderConfig,
};

#[cfg(all(feature = "zencodec", feature = "decode"))]
pub use codec::{JxlAnimationFrameDecoder, JxlDecodeJob, JxlDecoder, JxlDecoderConfig};

// Re-export encoder config types for callers.
#[cfg(feature = "encode")]
pub use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};
