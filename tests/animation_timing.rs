//! Timing admission and encoded frame durations through the shared adapter.
#![cfg(all(feature = "encode", feature = "decode", feature = "zencodec"))]

use std::borrow::Cow;
use zencodec::animation::FrameDuration;
use zencodec::decode::{AnimationFrameDecoder as _, DecodeJob as _, DecoderConfig as _};
use zencodec::encode::{AnimationFrameEncoder as _, EncodeJob as _, EncoderConfig as _};
use zenjxl::{JxlDecoderConfig, JxlEncoderConfig};

#[test]
fn ordinary_rounding_preserves_total_and_rejection_preserves_phase() {
    let mut encoder = JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .animation_frame_encoder()
        .unwrap();
    let pixels = zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(
        vec![rgb::Rgb::new(20, 40, 60); 16 * 16],
        16,
        16,
    )
    .unwrap();
    let duration = FrameDuration::new(1, 30).unwrap();
    assert!(
        encoder
            .push_frame_timed(
                pixels.as_slice().into(),
                FrameDuration::new(u64::MAX, 1).unwrap(),
                None
            )
            .is_err()
    );
    let mut expected = Vec::new();
    for _ in 0..30 {
        expected.push(
            encoder
                .push_frame_timed(pixels.as_slice().into(), duration, None)
                .unwrap(),
        );
    }
    assert_eq!(expected[0], FrameDuration::from_millis(33));
    assert_eq!(expected[1], FrameDuration::from_millis(34));
    assert_eq!(
        expected
            .iter()
            .map(|d| d.ticks_at(1000).unwrap())
            .sum::<u64>(),
        1000
    );
    encoder
        .push_frame_exact(
            pixels.as_slice().into(),
            FrameDuration::from_millis(10),
            None,
        )
        .unwrap();
    expected.push(FrameDuration::from_millis(10));
    let bytes = encoder.finish(None).unwrap().into_vec();
    let mut decoder = JxlDecoderConfig::new()
        .job()
        .animation_frame_decoder(Cow::Owned(bytes), &[])
        .unwrap();
    for duration in expected {
        let frame = decoder.render_next_frame_owned(None).unwrap().unwrap();
        assert_eq!(frame.duration(), duration);
    }
    assert!(decoder.render_next_frame_owned(None).unwrap().is_none());
}

#[test]
fn decoder_keeps_native_fractional_clock() {
    let pixels = vec![80u8; 16 * 16 * 3];
    let frames = [
        jxl_encoder::AnimationFrame::new(&pixels, 1),
        jxl_encoder::AnimationFrame::new(&pixels, 2),
    ];
    let animation = jxl_encoder::AnimationParams {
        tps_numerator: 30000,
        tps_denominator: 1001,
        ..Default::default()
    };
    let bytes = jxl_encoder::LosslessConfig::default()
        .encode_animation(16, 16, jxl_encoder::PixelLayout::Rgb8, &animation, &frames)
        .unwrap();
    let mut decoder = JxlDecoderConfig::new()
        .job()
        .animation_frame_decoder(Cow::Owned(bytes), &[])
        .unwrap();
    assert_eq!(
        decoder
            .render_next_frame_owned(None)
            .unwrap()
            .unwrap()
            .duration(),
        FrameDuration::new(1001, 30000).unwrap()
    );
    assert_eq!(
        decoder
            .render_next_frame_owned(None)
            .unwrap()
            .unwrap()
            .duration(),
        FrameDuration::new(2002, 30000).unwrap()
    );
}

#[test]
fn exact_encoding_rescales_buffered_frames() {
    let mut encoder = JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .animation_frame_encoder()
        .unwrap();
    let pixels = zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(
        vec![rgb::Rgb::new(20, 40, 60); 256],
        16,
        16,
    )
    .unwrap();
    let durations = [
        FrameDuration::from_millis(17),
        FrameDuration::new(1001, 30000).unwrap(),
        FrameDuration::new(1, 24).unwrap(),
    ];
    for duration in durations {
        encoder
            .push_frame_exact(pixels.as_slice().into(), duration, None)
            .unwrap();
    }
    assert!(
        encoder
            .push_frame_exact(
                pixels.as_slice().into(),
                FrameDuration::new(1, u32::MAX).unwrap(),
                None
            )
            .is_err()
    );
    let mut decoder = JxlDecoderConfig::new()
        .job()
        .animation_frame_decoder(Cow::Owned(encoder.finish(None).unwrap().into_vec()), &[])
        .unwrap();
    for duration in durations {
        assert_eq!(
            decoder
                .render_next_frame_owned(None)
                .unwrap()
                .unwrap()
                .duration(),
            duration
        );
    }
    assert!(decoder.render_next_frame_owned(None).unwrap().is_none());
}
