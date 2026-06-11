//! Exercises `zencodec::GainMapRender` through the `JxlDecodeJob` trait path.
//!
//! `Components` decodes the jhgm gain-map codestream into a
//! [`zencodec::decode::DecodedGainMap`] (plus the raw `GainMapSource`);
//! `BaseOnly` (default) attaches nothing. `ReconstructHdr` applies the gain
//! map natively when the `reconstruct-hdr` feature is on (SDR-base bundles →
//! linear f32 HDR + envelope; HDR-base bundles → the base as-is), and
//! downgrades to surfacing Components when it is off.
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

/// Container with a `jhgm` gain-map box wrapping the given ISO 21496-1 params.
fn jhgm_fixture_with(params: &zencodec::gainmap::GainMapParams) -> Vec<u8> {
    use jxl_encoder::hdr::{GainMapBundle, append_gain_map_bundle};

    let base = encode_codestream(16, 16, 7);
    let gain_map_codestream = encode_codestream(8, 8, 101);

    let metadata = zencodec::gainmap::serialize_iso21496_fmt(
        params,
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

/// SDR-base fixture: 2x (1 stop) alternate headroom, real per-channel gain.
fn jhgm_fixture() -> Vec<u8> {
    let mut params = zencodec::gainmap::GainMapParams::default();
    params.alternate_hdr_headroom = 1.0; // log2 → 2x peak; base 0.0 → BaseIsSdr
    for ch in &mut params.channels {
        ch.max = 1.0; // log2 → up to 2x gain where the map is at full scale
    }
    jhgm_fixture_with(&params)
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

/// Without the `reconstruct-hdr` feature, ReconstructHdr downgrades to
/// Components: zenjxl surfaces instead of applying, and the base stays
/// labeled per its own descriptor.
#[cfg(not(feature = "reconstruct-hdr"))]
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

/// Max linear value over every f32 channel (RGBA — alpha is ≤ 1 so it never
/// wins) of a decoded output.
#[cfg(feature = "reconstruct-hdr")]
fn max_linear_f32(out: &zencodec::decode::DecodeOutput) -> f32 {
    out.pixels()
        .contiguous_bytes()
        .chunks_exact(4)
        .map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        .fold(0.0f32, f32::max)
}

/// With the `reconstruct-hdr` feature, an SDR-base bundle gets the gain map
/// applied: linear f32 output brighter than SDR white, with the
/// content-light-level / mastering-display envelope populated, and no
/// Components extras (the gain map was consumed).
#[cfg(feature = "reconstruct-hdr")]
#[test]
fn reconstruct_applies_gain_map_on_sdr_base() {
    assert!(
        <JxlDecoderConfig as zencodec::decode::DecoderConfig>::capabilities().reconstructs_hdr()
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

    let desc = out.pixels().descriptor();
    assert_eq!(desc.channel_type(), zenpixels::ChannelType::F32);
    assert_eq!(desc.transfer(), zenpixels::TransferFunction::Linear);

    let max = max_linear_f32(&out);
    assert!(max > 1.05, "expected headroom above SDR white, max={max}");

    let info = out.info();
    let cll = info
        .source_color
        .content_light_level
        .expect("ReconstructHdr must populate the content light level");
    assert!(cll.max_content_light_level > 203);
    assert!(info.source_color.mastering_display.is_some());

    assert!(out.extras::<zencodec::decode::DecodedGainMap>().is_none());
    assert!(out.extras::<zencodec::gainmap::GainMapSource>().is_none());
}

/// `target_headroom: Some(1.0)` clamps the boost to SDR — linear output stays
/// at (or barely above) SDR white.
#[cfg(feature = "reconstruct-hdr")]
#[test]
fn reconstruct_honors_target_headroom() {
    let data = jhgm_fixture();
    let out = JxlDecoderConfig::new()
        .job()
        .with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
            target_headroom: Some(1.0),
        })
        .decoder(data.as_slice().into(), &[])
        .unwrap()
        .decode()
        .unwrap();
    let max = max_linear_f32(&out);
    assert!(
        max <= 1.01,
        "boost clamped to 1.0 must not exceed SDR white, max={max}"
    );
}

/// An HDR-base bundle (alternate is the SDR rendition) needs nothing applied:
/// ReconstructHdr returns the base exactly as BaseOnly would.
#[cfg(feature = "reconstruct-hdr")]
#[test]
fn reconstruct_returns_base_when_base_is_hdr() {
    let mut params = zencodec::gainmap::GainMapParams::default();
    params.base_hdr_headroom = 2.0; // base brighter than alternate → BaseIsHdr
    params.alternate_hdr_headroom = 0.0;
    let data = jhgm_fixture_with(&params);

    let reconstructed = JxlDecoderConfig::new()
        .job()
        .with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
            target_headroom: None,
        })
        .decoder(data.as_slice().into(), &[])
        .unwrap()
        .decode()
        .unwrap();
    let base_only = JxlDecoderConfig::new()
        .job()
        .decoder(data.as_slice().into(), &[])
        .unwrap()
        .decode()
        .unwrap();

    assert_eq!(
        reconstructed.pixels().descriptor(),
        base_only.pixels().descriptor(),
        "HDR-base reconstruction must not re-render the base"
    );
    assert_eq!(
        reconstructed.pixels().contiguous_bytes(),
        base_only.pixels().contiguous_bytes(),
    );
    assert!(
        reconstructed
            .extras::<zencodec::decode::DecodedGainMap>()
            .is_none()
    );
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
