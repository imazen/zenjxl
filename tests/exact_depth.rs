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
        let native =
            zenjxl::decode_with_options(SOURCE, None, &[descriptor], Some(false), None).unwrap();
        let exact = native
            .pixels
            .exact_depth()
            .expect("native integer evidence");
        assert_eq!(native.info.bit_depth, Some(8));
        assert_eq!(
            exact.color_bits(),
            1,
            "fixture contains only black and white"
        );
        let slice = native.pixels.as_slice();
        for y in 0..slice.rows() {
            let row = slice.row(y);
            match descriptor.channel_type() {
                ChannelType::U8 => assert!(row.iter().all(|&v| v == 0 || v == 255)),
                ChannelType::U16 => assert!(
                    row.as_chunks::<2>()
                        .0
                        .iter()
                        .all(|v| matches!(u16::from_ne_bytes(*v), 0 | 65535))
                ),
                _ => unreachable!(),
            }
        }
        assert!(exact.holds_for(&native.pixels.as_slice()).unwrap());
        for hint in [
            OrientationHint::Preserve,
            OrientationHint::Correct,
            OrientationHint::ExactTransform(Orientation::FlipH),
            OrientationHint::CorrectAndTransform(Orientation::Rotate90),
        ] {
            let output = JxlDecoderConfig::new()
                .job()
                .with_orientation(hint)
                .decoder(Cow::Borrowed(SOURCE), &[descriptor])
                .unwrap()
                .decode()
                .unwrap();
            let pixels = output.pixels();
            assert_eq!(
                pixels.descriptor().channel_type(),
                descriptor.channel_type()
            );
            let exact = pixels.exact_depth().expect("adapter integer evidence");
            assert_eq!(exact.color_bits(), 1);
            assert!(exact.holds_for(&pixels).unwrap());
        }
    }
}

#[test]
fn float_output_has_no_integer_grid_claim() {
    let pixels = zenjxl::decode_with_options(
        SOURCE,
        None,
        &[PixelDescriptor::RGBF32_LINEAR],
        Some(false),
        None,
    )
    .unwrap()
    .pixels;
    assert_eq!(pixels.descriptor().channel_type(), ChannelType::F32);
    assert!(pixels.exact_depth().is_none());
}

#[cfg(feature = "encode")]
#[test]
fn lossless_deep_samples_do_not_inherit_a_coarser_grid() {
    use zencodec::encode::{EncodeJob, EncoderConfig};
    use zenpixels::PixelSlice;
    for descriptor in [PixelDescriptor::RGB16_SRGB, PixelDescriptor::RGBA16_SRGB] {
        let channels = descriptor.channels();
        let bytes: Vec<u8> = (0..8 * 4 * channels)
            .flat_map(|i| [0u16, 65, 32768, 65535][i % 4].to_ne_bytes())
            .collect();
        let slice = PixelSlice::new(&bytes, 8, 4, 8 * channels * 2, descriptor).unwrap();
        let encoded = zenjxl::JxlEncoderConfig::new()
            .with_lossless(true)
            .job()
            .encode(slice)
            .unwrap();
        let native =
            zenjxl::decode_with_options(encoded.data(), None, &[descriptor], Some(false), None)
                .unwrap();
        assert_eq!(
            native.pixels.as_slice().contiguous_bytes().as_ref(),
            bytes.as_slice()
        );
        let exact = native.pixels.exact_depth().unwrap();
        assert_eq!(exact.color_bits(), 16);
        assert!(exact.holds_for(&native.pixels.as_slice()).unwrap());
    }
}
