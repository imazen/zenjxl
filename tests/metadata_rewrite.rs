#![cfg(feature = "metadata-rewrite")]
// Reused unchanged expectations from zencodecs at aa21cba0.
use zencodec::metadata_rewrite::{Action, Error, Request as ScrubRequest};
trait PlanRequest<'a> {
    fn plan(&self) -> Result<zencodec::metadata_rewrite::Plan<'a>, Error>;
}
impl<'a> PlanRequest<'a> for ScrubRequest<'a> {
    fn plan(&self) -> Result<zencodec::metadata_rewrite::Plan<'a>, Error> {
        zenjxl::metadata::rewrite(self)
    }
}
mod jxl_scrub {
    use super::*;
    pub fn box_(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        [(data.len() as u32 + 8).to_be_bytes().as_slice(), kind, data].concat()
    }
    pub fn fixture(extras: &[u8]) -> Vec<u8> {
        [
            b"\0\0\0\x0cJXL \r\n\x87\n".as_slice(),
            &box_(b"ftyp", b"jxl \0\0\0\0jxl "),
            extras,
            &box_(b"jxlc", b"\xff\x0aopaque-codestream"),
        ]
        .concat()
    }
    #[test]
    fn removes_plain_and_compressed_private_packets_without_decompressing() {
        let extra = [box_(b"xml ", b"SECRET"), box_(b"zzzz", b"SECRET")].concat();
        let input = fixture(&extra);
        let out = ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap();
        assert_eq!(out, fixture(&[]));
        let input = fixture(&box_(b"brob", b"xml SECRET-not-valid-brotli"));
        assert_eq!(
            ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap(),
            fixture(&[])
        );
    }
    #[test]
    fn reconstruction_loss_requires_explicit_opt_in_even_when_compressed() {
        for b in [
            box_(b"jbrd", b"SECRET-reconstruction"),
            box_(b"brob", b"jbrdSECRET-reconstruction"),
        ] {
            let input = fixture(&b);
            assert!(ScrubRequest::new(&input).plan().is_err());
            let req = ScrubRequest::new(&input).with_jpeg_reconstruction_removal(true);
            let plan = req.plan().unwrap();
            assert!(
                plan.report()
                    .iter()
                    .any(|r| r.action() == Action::RemoveJpegReconstruction)
            );
            assert_eq!(plan.to_vec().unwrap(), fixture(&[]));
        }
    }
    #[test]
    fn malformed_box_sizes_and_partial_codestream_sequences_refuse() {
        for extra in [
            b"\0\0\0\x04zzzz".to_vec(),
            box_(b"jxlp", b"\x80\0\0\x02X"),
            box_(b"brob", b"jxlcX"),
            box_(b"jxlc", b"duplicate"),
        ] {
            assert!(ScrubRequest::new(&fixture(&extra)).plan().is_err());
        }
        let input = fixture(&[]);
        for end in 0..input.len() {
            assert!(ScrubRequest::new(&input[..end]).plan().is_err(), "{end}");
        }
    }
    #[test]
    fn compressed_exif_requires_bounded_decompression() {
        let input = fixture(&box_(b"brob", b"Exifcompressed"));
        assert!(matches!(
            ScrubRequest::new(&input).plan(),
            Err(Error::Unsupported(_))
        ));
    }
}

#[test]
fn cancellation_is_observed_before_parsing() {
    struct Stop;
    impl enough::Stop for Stop {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::Cancelled)
        }
    }
    assert_eq!(
        ScrubRequest::new(b"data")
            .with_stop(&Stop)
            .plan()
            .unwrap_err(),
        Error::Cancelled
    );
}

#[test]
fn bounded_mutations_preserve_plan_accounting() {
    let seeds = [
        b"\xff\x0a".to_vec(),
        jxl_scrub::fixture(&jxl_scrub::box_(b"xml ", b"SECRET")),
    ];
    for seed in seeds {
        for i in 0..seed.len() {
            for byte in [0, 1, 127, 255] {
                let mut input = seed.clone();
                input[i] = byte;
                if let Ok(plan) = ScrubRequest::new(&input)
                    .with_byte_limits(4096, 4096)
                    .with_metadata_limit(1024)
                    .with_carrier_limit(64)
                    .plan()
                {
                    assert_eq!(
                        plan.output_len(),
                        plan.chunks().map(<[u8]>::len).sum::<usize>()
                    );
                    for entry in plan.report() {
                        assert!(entry.source_range().end <= input.len());
                    }
                }
            }
        }
    }
}
