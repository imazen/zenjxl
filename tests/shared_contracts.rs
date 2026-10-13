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
        (EncodePolicy::strip_all(), false),
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
        (DecodePolicy::strict(), false),
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
        zencodec::metadata_rewrite::Request::new(&input),
        zenjxl::metadata::rewrite,
        |out| {
            assert_eq!(out, clean, "image carrier bytes must not change");
            let a = zenjxl::decode(&input, None, &[]).unwrap();
            let b = zenjxl::decode(out, None, &[]).unwrap();
            assert_eq!(
                a.pixels.as_contiguous_bytes(),
                b.pixels.as_contiguous_bytes()
            );
            assert_eq!(a.info.icc_profile, b.info.icc_profile);
            Ok(())
        },
    )
    .unwrap();
    tk::check_metadata_rewrite(
        zencodec::metadata_rewrite::Request::new(&input).with_policy(MetadataPolicy::PreserveExact),
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
