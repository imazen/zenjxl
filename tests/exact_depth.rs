//! Whole-image integer grid evidence, separate from codestream depth.
#![cfg(all(feature = "decode", feature = "zencodec"))]
use std::borrow::Cow;
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::{Orientation, OrientationHint};
use zenjxl::JxlDecoderConfig;
use zenpixels::{PixelFormat, SampleType};

const SOURCE: &[u8] = include_bytes!("fixtures/orientation5_transpose.jxl");

#[test]
fn integer_grid_survives_native_and_adapter_orientation() {
    for descriptor in [PixelFormat::RGB8_SRGB, PixelFormat::RGB16_SRGB] {
        let native = zenjxl::decode_with_options(
            SOURCE,
            None,
            core::slice::from_ref(&descriptor),
            Some(false),
            None,
        )
        .unwrap();
        let exact = native.pixels.facts().depth.unwrap();
        assert_eq!(native.info.bit_depth, Some(8));
        assert_eq!(
            exact.color_bits(),
            1,
            "fixture contains only black and white"
        );
        let slice = native.pixels.as_view();
        for y in 0..slice.rows() {
            let row = slice.row(y);
            match descriptor.layout.sample {
                SampleType::U8 => assert!(row.iter().all(|&v| v == 0 || v == 255)),
                SampleType::U16 => assert!(
                    row.as_chunks::<2>()
                        .0
                        .iter()
                        .all(|v| matches!(u16::from_ne_bytes(*v), 0 | 65535))
                ),
                _ => unreachable!(),
            }
        }
        assert!(holds_for(exact.color_bits(), &native.pixels.as_view()));
        for hint in [
            OrientationHint::Preserve,
            OrientationHint::Correct,
            OrientationHint::ExactTransform(Orientation::FlipH),
            OrientationHint::CorrectAndTransform(Orientation::Rotate90),
        ] {
            let output = JxlDecoderConfig::new()
                .job()
                .with_orientation(hint)
                .decoder(Cow::Borrowed(SOURCE), core::slice::from_ref(&descriptor))
                .unwrap()
                .decode()
                .unwrap();
            let pixels = output.pixels();
            assert_eq!(pixels.format().layout.sample, descriptor.layout.sample);
            let exact = pixels.facts().depth.unwrap();
            assert_eq!(exact.color_bits(), 1);
            assert!(holds_for(exact.color_bits(), &pixels));
        }
    }
}

#[test]
fn float_output_has_no_integer_grid_claim() {
    let pixels = zenjxl::decode_with_options(
        SOURCE,
        None,
        &[PixelFormat::RGBF32_LINEAR],
        Some(false),
        None,
    )
    .unwrap()
    .pixels;
    assert_eq!(pixels.format().layout.sample, SampleType::F32);
    assert_eq!(pixels.facts().depth, None);
}

#[cfg(feature = "encode")]
#[test]
fn lossless_deep_samples_do_not_inherit_a_coarser_grid() {
    use zencodec::encode::{EncodeJob, EncoderConfig};
    use zenpixels::PixelView;
    for descriptor in [PixelFormat::RGB16_SRGB, PixelFormat::RGBA16_SRGB] {
        let channels = descriptor.layout.channels();
        let bytes: Vec<u8> = (0..8 * 4 * channels)
            .flat_map(|i| [0u16, 65, 32768, 65535][i % 4].to_ne_bytes())
            .collect();
        let slice = PixelView::new(&bytes, 8, 4, 8 * channels * 2, &descriptor).unwrap();
        let encoded = zenjxl::JxlEncoderConfig::new()
            .with_lossless(true)
            .job()
            .encode(slice)
            .unwrap();
        let native = zenjxl::decode_with_options(
            encoded.data(),
            None,
            core::slice::from_ref(&descriptor),
            Some(false),
            None,
        )
        .unwrap();
        assert_eq!(
            native.pixels.as_view().contiguous_bytes().unwrap().as_ref(),
            bytes.as_slice()
        );
        let exact = native.pixels.facts().depth.unwrap();
        assert_eq!(exact.color_bits(), 16);
        if descriptor.layout.order.has_alpha_slot() {
            assert_eq!(exact.alpha_bits(), 1);
        }
        assert!(holds_for(exact.color_bits(), &native.pixels.as_view()));
    }
}

fn holds_for(bits: u8, view: &zenpixels::PixelView<'_>) -> bool {
    let storage = view.format().layout.sample.bits();
    let wide = (1u32 << storage) - 1;
    let narrow = (1u32 << bits) - 1;
    (0..view.rows()).all(|y| {
        view.row(y)
            .chunks_exact(usize::from(storage / 8))
            .all(|bytes| {
                let value = match bytes {
                    [u8_value] => u32::from(*u8_value),
                    [lo, hi] => u32::from(u16::from_ne_bytes([*lo, *hi])),
                    _ => unreachable!(),
                };
                let code = (value * narrow + wide / 2) / wide;
                (code * wide + narrow / 2) / narrow == value
            })
    })
}
