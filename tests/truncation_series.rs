//! zencodec-testkit conformance: EOF / truncation series.
//!
//! Encodes a small valid JXL codestream through the `JxlEncoderConfig` trait
//! path, then feeds it to `check_decode_truncation_series`, which truncates the
//! bytes at a deterministic series of prefixes and decodes each through the
//! dyn-erased `JxlDecoderConfig` path. Every truncated prefix must categorize
//! as incomplete-input (`ErrorCategory`), never panic, OOM, or map to Internal.

#![cfg(all(feature = "zencodec", feature = "encode", feature = "decode"))]

use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
use zenjxl::{JxlDecoderConfig, JxlEncoderConfig};

/// A small valid lossless JXL codestream, produced via the zencodec trait path.
fn valid_jxl() -> Vec<u8> {
    let (w, h) = (16u32, 16u32);
    let pixels: Vec<rgb::Rgb<u8>> = (0..w * h)
        .map(|i| {
            let v = (i as u8).wrapping_mul(31).wrapping_add(7);
            rgb::Rgb { r: v, g: v, b: v }
        })
        .collect();
    let buf =
        zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, w, h).expect("pixel buffer");
    JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .encoder()
        .expect("encoder")
        .encode(buf.as_slice().into())
        .expect("encode")
        .into_vec()
}

#[test]
fn truncation_series_categorizes_as_incomplete_input() {
    let valid = valid_jxl();
    zencodec_testkit::check_decode_truncation_series(JxlDecoderConfig::new(), &valid)
        .expect("truncated input must categorize as incomplete, never panic/OOM/Internal");
}
