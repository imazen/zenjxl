#![cfg(all(feature = "encode", feature = "decode", feature = "zencodec"))]
use std::borrow::Cow;
use zencodec::animation::FrameDuration;
use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};
use zencodec::encode::{AnimationFrameEncoder, EncodeJob, EncoderConfig};
use zenjxl::{JxlDecoderConfig, JxlEncoderConfig};
use zenpixels::{PixelBuffer, PixelDescriptor};

fn pixels() -> PixelBuffer {
    PixelBuffer::from_vec(
        [100, 50, 200].repeat(16 * 16),
        16,
        16,
        PixelDescriptor::RGB8_SRGB,
    )
    .unwrap()
}

#[test]
fn rational_clock_rescales_earlier_frames_and_preserves_source_pixels() {
    let pixels = pixels();
    for durations in [
        vec![(1001, 30000), (1001, 15000), (1, 60)],
        vec![(3, 1), (6, 1), (9, 1)],
        vec![(u64::from(u32::MAX) * 1024, 1)],
        vec![(u64::from(u32::MAX), 1), (u64::from(u32::MAX), 1)],
    ] {
        let mut encoder = JxlEncoderConfig::new()
            .with_lossless(true)
            .with_generic_effort(1)
            .job()
            .with_loop_count(Some(2))
            .animation_frame_encoder()
            .unwrap();
        for &(n, d) in &durations {
            encoder
                .push_frame_timed(pixels.as_slice(), FrameDuration::new(n, d).unwrap(), None)
                .unwrap();
        }
        let encoded = encoder.finish(None).unwrap();
        let mut decoder = JxlDecoderConfig::new()
            .job()
            .with_limits(zencodec::ResourceLimits::none())
            .animation_frame_decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap();
        assert_eq!(decoder.loop_count(), Some(2));
        for (index, (n, d)) in durations.into_iter().enumerate() {
            let frame = decoder.render_next_frame(None).unwrap().unwrap();
            assert_eq!(frame.duration(), FrameDuration::new(n, d).unwrap());
            assert_eq!(frame.frame_index(), index as u32);
            for row in 0..16 {
                assert_eq!(frame.pixels().row(row), pixels.as_slice().row(row));
            }
        }
        assert!(decoder.render_next_frame(None).unwrap().is_none());
    }
}

#[test]
fn clock_overflow_and_helper_zero_reject_without_rescaling_accepted_frame() {
    let pixels = pixels();
    let mut encoder = JxlEncoderConfig::new()
        .with_lossless(true)
        .with_generic_effort(1)
        .job()
        .animation_frame_encoder()
        .unwrap();
    assert!(
        encoder
            .push_frame_timed(pixels.as_slice(), FrameDuration::from_millis(0), None)
            .is_err()
    );
    let duration = FrameDuration::new(1, 1 << 30).unwrap();
    encoder
        .push_frame_timed(pixels.as_slice(), duration, None)
        .unwrap();
    assert!(
        encoder
            .push_frame_timed(
                pixels.as_slice(),
                FrameDuration::new(1, (1 << 30) - 1).unwrap(),
                None
            )
            .is_err()
    );
    encoder
        .push_frame_timed(pixels.as_slice(), duration, None)
        .unwrap();
    let encoded = encoder.finish(None).unwrap();
    let mut decoder = JxlDecoderConfig::new()
        .job()
        .animation_frame_decoder(Cow::Borrowed(encoded.data()), &[])
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            decoder.render_next_frame(None).unwrap().unwrap().duration(),
            duration
        );
    }
    assert!(decoder.render_next_frame(None).unwrap().is_none());
}

#[test]
fn truncated_animation_never_succeeds_with_a_shortened_sequence() {
    let pixels = pixels();
    let mut encoder = JxlEncoderConfig::new()
        .with_lossless(true)
        .with_generic_effort(1)
        .job()
        .animation_frame_encoder()
        .unwrap();
    for duration in [10, 20, 30] {
        encoder
            .push_frame(pixels.as_slice(), duration, None)
            .unwrap();
    }
    let encoded = encoder.finish(None).unwrap();
    let mut incomplete_successes = Vec::new();
    for end in 1..encoded.data().len() {
        let Ok(mut decoder) = JxlDecoderConfig::new()
            .job()
            .animation_frame_decoder(Cow::Borrowed(&encoded.data()[..end]), &[])
        else {
            continue;
        };
        let mut frames = 0;
        loop {
            match decoder.render_next_frame(None) {
                Ok(Some(_)) => frames += 1,
                Ok(None) => {
                    if frames != 3 {
                        incomplete_successes.push((end, frames));
                    }
                    break;
                }
                Err(_) => break,
            }
        }
    }
    assert!(
        incomplete_successes.is_empty(),
        "truncation accepted partial animation: {incomplete_successes:?}"
    );
}

#[test]
fn decoder_preserves_native_tick_products_above_u32() {
    let data = pixels().copy_to_contiguous_bytes();
    let clock = jxl_encoder::AnimationParams {
        tps_numerator: 1,
        tps_denominator: 1024,
        num_loops: 1,
        ..jxl_encoder::AnimationParams::default()
    };
    let frames = [jxl_encoder::AnimationFrame::new(&data, u32::MAX)];
    let bytes = jxl_encoder::LosslessConfig::new()
        .with_effort(1)
        .encode_animation(16, 16, jxl_encoder::PixelLayout::Rgb8, &clock, &frames)
        .unwrap();
    let mut decoder = JxlDecoderConfig::new()
        .job()
        .with_limits(zencodec::ResourceLimits::none())
        .animation_frame_decoder(Cow::Owned(bytes), &[])
        .unwrap();
    let frame = decoder.render_next_frame(None).unwrap().unwrap();
    let expected = u64::from(u32::MAX) * 1024;
    assert!(expected > u64::from(u32::MAX));
    assert_eq!(frame.duration(), FrameDuration::new(expected, 1).unwrap());
    assert!(decoder.render_next_frame(None).unwrap().is_none());
}

#[test]
fn first_canvas_is_available_before_a_truncated_later_frame() {
    let input = pixels();
    let mut encoder = JxlEncoderConfig::new()
        .with_lossless(true)
        .with_generic_effort(1)
        .job()
        .animation_frame_encoder()
        .unwrap();
    for _ in 0..3 {
        encoder.push_frame(input.as_slice(), 10, None).unwrap();
    }
    let encoded = encoder.finish(None).unwrap();
    let mut found = false;
    for end in 1..encoded.data().len() {
        let Ok(mut decoder) = JxlDecoderConfig::new()
            .job()
            .animation_frame_decoder(Cow::Borrowed(&encoded.data()[..end]), &[])
        else {
            continue;
        };
        if !matches!(decoder.render_next_frame(None), Ok(Some(_))) {
            continue;
        }
        let mut count = 1;
        loop {
            match decoder.render_next_frame(None) {
                Ok(Some(_)) => count += 1,
                Ok(None) => break,
                Err(_) => {
                    found = count < 3;
                    break;
                }
            }
        }
        if found {
            break;
        }
    }
    assert!(
        found,
        "decoder buffered all decoded canvases before yielding the first"
    );
}

#[test]
fn cumulative_admission_limits_and_unknown_color_are_atomic() {
    use zencodec::ResourceLimits;
    let input = pixels();
    for limits in [
        ResourceLimits::none().with_max_animation_ms(1),
        ResourceLimits::none().with_max_total_pixels(16 * 16 * 3),
    ] {
        let mut encoder = JxlEncoderConfig::new()
            .with_lossless(true)
            .with_generic_effort(1)
            .job()
            .with_limits(limits)
            .animation_frame_encoder()
            .unwrap();
        let duration = FrameDuration::new(1, 3000).unwrap();
        for _ in 0..3 {
            encoder
                .push_frame_timed(input.as_slice(), duration, None)
                .unwrap();
        }
        assert!(
            encoder
                .push_frame_timed(input.as_slice(), duration, None)
                .is_err()
        );
        let bytes = encoder.finish(None).unwrap();
        let mut decoder = JxlDecoderConfig::new()
            .job()
            .animation_frame_decoder(Cow::Borrowed(bytes.data()), &[])
            .unwrap();
        for _ in 0..3 {
            assert_eq!(
                decoder.render_next_frame(None).unwrap().unwrap().duration(),
                duration
            );
        }
        assert!(decoder.render_next_frame(None).unwrap().is_none());
    }
    let mut encoder = JxlEncoderConfig::new()
        .with_lossless(true)
        .with_generic_effort(1)
        .job()
        .animation_frame_encoder()
        .unwrap();
    let unknown = pixels().with_transfer(zenpixels::TransferFunction::Unknown);
    assert!(encoder.push_frame(unknown.as_slice(), 10, None).is_err());
    encoder.push_frame(input.as_slice(), 10, None).unwrap();
    assert!(encoder.finish(None).is_ok());
}

#[test]
fn hdr_animation_color_context_and_samples_survive_reencoding() {
    use zenpixels::{Cicp, ColorPrimaries, TransferFunction};
    for (cp, tf) in [
        (ColorPrimaries::Bt709, TransferFunction::Linear),
        (ColorPrimaries::Bt2020, TransferFunction::Pq),
        (ColorPrimaries::Bt2020, TransferFunction::Hlg),
    ] {
        let descriptor = PixelDescriptor::RGBA16_SRGB
            .with_primaries(cp)
            .with_transfer(tf);
        let cicp = Cicp::new(cp.to_cicp().unwrap(), tf.to_cicp().unwrap(), 0, true);
        let bytes: Vec<u8> = (0..17 * 13 * 4)
            .flat_map(|i| ((i * 173 + 32769) as u16).to_ne_bytes())
            .collect();
        let mut frames: Vec<_> = (0..3)
            .map(|_| {
                PixelBuffer::from_vec(bytes.clone(), 17, 13, descriptor)
                    .unwrap()
                    .with_cicp(cicp)
            })
            .collect();
        for _generation in 0..2 {
            let mut encoder = JxlEncoderConfig::new()
                .with_lossless(true)
                .with_generic_effort(1)
                .job()
                .animation_frame_encoder()
                .unwrap();
            for input in &frames {
                encoder.push_frame(input.as_slice(), 10, None).unwrap();
            }
            let bytes = encoder.finish(None).unwrap();
            let mut decoder = JxlDecoderConfig::new()
                .job()
                .animation_frame_decoder(Cow::Borrowed(bytes.data()), &[descriptor])
                .unwrap();
            let mut decoded = Vec::new();
            for expected in &frames {
                let frame = decoder.render_next_frame_owned(None).unwrap().unwrap();
                let got = frame.pixels();
                assert_eq!(got.descriptor().primaries, cp);
                assert_eq!(got.descriptor().transfer(), tf);
                assert_eq!(got.color_context().unwrap().cicp, Some(cicp));
                for y in 0..13 {
                    assert_eq!(got.row(y), expected.as_slice().row(y));
                }
                decoded.push(frame.into_buffer());
            }
            assert!(decoder.render_next_frame(None).unwrap().is_none());
            frames = decoded;
        }
    }
}

struct PollLimit(std::sync::atomic::AtomicUsize);
impl enough::Stop for PollLimit {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 8 {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
fn borrowed_finish_stop_reaches_native_animation_work() {
    for lossless in [true, false] {
        let input = PixelBuffer::from_vec(
            (0..511 * 259 * 3)
                .map(|i| ((i * 37 + i / 19 * 113) % 256) as u8)
                .collect(),
            511,
            259,
            PixelDescriptor::RGB8_SRGB,
        )
        .unwrap();
        let mut encoder = JxlEncoderConfig::new()
            .with_lossless(lossless)
            .with_generic_effort(1)
            .job()
            .animation_frame_encoder()
            .unwrap();
        encoder.push_frame(input.as_slice(), 10, None).unwrap();
        encoder.push_frame(input.as_slice(), 20, None).unwrap();
        let stop = PollLimit(std::sync::atomic::AtomicUsize::new(0));
        assert!(
            encoder.finish(Some(&stop)).is_err(),
            "lossless={lossless}, polls={}",
            stop.0.load(std::sync::atomic::Ordering::Relaxed)
        );
        assert!(stop.0.load(std::sync::atomic::Ordering::Relaxed) > 8);
    }
}
