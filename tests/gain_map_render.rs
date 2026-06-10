//! Exercises `zencodec::GainMapRender` through the `JxlDecodeJob` trait path.
//!
//! zenjxl surfaces gain maps but does not apply them (`reconstructs_hdr()` is
//! `false`): `Components` decodes the jhgm gain-map codestream into a
//! [`zencodec::decode::DecodedGainMap`] (plus the raw `GainMapSource`);
//! `ReconstructHdr` downgrades to surfacing per the zencodec contract;
//! `BaseOnly` (default) attaches nothing.
//!
//! The fixture is built in-test: base JXL codestream + a tiny gray gain-map
//! codestream wrapped into a container with a `jhgm` box via jxl-encoder's
//! `hdr-gainmap` machinery.

#![cfg(all(feature = "zencodec", feature = "encode", feature = "decode"))]

use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
use zenjxl::JxlDecoderConfig;

/// Encode a small RGB image to a JXL codestream via the zencodec trait path.
fn encode_codestream(w: u32, h: u32, seed: u8) -> Vec<u8> {
    use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
    let pixels: Vec<rgb::Rgb<u8>> = (0..w * h)
        .map(|i| {
            let v = ((i as u8).wrapping_mul(31)).wrapping_add(seed);
            rgb::Rgb { r: v, g: v, b: v }
        })
        .collect();
    let buf = zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, w, h).unwrap();
    zenjxl::JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .encoder()
        .unwrap()
        .encode(buf.as_slice().into())
        .unwrap()
        .into_vec()
}

/// Container with a `jhgm` gain-map box: base image + gray gain map + ISO
/// 21496-1 params with a real (2x = 1 stop) alternate headroom.
fn jhgm_fixture() -> Vec<u8> {
    use jxl_encoder::hdr::{GainMapBundle, append_gain_map_bundle};

    let base = encode_codestream(16, 16, 7);
    let gain_map_codestream = encode_codestream(8, 8, 101);

    let mut params = zencodec::gainmap::GainMapParams::default();
    params.alternate_hdr_headroom = 1.0; // log2 → 2x peak
    let metadata = zencodec::gainmap::serialize_iso21496_fmt(
        &params,
        zencodec::gainmap::Iso21496Format::JxlJhgm,
    );

    let bundle = GainMapBundle {
        jhgm_version: 0,
        iso21496_metadata: metadata,
        color_encoding: None,
        alt_icc: Vec::new(),
        gain_map_codestream,
    };
    append_gain_map_bundle(&base, &bundle).expect("container with jhgm box")
}

/// Default (BaseOnly): no gain-map extras at all.
#[test]
fn base_only_attaches_nothing() {
    let data = jhgm_fixture();
    let out = JxlDecoderConfig::new()
        .job()
        .decoder(data.as_slice().into(), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert!(out.extras::<zencodec::gainmap::GainMapSource>().is_none());
    assert!(out.extras::<zencodec::decode::DecodedGainMap>().is_none());
}

/// Components: both the raw `GainMapSource` and the DECODED gain map surface.
#[test]
fn components_surfaces_decoded_gain_map() {
    let data = jhgm_fixture();
    let out = JxlDecoderConfig::new()
        .job()
        .with_gain_map_render(zencodec::GainMapRender::Components)
        .decoder(data.as_slice().into(), &[])
        .unwrap()
        .decode()
        .unwrap();

    let src = out
        .extras::<zencodec::gainmap::GainMapSource>()
        .expect("Components must surface the raw GainMapSource");
    assert_eq!(src.format, zencodec::ImageFormat::Jxl);
    assert!(!src.data.is_empty());

    let dgm = out
        .extras::<zencodec::decode::DecodedGainMap>()
        .expect("Components must surface the DecodedGainMap");
    assert_eq!(dgm.pixels.width(), 8);
    assert_eq!(dgm.pixels.height(), 8);
    assert!(
        (dgm.metadata.params.alternate_hdr_headroom - 1.0).abs() < 1e-3,
        "ISO 21496-1 headroom must round-trip"
    );
}

/// ReconstructHdr downgrades to Components: zenjxl never applies the gain map
/// itself, and the base stays labeled per its own descriptor.
#[test]
fn reconstruct_downgrades_to_components() {
    assert!(
        !<JxlDecoderConfig as zencodec::decode::DecoderConfig>::capabilities().reconstructs_hdr()
    );
    let data = jhgm_fixture();
    let out = JxlDecoderConfig::new()
        .job()
        .with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
            target_headroom: None,
        })
        .decoder(data.as_slice().into(), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert!(out.extras::<zencodec::decode::DecodedGainMap>().is_some());
}

/// Components on a plain (no-jhgm) JXL surfaces nothing and decodes normally.
#[test]
fn components_on_plain_jxl_is_clean() {
    let data = encode_codestream(16, 16, 3);
    let out = JxlDecoderConfig::new()
        .job()
        .with_gain_map_render(zencodec::GainMapRender::Components)
        .decoder(data.as_slice().into(), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert!(out.extras::<zencodec::decode::DecodedGainMap>().is_none());
    assert_eq!(out.pixels().width(), 16);
}
