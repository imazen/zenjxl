//! What a JXL encode says about its pixels when the caller's metadata says
//! nothing, and what an Exif box holds.

#![cfg(all(feature = "encode", feature = "decode", feature = "zencodec"))]

use std::borrow::Cow;

use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zencodec::{Metadata, MetadataPolicy};
use zenjxl::{JxlDecoderConfig, JxlEncoderConfig};
use zenpixels::{PixelDescriptor, PixelSlice};

fn encode(desc: PixelDescriptor, meta: Option<Metadata>) -> Vec<u8> {
    let (w, h) = (8u32, 4u32);
    let bpp = desc.bytes_per_pixel();
    let bytes: Vec<u8> = (0..w as usize * h as usize * bpp)
        .map(|i| (i * 37) as u8)
        .collect();
    let slice = PixelSlice::new(&bytes, w, h, w as usize * bpp, desc).unwrap();
    let mut job = JxlEncoderConfig::new().with_generic_quality(90.0).job();
    if let Some(m) = meta {
        job = job.with_metadata_policy(m, MetadataPolicy::PreserveExact);
    }
    job.encoder().unwrap().encode(slice).unwrap().into_vec()
}

fn decode(bytes: &[u8]) -> zencodec::decode::DecodeOutput {
    JxlDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(bytes), &[])
        .unwrap()
        .decode()
        .unwrap()
}

/// PQ and HLG pixels with no colour metadata were tagged sRGB.
#[test]
fn hdr_and_wide_gamut_descriptors_are_signalled_without_metadata() {
    for (desc, primaries, transfer) in [
        (PixelDescriptor::RGB16_BT2100_PQ, 9, 16),
        (PixelDescriptor::RGB16_BT2100_HLG, 9, 18),
        (
            PixelDescriptor::RGB8_SRGB.with_primaries(zenpixels::ColorPrimaries::DisplayP3),
            12,
            13,
        ),
    ] {
        let out = decode(&encode(desc, None));
        let cicp = out
            .info()
            .source_color
            .cicp
            .unwrap_or_else(|| panic!("{desc:?}: no colour signalled"));
        assert_eq!(
            (cicp.color_primaries, cicp.transfer_characteristics),
            (primaries, transfer),
            "{desc:?}"
        );
    }
}

/// A JPEG-style `Exif\0\0` prefix doesn't reach the Exif box.
#[test]
fn a_jpeg_exif_prefix_is_not_boxed() {
    #[allow(deprecated)] // the only EXIF writer zencodec 0.1.26 has
    let tiff = {
        let mut e = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
        e.set_copyright("(c) test");
        e.to_bytes()
    };
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    let out = decode(&encode(
        PixelDescriptor::RGB8_SRGB,
        Some(Metadata::none().with_exif(app1)),
    ));
    let exif = out.metadata().exif.expect("EXIF kept");
    assert_eq!(&exif[..], &tiff[..], "the box holds the TIFF stream");
}
