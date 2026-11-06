//! Whole-image integer grid evidence, separate from codestream depth.
#![cfg(all(feature = "decode", feature = "zencodec"))]
use std::borrow::Cow;
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::{Orientation, OrientationHint};
use zenjxl::JxlDecoderConfig;
use zenpixels::{ChannelType, PixelDescriptor};

const SOURCE: &[u8] = include_bytes!("fixtures/orientation5_transpose.jxl");

#[test]
fn integer_grid_survives_native_and_adapter_orientation() {
    for descriptor in [PixelDescriptor::RGB8_SRGB, PixelDescriptor::RGB16_SRGB] {
        let native = zenjxl::decode_with_options(SOURCE, None, &[descriptor], Some(false), None).unwrap();
        let exact = native.pixels.exact_depth().expect("native integer evidence");
        assert_eq!(exact.color_bits(), 8, "lossless eight-bit fixture");
        assert!(exact.holds_for(&native.pixels.as_slice()).unwrap());
        for hint in [OrientationHint::Preserve, OrientationHint::Correct,
            OrientationHint::ExactTransform(Orientation::FlipH),
            OrientationHint::CorrectAndTransform(Orientation::Rotate90)] {
            let output = JxlDecoderConfig::new().job().with_orientation(hint)
                .decoder(Cow::Borrowed(SOURCE), &[descriptor]).unwrap().decode().unwrap();
            let pixels = output.pixels();
            assert_eq!(pixels.descriptor().channel_type(), descriptor.channel_type());
            let exact = pixels.exact_depth().expect("adapter integer evidence");
            assert_eq!(exact.color_bits(), 8);
            assert!(exact.holds_for(&pixels).unwrap());
        }
    }
}

#[test]
fn float_output_has_no_integer_grid_claim() {
    let pixels = zenjxl::decode_with_options(SOURCE, None,
        &[PixelDescriptor::RGBF32_LINEAR], Some(false), None).unwrap().pixels;
    assert_eq!(pixels.descriptor().channel_type(), ChannelType::F32);
    assert!(pixels.exact_depth().is_none());
}
