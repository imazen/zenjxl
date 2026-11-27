#![cfg(all(feature = "encode", feature = "decode", feature = "zencodec"))]
use std::borrow::Cow;
use zencodec::Metadata;
#[cfg(feature = "metadata-rewrite")]
use zencodec::MetadataPolicy;
use zencodec::animation::FrameDuration as Duration;
use zencodec::decode::{Decode, DecodeJob, DecodePolicy, DecoderConfig};
use zencodec::encode::{EncodeJob, EncodePolicy, EncoderConfig};
use zencodec_testkit as tk;
use zenjxl::{JxlDecoderConfig as D, JxlEncoderConfig as E};

#[test]
fn timing_admission_and_all_duration_paths() {
    let d = |n, den| Duration::new(n, den).unwrap();
    let ms = Duration::from_millis;
    let img = tk::TestImage::rgb8_gradient(16, 16);
    let exact = [
        (ms(17), Some(ms(17))),
        (d(1001, 30000), Some(d(1001, 30000))),
        (d(1, u32::MAX), None),
        (d(1, 24), Some(d(1, 24))),
        (d(u64::MAX, 1), None),
        (ms(10), Some(ms(10))),
    ];
    tk::check_animation_timing(
        E::new().with_lossless(true),
        D::new(),
        img.as_slice(),
        true,
        &exact,
    )
    .unwrap();
    let ordinary = [
        (d(1, 30), Some(ms(33))),
        (d(u64::MAX, 1), None),
        (d(1, 30), Some(ms(34))),
        (d(1, 30), Some(ms(33))),
    ];
    tk::check_animation_timing(
        E::new().with_lossless(true),
        D::new(),
        img.as_slice(),
        false,
        &ordinary,
    )
    .unwrap();
}

#[test]
fn checked_policy_defaults_and_inventory_feature_contract() {
    let img = tk::TestImage::rgb8_gradient(16, 16);
    let enc = E::new().with_lossless(true);
    let bytes = enc.clone().job().encode(img.as_slice()).unwrap();
    let expected = D::new()
        .job()
        .decoder(Cow::Borrowed(bytes.data()), &[])
        .unwrap()
        .decode()
        .unwrap()
        .metadata();
    for (policy, accepted) in [
        (EncodePolicy::none(), true),
        (EncodePolicy::strip_all(), true),
        (EncodePolicy::none().with_strict(true), false),
    ] {
        tk::check_encode_policy_admission(
            enc.clone(),
            D::new(),
            img.as_slice(),
            Metadata::none(),
            EncodePolicy::none(),
            policy,
            accepted,
            &expected,
        )
        .unwrap();
    }
    for (policy, accepted) in [
        (DecodePolicy::none(), true),
        (DecodePolicy::untrusted_minimal(), false),
    ] {
        tk::check_decode_policy_admission(
            D::new(),
            bytes.data(),
            DecodePolicy::none(),
            policy,
            accepted,
            &expected,
        )
        .unwrap();
    }
    tk::check_inventory_dispatch(D::new(), bytes.data()).unwrap();
    #[cfg(feature = "inventory")]
    tk::check_inventory_input_limit(D::new(), bytes.data()).unwrap();
}

#[cfg(feature = "metadata-rewrite")]
#[test]
fn rewrite_preserves_encoded_image_and_decoded_metadata() {
    let img = tk::TestImage::rgb8_gradient(16, 16);
    let encoded = E::new()
        .with_lossless(true)
        .job()
        .encode(img.as_slice())
        .unwrap();
    let mut input = if encoded.data().starts_with(b"\xff\x0a") {
        let mut v = b"\0\0\0\x0cJXL \r\n\x87\n".to_vec();
        add_box(&mut v, b"ftyp", b"jxl \0\0\0\0jxl ");
        add_box(&mut v, b"jxlc", encoded.data());
        v
    } else {
        encoded.data().to_vec()
    };
    let clean = input.clone();
    add_box(&mut input, b"xml ", b"<private>SECRET</private>");
    tk::check_metadata_rewrite(
        zencodec::metadata_rewrite::RewriteRequest::new(&input),
        zenjxl::metadata::rewrite,
        |out| {
            assert_eq!(out, clean, "image carrier bytes must not change");
            let a = zenjxl::decode(&input, None, &[]).unwrap();
            let b = zenjxl::decode(out, None, &[]).unwrap();
            assert_eq!(
                a.pixels.as_view().as_contiguous_bytes(),
                b.pixels.as_view().as_contiguous_bytes()
            );
            assert_eq!(a.info.icc_profile, b.info.icc_profile);
            Ok(())
        },
    )
    .unwrap();
    tk::check_metadata_rewrite(
        zencodec::metadata_rewrite::RewriteRequest::new(&input)
            .with_policy(MetadataPolicy::PreserveExact),
        zenjxl::metadata::rewrite,
        |out| {
            assert_eq!(out, input);
            Ok(())
        },
    )
    .unwrap();
}
#[cfg(feature = "metadata-rewrite")]
fn add_box(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
}

#[test]
fn oriented_pq_rows_and_hdr_envelope_survive_reencode() {
    use jxl_encoder::{
        ColorEncoding, ImageMetadata, LosslessConfig, PixelLayout, Primaries, TransferFunction,
    };
    use zencodec::{Cicp, Orientation, OrientationHint};
    use zenpixels::{ContentLightLevel, SignalRange, Transfer};

    let rgb = [
        [17u16, 543, 49007],
        [5573, 18171, 61003],
        [1117, 32841, 15791],
        [62577, 23917, 975],
        [37115, 71, 20477],
        [511, 15333, 45269],
    ];
    let bytes = |indexes: &[usize]| -> Vec<u8> {
        indexes
            .iter()
            .flat_map(|&i| rgb[i])
            .flat_map(u16::to_ne_bytes)
            .collect()
    };
    let mut pq = ColorEncoding::srgb();
    pq.primaries = Primaries::Bt2100;
    pq.transfer_function = TransferFunction::Pq;
    // Independently author conflicting EXIF. The JXL codestream orientation
    // controls the active image, while the original TIFF bytes stay available.
    let exif = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x02\0\0\0\0\0\0\0";
    assert_eq!(
        zencodec::exif::Exif::parse(exif).unwrap().orientation(),
        Some(Orientation::FlipH)
    );
    let metadata = ImageMetadata::new()
        .with_orientation(Orientation::Rotate90)
        .with_exif(exif)
        .with_intensity_target(4_000.0);
    let input = LosslessConfig::default()
        .encode_request(3, 2, PixelLayout::Rgb16)
        .with_color_encoding(pq)
        .with_metadata(&metadata)
        .encode(&bytes(&[0, 1, 2, 3, 4, 5]))
        .unwrap();
    let expected_cicp = Cicp::new(9, 16, 0, true);
    let probe = zenjxl::probe(&input).unwrap();
    let synthesized_icc = probe.icc_profile.expect("PQ enum exports a derived ICC");
    assert_eq!(probe.cicp, Some((9, 16, 0, true)));

    let decode = |data: &[u8], hint| {
        D::new()
            .job()
            .with_orientation(hint)
            .decoder(Cow::Borrowed(data), &[])
            .unwrap()
            .decode()
            .unwrap()
    };
    let assert_rows = |output: &zencodec::decode::DecodeOutput, dims, indexes: &[usize]| {
        let pixels = output.pixels();
        assert_eq!((pixels.width(), pixels.height()), dims);
        assert_eq!(
            pixels.to_contiguous_bytes().unwrap().as_ref(),
            bytes(indexes)
        );
        assert!(matches!(
            pixels.format().color,
            zenpixels::ColorSpace::Rgb {
                primaries: zenpixels::Primaries::Bt2020,
                transfer: Transfer::Pq,
                range: SignalRange::Full,
                ..
            }
        ));
        assert_eq!(pixels.origin().unwrap().cicp, Some(expected_cicp));
        assert!(
            pixels.origin().unwrap().icc.is_none(),
            "ICC is derived from the enum"
        );
        assert_eq!(output.info().source_color.cicp, Some(expected_cicp));
        assert!(output.info().source_color.has_hdr_transfer());
        assert_eq!(
            output.info().source_color.icc_profile.as_deref(),
            Some(synthesized_icc.as_slice())
        );
        assert_eq!(
            output.info().source_color.content_light_level,
            Some(ContentLightLevel::new(4000, 0))
        );
        assert_eq!(
            pixels.hdr().content_light_level,
            Some(ContentLightLevel::new(4000, 0))
        );
    };
    for (hint, dims, indexes, residual) in [
        (
            OrientationHint::Preserve,
            (3, 2),
            [0, 1, 2, 3, 4, 5],
            Orientation::Rotate90,
        ),
        (
            OrientationHint::Correct,
            (2, 3),
            [3, 0, 4, 1, 5, 2],
            Orientation::Identity,
        ),
    ] {
        let output = decode(&input, hint);
        assert_rows(&output, dims, &indexes);
        assert_eq!(output.info().orientation, residual);
        let mut sink = CompletedRows::default();
        let completed = D::new()
            .job()
            .with_orientation(hint)
            .push_decoder(Cow::Borrowed(&input), &mut sink, &[])
            .unwrap();
        let applied = if matches!(hint, OrientationHint::Preserve) {
            Orientation::Identity
        } else {
            Orientation::Rotate90
        };
        assert_eq!(completed.orientation_applied, applied);
        let copied = sink.metadata.unwrap().attach_owned(sink.pixels.unwrap());
        assert_eq!(copied.hdr(), output.pixels().hdr());
        assert_eq!(copied.origin().cicp, Some(expected_cicp));
        assert_eq!(copied.facts().depth, output.pixels().facts().depth);
        assert_eq!(
            copied.as_view().to_contiguous_bytes().unwrap().as_ref(),
            bytes(&indexes)
        );
        assert_eq!(
            output.info().embedded_metadata.exif.as_deref(),
            Some(exif.as_slice())
        );
        let encoded = E::new()
            .with_lossless(true)
            .job()
            .with_unfiltered_metadata(
                Metadata::from(output.info()).filtered(&zencodec::MetadataPolicy::PreserveExact),
            )
            .encode(output.pixels())
            .unwrap();
        let header = zenjxl::probe(encoded.data()).unwrap();
        assert_eq!(header.intensity_target, 4000.0);
        assert_eq!(header.intrinsic_orientation, residual as u8);
        let stored = decode(encoded.data(), OrientationHint::Preserve);
        assert_rows(&stored, dims, &indexes);
        let reencoded_exif = stored.info().embedded_metadata.exif.as_deref().unwrap();
        assert_eq!(
            zencodec::exif::Exif::parse(reencoded_exif)
                .unwrap()
                .orientation(),
            Some(residual)
        );
        assert_rows(
            &decode(encoded.data(), OrientationHint::Correct),
            (2, 3),
            &[3, 0, 4, 1, 5, 2],
        );
    }
}

#[derive(Default)]
struct CompletedRows {
    pixels: Option<zenpixels::PixelBuf>,
    metadata: Option<zencodec::decode::RowMetadata>,
}

impl zencodec::decode::DecodeRowSink for CompletedRows {
    fn provide_next_buffer(
        &mut self,
        strip: zencodec::decode::RowStrip,
        format: zenpixels::PixelFormat,
    ) -> Result<zenpixels::PixelViewMut<'_>, zencodec::decode::SinkError> {
        assert_eq!(strip.y, 0);
        let pixels =
            self.pixels
                .insert(zenpixels::PixelBuf::new(strip.width, strip.height, format)?);
        Ok(pixels.as_view_mut())
    }

    fn provide_buffer_with_metadata(
        &mut self,
        batch: zencodec::decode::BatchMeta,
        size: zencodec::decode::StripDimensions,
        format: zenpixels::PixelFormat,
        metadata: &zencodec::decode::RowMetadata,
    ) -> Result<zenpixels::PixelViewMut<'_>, zencodec::decode::SinkError> {
        assert!(batch.is_final);
        self.metadata = Some(metadata.clone());
        self.provide_next_buffer(size.at_row(batch.y), format)
    }
}

#[test]
fn animation_sink_reports_exact_frame_completion() {
    use zencodec::decode::AnimationFrameDecoder;
    use zencodec::encode::AnimationFrameEncoder;

    let img = tk::TestImage::rgb8_gradient(16, 16);
    let durations = [
        Duration::new(1001, 30_000).unwrap(),
        Duration::new(1001, 15_000).unwrap(),
    ];
    let mut encoder = E::new()
        .with_lossless(true)
        .job()
        .animation_frame_encoder()
        .unwrap();
    for duration in durations {
        encoder
            .push_frame_exact(img.as_slice(), duration, None)
            .unwrap();
    }
    let encoded = encoder.finish(None).unwrap();
    let job = || {
        D::new()
            .job()
            .animation_frame_decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap()
    };
    let mut borrowed = job();
    let mut copied = job();
    let mut sink = CompletedRows::default();
    for (index, duration) in durations.into_iter().enumerate() {
        let expected = borrowed.render_next_frame(None).unwrap().unwrap();
        let result = copied
            .render_next_frame_to_sink(None, &mut sink)
            .unwrap()
            .unwrap();
        let frame = result.frame.unwrap();
        assert_eq!(frame.duration, duration);
        assert_eq!(frame.frame_index, index as u32);
        assert_eq!(frame.pts, expected.pts());
        assert_eq!(
            sink.pixels
                .as_ref()
                .unwrap()
                .as_view()
                .to_contiguous_bytes()
                .unwrap(),
            expected.pixels().to_contiguous_bytes().unwrap(),
        );
    }
    assert!(
        copied
            .render_next_frame_to_sink(None, &mut sink)
            .unwrap()
            .is_none()
    );
}
