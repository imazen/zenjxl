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
//! codestream wrapped into a container with a `jhgm` box.

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
    let buf = zenpixels::PixelBuf::from_typed::<rgb::Rgb<u8>>(
        pixels,
        w,
        h,
        zenpixels::ImageSpec::RGB8_SRGB,
    )
    .unwrap();
    zenjxl::JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .encoder()
        .unwrap()
        .encode(buf.as_view())
        .unwrap()
        .into_vec()
}

/// Container with a `jhgm` gain-map box wrapping the given ISO 21496-1 params.
fn jhgm_fixture_with(params: &zencodec::gainmap::GainMapParams) -> Vec<u8> {
    let base = encode_codestream(16, 16, 7);
    let gain_map_codestream = encode_codestream(8, 8, 101);

    bundle(&base, gain_map_codestream, params)
}

fn bundle(
    base: &[u8],
    gain_map_codestream: Vec<u8>,
    params: &zencodec::gainmap::GainMapParams,
) -> Vec<u8> {
    use zenjxl::{GainMapBundle, append_gain_map_box};
    let metadata = zencodec::gainmap::serialize_iso21496_fmt(
        params,
        zencodec::gainmap::Iso21496Format::JxlJhgm,
    )
    .unwrap();

    let bundle = GainMapBundle {
        metadata,
        color_encoding: None,
        alt_icc_compressed: None,
        gain_map_codestream,
    };
    append_gain_map_box(base, &bundle.serialize()).expect("container with jhgm box")
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

#[cfg(feature = "reconstruct-hdr")]
fn linear_base(
    dims: (u32, u32),
    values: &[f32],
    alpha: bool,
    primaries: jxl_encoder::Primaries,
    orientation: zencodec::Orientation,
) -> Vec<u8> {
    use jxl_encoder::{ColorEncoding, ImageMetadata, LosslessConfig, PixelLayout};
    let layout = if alpha {
        PixelLayout::RgbaLinearF32
    } else {
        PixelLayout::RgbLinearF32
    };
    let bytes: Vec<_> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let mut encoding = ColorEncoding::linear_srgb();
    encoding.primaries = primaries;
    LosslessConfig::default()
        .encode_request(dims.0, dims.1, layout)
        .with_metadata(&ImageMetadata::new().with_orientation(orientation))
        .with_color_encoding(encoding)
        .encode(&bytes)
        .unwrap()
}

#[cfg(feature = "reconstruct-hdr")]
fn gray_gain(w: u32, h: u32, codes: &[u8]) -> Vec<u8> {
    jxl_encoder::LosslessConfig::default()
        .encode_request(w, h, jxl_encoder::PixelLayout::Gray8)
        .encode(codes)
        .unwrap()
}

#[cfg(feature = "reconstruct-hdr")]
fn render(
    data: &[u8],
    mode: zencodec::GainMapRender,
    hint: zencodec::OrientationHint,
) -> Result<zencodec::decode::DecodeOutput, whereat::At<zencodec::CodecError>> {
    JxlDecoderConfig::new()
        .job()
        .with_orientation(hint)
        .with_gain_map_render(mode)
        .decoder(data.into(), &[])?
        .decode()
}

#[cfg(feature = "reconstruct-hdr")]
fn assert_f32_rows(out: &zencodec::decode::DecodeOutput, expected: &[f32]) {
    assert_eq!(
        out.pixels().spec().layout.sample,
        zenpixels::SampleType::F32
    );
    let bytes: Vec<_> = expected.iter().flat_map(|v| v.to_ne_bytes()).collect();
    assert_eq!(out.pixels().contiguous_bytes().unwrap().as_ref(), bytes);
}

// X1: independent lane equations at full weight and exact bypass at zero weight.
#[cfg(feature = "reconstruct-hdr")]
#[test]
fn reconstructs_mono_gain_using_all_three_iso_records() {
    use zencodec::{GainMapRender, Orientation, OrientationHint};
    assert!(JxlDecoderConfig::capabilities().reconstructs_hdr());
    let src = [0.5, 0.25, 0.75, 0.25, 0.5, 0.25];
    let base = linear_base(
        (2, 1),
        &src,
        false,
        jxl_encoder::Primaries::Srgb,
        Orientation::Identity,
    );
    let mut params = zencodec::gainmap::GainMapParams::default();
    params.alternate_hdr_headroom = 2.0;
    for (ch, (gain, base_offset, alternate_offset)) in params.channels.iter_mut().zip([
        (1.0, 0.125, 0.25),
        (0.0, 0.25, 0.125),
        (-1.0, 0.375, 0.0625),
    ]) {
        ch.min = gain;
        ch.max = gain;
        ch.base_offset = base_offset;
        ch.alternate_offset = alternate_offset;
    }
    let bytes = bundle(&base, gray_gain(2, 1, &[0, 255]), &params);
    let components = render(&bytes, GainMapRender::Components, OrientationHint::Preserve).unwrap();
    let gain = components
        .extras::<zencodec::decode::DecodedGainMap>()
        .unwrap();
    assert_eq!(gain.metadata.params.iso_channel_count(), Some(3));
    assert_eq!(gain.pixels.as_view().row(0), &[0, 255]);
    let zero = render(
        &bytes,
        GainMapRender::ReconstructHdr {
            target_headroom: Some(1.0),
        },
        OrientationHint::Preserve,
    )
    .unwrap();
    assert_f32_rows(
        &zero,
        &[src[0], src[1], src[2], 1.0, src[3], src[4], src[5], 1.0],
    );
    let full = render(
        &bytes,
        GainMapRender::ReconstructHdr {
            target_headroom: None,
        },
        OrientationHint::Preserve,
    )
    .unwrap();
    // (base + base_offset) * exp2(stops) - alternate_offset.
    assert_f32_rows(&full, &[1.0, 0.375, 0.5, 1.0, 0.5, 0.625, 0.25, 1.0]);
    assert!(full.extras::<zencodec::decode::DecodedGainMap>().is_none());
    assert!(full.extras::<zencodec::gainmap::GainMapSource>().is_none());
}

// X2: the shortcut preserves real PQ code precision, color, and HDR.
#[cfg(feature = "reconstruct-hdr")]
#[test]
#[allow(deprecated)] // JXL currently exposes best-effort metadata retention.
fn hdr_base_passthrough_preserves_pq_and_refuses_lower_target() {
    use zencodec::{Cicp, GainMapRender, OrientationHint};
    use zenpixels::{ContentLightLevel, ImageSpec, Primaries, Transfer};
    let values = [17u16, 32771, 60117, 64003, 9181, 1029];
    let rows: Vec<_> = values.into_iter().flat_map(u16::to_ne_bytes).collect();
    let pq = Cicp::new(9, 16, 0, true);
    let cll = ContentLightLevel::new(1700, 601);
    let mastering = zenpixels::hdr::MasteringDisplay::HDR10_REFERENCE;
    let spec = ImageSpec::RGB16_SRGB
        .with_primaries(Primaries::Bt2020)
        .with_transfer(Transfer::Pq);
    let pixels = zenpixels::PixelView::new(&rows, 2, 1, 12, &spec).unwrap();
    use zencodec::encode::{EncodeJob as _, EncoderConfig as _};
    let base = zenjxl::JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .with_metadata_policy(
            zencodec::Metadata::none()
                .with_cicp(pq)
                .with_content_light_level(cll)
                .with_mastering_display(mastering),
            zencodec::MetadataPolicy::PreserveExact,
        )
        .encode(pixels)
        .unwrap();
    let mut params = zencodec::gainmap::GainMapParams::default();
    params.base_hdr_headroom = 2.0;
    let data = bundle(base.data(), gray_gain(2, 1, &[9, 239]), &params);
    let base_only = render(&data, GainMapRender::BaseOnly, OrientationHint::Preserve).unwrap();
    for target in [None, Some(4.0)] {
        let output = render(
            &data,
            GainMapRender::ReconstructHdr {
                target_headroom: target,
            },
            OrientationHint::Preserve,
        )
        .unwrap();
        assert_eq!(output.pixels().row(0), rows);
        assert_eq!(output.pixels().row(0), base_only.pixels().row(0));
        assert_eq!(output.pixels().spec(), base_only.pixels().spec());
        assert_eq!(
            output.pixels().facts().depth,
            base_only.pixels().facts().depth
        );
        assert_eq!(output.pixels().facts().depth.unwrap().color_bits(), 16);
        assert_eq!(output.info().source_color.cicp, Some(pq));
        assert_eq!(output.pixels().hdr(), base_only.pixels().hdr());
        assert_eq!(output.pixels().hdr().cll, Some(cll));
        assert_eq!(output.pixels().hdr().mastering, Some(mastering));
        assert!(
            output
                .extras::<zencodec::decode::DecodedGainMap>()
                .is_none()
        );
    }
    let lower = render(
        &data,
        GainMapRender::ReconstructHdr {
            target_headroom: Some(2.0),
        },
        OrientationHint::Preserve,
    );
    assert_eq!(
        lower.unwrap_err().error().category(),
        zencodec::ErrorCategory::Request(zencodec::RequestError::Unsupported(
            zencodec::UnsupportedOperation::DecodePolicy
        )),
    );
}

// X3: register gains against the stored RGBA grid, and then rotate the result.
#[cfg(feature = "reconstruct-hdr")]
#[test]
fn rgba_gain_preserves_alpha_and_registered_samples() {
    use zencodec::{GainMapRender, Orientation, OrientationHint};
    let samples = [
        [0.25, 0.5, 0.75, 0.125],
        [0.625, 0.125, 0.25, 0.25],
        [0.375, 0.75, 0.125, 0.5],
        [0.75, 0.25, 0.5, 0.875],
    ];
    let base = linear_base(
        (2, 2),
        samples.as_flattened(),
        true,
        jxl_encoder::Primaries::Srgb,
        Orientation::Rotate90,
    );
    let gain_codes = [0, 255, 255, 255];
    let mut params = zencodec::gainmap::GainMapParams::default();
    params.alternate_hdr_headroom = 1.0;
    for ch in &mut params.channels {
        ch.max = 1.0;
        ch.base_offset = 0.0;
        ch.alternate_offset = 0.0;
    }
    let code = gray_gain(2, 2, &gain_codes);
    let data = bundle(&base, code.clone(), &params);
    let components = render(&data, GainMapRender::Components, OrientationHint::Preserve).unwrap();
    assert_f32_rows(&components, samples.as_flattened());
    let raw = components
        .extras::<zencodec::gainmap::GainMapSource>()
        .unwrap();
    assert_eq!(raw.data.as_slice(), code);
    let gain = components
        .extras::<zencodec::decode::DecodedGainMap>()
        .unwrap();
    assert_eq!(
        gain.pixels.as_view().contiguous_bytes().unwrap().as_ref(),
        gain_codes
    );

    let expected = [
        samples[0],
        [1.25, 0.25, 0.5, 0.25],
        [0.75, 1.5, 0.25, 0.5],
        [1.5, 0.5, 1.0, 0.875],
    ];
    let mode = GainMapRender::ReconstructHdr {
        target_headroom: None,
    };
    let stored = render(&data, mode, OrientationHint::Preserve).unwrap();
    assert_f32_rows(&stored, expected.as_flattened());
    assert_eq!(stored.info().orientation, Orientation::Rotate90);
    let upright = render(&data, mode, OrientationHint::Correct).unwrap();
    assert_f32_rows(
        &upright,
        [expected[2], expected[0], expected[3], expected[1]].as_flattened(),
    );
    assert_eq!(upright.info().orientation, Orientation::Identity);
}

// X4: native MaxRGB on a known saturated P3 raster; capacity is not a sample statistic.
#[cfg(feature = "reconstruct-hdr")]
#[test]
#[allow(deprecated)] // JXL currently exposes best-effort metadata retention.
fn measured_hdr_envelope_survives_native_encode() {
    use jxl_encoder::{ColorEncoding, LosslessConfig, PixelLayout, Primaries};
    use zencodec::encode::{EncodeJob as _, EncoderConfig as _};
    use zencodec::{GainMapRender, OrientationHint};
    let mut p3 = ColorEncoding::srgb();
    p3.primaries = Primaries::P3;
    let base = LosslessConfig::default()
        .encode_request(2, 1, PixelLayout::Rgb8)
        .with_color_encoding(p3)
        .encode(&[255, 0, 0, 0, 255, 0])
        .unwrap();
    let mut params = zencodec::gainmap::GainMapParams::default();
    params.alternate_hdr_headroom = 2.0;
    for ch in &mut params.channels {
        ch.max = 1.0;
        ch.base_offset = 0.0;
        ch.alternate_offset = 0.0;
    }
    let bytes = bundle(&base, gray_gain(2, 1, &[255, 0]), &params);
    let full = render(
        &bytes,
        GainMapRender::ReconstructHdr {
            target_headroom: None,
        },
        OrientationHint::Preserve,
    )
    .unwrap();
    let expected = [2.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
    assert_f32_rows(&full, &expected);
    // MaxRGB at 203 cd/m² per linear unit → max(406, 203), ceil(304.5).
    let cll = zenpixels::ContentLightLevel::new(406, 305);
    let mastering = zenpixels::hdr::MasteringDisplay::new(
        [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
        [0.3127, 0.3290],
        812.0,
        0.0,
    );
    assert_eq!(
        full.info().source_color.diffuse_white,
        Some(zenpixels::DiffuseWhite::BT2408)
    );
    let native = zenjxl::JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .with_metadata_policy(
            zencodec::Metadata::from(full.info()),
            zencodec::MetadataPolicy::PreserveExact,
        )
        .encode(full.pixels())
        .unwrap();
    assert_eq!(
        zenjxl::probe(native.data()).unwrap().intensity_target,
        812.0
    );
    let back = render(
        native.data(),
        GainMapRender::BaseOnly,
        OrientationHint::Preserve,
    )
    .unwrap();
    assert_f32_rows(&back, &expected);
    for output in [&full, &back] {
        assert_eq!(output.info().source_color.content_light_level, Some(cll));
        assert_eq!(
            output.info().source_color.mastering_display,
            Some(mastering)
        );
        assert_eq!(output.pixels().hdr().cll, Some(cll));
        assert_eq!(output.pixels().hdr().mastering, Some(mastering));
    }
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

#[cfg(feature = "reconstruct-hdr")]
#[test]
fn reconstructs_the_stored_gain_grid_before_correcting_orientation() {
    use zencodec::{GainMapRender, Orientation, OrientationHint};
    use zenjxl::{GainMapBundle, append_gain_map_box};
    let initial = jhgm_fixture();
    let bundle: GainMapBundle = zenjxl::decode(&initial, None, &[])
        .unwrap()
        .gain_map
        .unwrap();
    let pixels: Vec<u8> = (0..17 * 9 * 3).map(|i| (61 * i + 20) as u8).collect();
    let metadata = jxl_encoder::ImageMetadata::new().with_orientation(Orientation::Transpose);
    let base = jxl_encoder::LosslessConfig::default()
        .encode_request(17, 9, jxl_encoder::PixelLayout::Rgb8)
        .with_metadata(&metadata)
        .encode(&pixels)
        .unwrap();
    let bytes = append_gain_map_box(&base, &bundle.serialize()).unwrap();
    let render = |hint| {
        JxlDecoderConfig::new()
            .job()
            .with_orientation(hint)
            .with_gain_map_render(GainMapRender::ReconstructHdr {
                target_headroom: None,
            })
            .decoder(bytes.as_slice().into(), &[])
            .unwrap()
            .decode()
            .unwrap()
    };
    let stored = render(OrientationHint::Preserve).into_buffer();
    let corrected = render(OrientationHint::Correct).into_buffer();
    let expected =
        zenpixels_convert::orient::apply_orientation(&stored.as_view(), Orientation::Transpose)
            .unwrap();
    assert_eq!((corrected.width(), corrected.height()), (9, 17));
    assert_eq!(
        corrected.as_view().as_contiguous_bytes(),
        expected.as_view().as_contiguous_bytes()
    );
    assert_eq!(corrected.hdr(), stored.hdr());
    assert_eq!(corrected.origin(), stored.origin());
}
