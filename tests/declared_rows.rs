#![cfg(all(feature = "encode", feature = "decode", feature = "zencodec"))]
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zenjxl::{JxlDecoderConfig, JxlEncoderConfig};
use zenpixels::{PixelDescriptor, PixelSlice};

#[test]
fn jxl_admits_declared_rows_and_preserves_padded_input() {
    for desc in [
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::RGBX8_SRGB,
    ] {
        let data: Vec<u8> = (0..96).map(|n| (n * 13) as u8).collect();
        let image = || PixelSlice::new(&data, 5, 4, 24, desc).unwrap();
        for lossless in [true, false] {
            let cfg = JxlEncoderConfig::new().with_lossless(lossless);
            let expected = cfg
                .clone()
                .job()
                .encoder()
                .unwrap()
                .encode(image())
                .unwrap()
                .into_vec();
            let mut enc = cfg.clone().job().encoder().unwrap();
            assert!(enc.begin(0, 4, desc).is_err());
            assert_eq!(
                enc.begin(5, 4, desc).unwrap(),
                zencodec::encode::OutputTiming::AT_FINISH
            );
            assert!(enc.begin(5, 4, desc).is_err());
            assert!(
                enc.push_rows(PixelSlice::new(&data, 4, 1, 24, desc).unwrap())
                    .is_err()
            );
            for y in [0, 2] {
                enc.push_rows(PixelSlice::new(&data[y * 24..], 5, 2, 24, desc).unwrap())
                    .unwrap();
            }
            assert!(
                enc.push_rows(PixelSlice::new(&data, 5, 1, 24, desc).unwrap())
                    .is_err()
            );
            assert_eq!(enc.finish().unwrap().into_vec(), expected);
            let mut enc = cfg.clone().job().encoder().unwrap();
            enc.begin(5, 4, desc).unwrap();
            enc.push_rows(PixelSlice::new(&data, 5, 1, 24, desc).unwrap())
                .unwrap();
            assert!(enc.finish().is_err());
            let mut enc = cfg.clone().job().encoder().unwrap();
            enc.begin(5, 4, desc).unwrap();
            assert!(enc.encode(image()).is_err());
            let mut enc = cfg.clone().job().encoder().unwrap();
            enc.begin(5, 4, desc).unwrap();
            let mut rgba = data.clone();
            assert!(enc.encode_srgba8(&mut rgba, false, 5, 4, 6).is_err());
            zencodec_testkit::check_streaming_output_honesty(
                cfg,
                JxlDecoderConfig::new(),
                &zencodec_testkit::TestImage::rgb8_gradient(5, 4),
            )
            .unwrap();
        }
    }
}

#[test]
fn declared_jxl_keeps_timeout_reasons_at_each_row_boundary() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use zencodec::how_far::{Child, Execution, PhaseSpec, PlanError, Pulse, Report};
    struct Deadline(Arc<AtomicBool>);
    impl enough::Stop for Deadline {
        fn check(&self) -> Result<(), enough::StopReason> {
            if self.0.load(Ordering::Relaxed) {
                Err(enough::StopReason::TimedOut)
            } else {
                Ok(())
            }
        }
    }
    impl Report for Deadline {
        fn advance(&self, _: u64) {}
    }
    impl Pulse for Deadline {
        fn split(&self, _: Execution, _: &[PhaseSpec<'_>]) -> Result<Vec<Child<'_>>, PlanError> {
            Err(PlanError::Unsupported)
        }
    }
    for stage in 0..3 {
        let expired = Arc::new(AtomicBool::new(false));
        let mut e = JxlEncoderConfig::new()
            .with_lossless(true)
            .job()
            .with_pulse(zencodec::SharedPulse::new(Deadline(expired.clone())))
            .encoder()
            .unwrap();
        let desc = PixelDescriptor::RGB8_SRGB;
        if stage > 0 {
            e.begin(1, 1, desc).unwrap();
        }
        if stage > 1 {
            e.push_rows(PixelSlice::new(&[1, 2, 3], 1, 1, 3, desc).unwrap())
                .unwrap();
        }
        expired.store(true, Ordering::Relaxed);
        let err = match stage {
            0 => e.begin(1, 1, desc).unwrap_err(),
            1 => e
                .push_rows(PixelSlice::new(&[1, 2, 3], 1, 1, 3, desc).unwrap())
                .unwrap_err(),
            _ => e.finish().unwrap_err(),
        };
        assert_eq!(
            err.error().category(),
            zencodec::ErrorCategory::Stopped(enough::StopReason::TimedOut)
        );
    }
}
