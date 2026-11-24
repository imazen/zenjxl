//! JXL decoding and probing via jxl-rs.

use alloc::vec;
use alloc::vec::Vec;

use zenpixels::{
    AlphaMode, ByteStorageMut, ChannelOrder, Cicp, ColorOrigin, ColorSpace, CustomPrimaries, Gamma,
    HdrMetadata, ImageSpec, PixelBuf, PixelStorage, Primaries, RenderingIntent, SampleLayout,
    SampleType, SignalRange, Transfer,
};

use alloc::string::{String, ToString};

use jxl::api::{
    ExtraChannel, GainMapBundle, JxlBitDepth, JxlColorEncoding, JxlColorProfile, JxlColorType,
    JxlDecoder, JxlDecoderOptions, JxlOutputBuffer, JxlPixelFormat, ProcessingResult,
};

use crate::error::JxlError;

type At<E> = whereat::At<E>;

/// Semantic type of a JXL extra channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JxlExtraChannelType {
    /// Alpha / transparency channel.
    Alpha,
    /// Depth map.
    Depth,
    /// Spot color (CMYK-style or custom ink).
    SpotColor,
    /// Selection mask for compositing.
    SelectionMask,
    /// Key (black) channel, typically for CMYK.
    Black,
    /// Color filter array (Bayer pattern for raw sensors).
    Cfa,
    /// Thermal / infrared data.
    Thermal,
    /// Optional channel (decoder may ignore).
    Optional,
    /// Unrecognized or reserved channel type.
    Unknown(u32),
}

impl JxlExtraChannelType {
    fn from_jxl(ec: &ExtraChannel) -> Self {
        match ec {
            ExtraChannel::Alpha => Self::Alpha,
            ExtraChannel::Depth => Self::Depth,
            ExtraChannel::SpotColor => Self::SpotColor,
            ExtraChannel::SelectionMask => Self::SelectionMask,
            ExtraChannel::Black => Self::Black,
            ExtraChannel::CFA => Self::Cfa,
            ExtraChannel::Thermal => Self::Thermal,
            ExtraChannel::Optional => Self::Optional,
            ExtraChannel::Unknown => Self::Unknown(15),
            // Reserved variants map to Unknown with their discriminant
            ExtraChannel::Reserved0 => Self::Unknown(7),
            ExtraChannel::Reserved1 => Self::Unknown(8),
            ExtraChannel::Reserved2 => Self::Unknown(9),
            ExtraChannel::Reserved3 => Self::Unknown(10),
            ExtraChannel::Reserved4 => Self::Unknown(11),
            ExtraChannel::Reserved5 => Self::Unknown(12),
            ExtraChannel::Reserved6 => Self::Unknown(13),
            ExtraChannel::Reserved7 => Self::Unknown(14),
        }
    }
}

/// Metadata for a single JXL extra channel.
#[derive(Clone, Debug)]
pub struct JxlExtraChannelInfo {
    /// Semantic type of this channel.
    pub channel_type: JxlExtraChannelType,
    /// Bits per sample for this channel.
    pub bits_per_sample: u8,
    /// Channel name, if the encoder provided one. `None` when unnamed.
    pub name: Option<String>,
    /// Whether alpha is premultiplied (only meaningful for Alpha channels).
    pub alpha_associated: bool,
    /// Dimensional shift (0 = full resolution, 1 = half, 2 = quarter, 3 = eighth).
    pub dim_shift: u8,
}

/// JXL image metadata from probing.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct JxlInfo {
    /// Emitted image width in pixels — the buffer the decoder will produce.
    ///
    /// In the default (orientation-adjusting) decode this is the *display*
    /// width; when orientation adjustment is disabled this equals
    /// [`coded_width`](Self::coded_width). Allocate output buffers against this.
    pub width: u32,
    /// Emitted image height in pixels — see [`width`](Self::width).
    pub height: u32,
    /// Stored (coded) width as written in the codestream, before any
    /// orientation is applied. Unaffected by orientation adjustment; differs
    /// from [`width`](Self::width) for axis-swapping orientations.
    pub coded_width: u32,
    /// Stored (coded) height — see [`coded_width`](Self::coded_width).
    pub coded_height: u32,
    /// Whether the image has an alpha channel.
    pub has_alpha: bool,
    /// Whether the image contains animation.
    pub has_animation: bool,
    /// Bits per sample (e.g. 8, 16, 32).
    pub bit_depth: Option<u8>,
    /// Embedded ICC color profile.
    pub icc_profile: Option<Vec<u8>>,
    /// Residual EXIF orientation (1-8) of the *emitted* pixels — the transform
    /// a caller must still apply to display the image upright. 1 = Normal.
    ///
    /// In the default (orientation-adjusting) decode the stored orientation is
    /// already baked into the pixels, so this is `1` (Identity). When
    /// orientation adjustment is disabled this equals
    /// [`intrinsic_orientation`](Self::intrinsic_orientation).
    pub orientation: u8,
    /// The image's intrinsic (stored) EXIF orientation (1-8) as written in the
    /// codestream, regardless of whether orientation was adjusted during decode.
    ///
    /// Use this to bake the stored orientation later (Preserve mode) or to
    /// re-tag re-encoded output. In the default decode the pixels are already
    /// upright even though this may report a non-Identity value.
    pub intrinsic_orientation: u8,
    /// CICP color description `(color_primaries, transfer_characteristics, matrix_coefficients, full_range)`.
    ///
    /// Derived from JXL's structured color encoding when the image does not use
    /// an ICC profile. `None` for ICC-profiled images or custom color spaces.
    pub cicp: Option<(u8, u8, u8, bool)>,
    /// Whether the image's color encoding is grayscale.
    pub is_gray: bool,
    /// Raw EXIF data from the `Exif` container box (TIFF header offset stripped).
    /// `None` for bare codestreams or files without an `Exif` box.
    pub exif: Option<Vec<u8>>,
    /// Raw XMP data from the `xml ` container box.
    /// `None` for bare codestreams or files without an `xml ` box.
    pub xmp: Option<Vec<u8>>,
    /// Extra channels beyond the color channels (alpha, depth, spot color, etc.).
    pub extra_channels: Vec<JxlExtraChannelInfo>,
    /// Preview image dimensions `(width, height)`, if the file contains a preview frame.
    ///
    /// JXL files can embed a small preview image for quick thumbnailing.
    /// `None` when no preview is present.
    pub preview_size: Option<(u32, u32)>,
    /// Whether the image uses XYB color space transform (VarDCT lossy encoding).
    ///
    /// When `false` (`uses_original_profile` in the spec), the image uses the
    /// modular pathway, which is the lossless encoding mode in JPEG XL.
    /// This is exposed as `!basic_info.uses_original_profile` from jxl-rs.
    pub xyb_encoded: bool,
    /// Peak display luminance the content was mastered for, in nits (cd/m²).
    ///
    /// Default is 255.0 (SDR). Higher values (e.g. 4000, 10000) indicate HDR content.
    /// From the JXL codestream `ToneMapping.intensity_target`.
    pub intensity_target: f32,
    pub(crate) hdr: HdrMetadata,
    /// Minimum display luminance in nits. Default is 0.0.
    ///
    /// From the JXL codestream `ToneMapping.min_nits`.
    pub min_nits: f32,
    /// Whether `linear_below` is relative to `intensity_target` (true) or absolute nits (false).
    ///
    /// From the JXL codestream `ToneMapping.relative_to_max_display`.
    pub relative_to_max_display: bool,
    /// Below this value, the transfer function is linear rather than the signaled TF.
    ///
    /// Interpretation depends on `relative_to_max_display`. Default is 0.0.
    /// From the JXL codestream `ToneMapping.linear_below`.
    pub linear_below: f32,
    /// Intrinsic display size `(width, height)`, if different from coded dimensions.
    ///
    /// When present, the image should be rendered at this size rather than
    /// `(width, height)`. `None` when the coded size is the intended display size.
    pub intrinsic_size: Option<(u32, u32)>,
}

impl zencodec::SourceEncodingDetails for JxlInfo {
    fn source_generic_quality(&self) -> Option<f32> {
        // JXL headers don't expose the encoding quality/distance.
        None
    }

    fn is_lossless(&self) -> bool {
        // JXL lossless images use the modular pathway (original color profile,
        // no XYB transform). `!xyb_encoded` is equivalent to
        // `uses_original_profile` in the spec. All lossless JXL images have
        // this flag; VarDCT (lossy) always uses XYB. Modular lossy exists but
        // is extremely rare in practice, so this is the best header-level signal.
        !self.xyb_encoded
    }
}

/// JXL decode output.
#[derive(Debug)]
pub struct JxlDecodeOutput {
    /// Decoded pixel data.
    pub pixels: PixelBuf,
    /// Image metadata.
    pub info: JxlInfo,
    /// HDR gain map bundle from `jhgm` container box (ISO 21496-1).
    ///
    /// Present when the JXL file contains a gain map for HDR/SDR adaptation.
    /// ISO 21496-1 metadata determines whether the base is SDR or HDR.
    pub gain_map: Option<GainMapBundle>,
}

/// Decode limits for JXL operations.
#[derive(Clone, Debug, Default)]
pub struct JxlLimits {
    /// Maximum total pixels (width * height).
    pub max_pixels: Option<u64>,
    /// Maximum memory allocation in bytes.
    pub max_memory_bytes: Option<u64>,
}

impl JxlLimits {
    fn validate(&self, width: u32, height: u32, bytes_per_pixel: u32) -> Result<(), JxlError> {
        if let Some(max_px) = self.max_pixels {
            let pixels = width as u64 * height as u64;
            if pixels > max_px {
                return Err(JxlError::LimitExceeded(
                    "pixel count exceeds limit".into(),
                    zencodec::LimitKind::Pixels,
                ));
            }
        }
        if let Some(max_mem) = self.max_memory_bytes {
            let estimated = width as u64 * height as u64 * bytes_per_pixel as u64;
            if estimated > max_mem {
                return Err(JxlError::LimitExceeded(
                    "estimated memory exceeds limit".into(),
                    zencodec::LimitKind::Memory,
                ));
            }
        }
        Ok(())
    }
}

use jxl::api::{JxlPrimaries, JxlTransferFunction, JxlWhitePoint};

pub(crate) fn map_err(e: jxl::api::Error) -> JxlError {
    // Surface a policy rejection as a dedicated variant so callers can match on
    // it without string-matching the decoder error. The decoder raises this when
    // `reject_progressive` is set and a progressive frame header is seen.
    if matches!(e, jxl::api::Error::ProgressiveRejected) {
        return JxlError::ProgressiveRejected;
    }
    JxlError::Decode(e)
}

/// Convert a header-reported dimension (`usize`) to `u32` with a checked cast.
///
/// JXL dimensions max out at 2^30 per spec, so a header value that does not
/// fit in `u32` is malformed input — the value came from decoding the
/// bitstream, not from a caller-supplied parameter, so a failure here is
/// image-bytes-origin ([`JxlError::MalformedImage`], not
/// [`JxlError::InvalidInput`]). On 32-bit targets the conversion is always a
/// no-op; on 64-bit targets it catches header forgeries.
pub(crate) fn dim_to_u32(value: usize, label: &'static str) -> Result<u32, JxlError> {
    u32::try_from(value).map_err(|_| {
        JxlError::MalformedImage(alloc::format!("JXL: {label} dimension {value} exceeds u32"))
    })
}

/// Compute a `usize` buffer size as `width * channels * bytes_per_sample * height`
/// using checked multiplication, returning an `OutOfMemory` error on overflow
/// (a size computation overflowing is not a caller-configured limit).
///
/// Used in the decoder to size the per-frame pixel buffer; a wraparound here
/// would silently allocate a too-small buffer.
pub(crate) fn checked_buf_size(
    width: usize,
    height: usize,
    channels: usize,
    bytes_per_sample: usize,
) -> Result<(usize, usize), JxlError> {
    let bytes_per_row = width
        .checked_mul(channels)
        .and_then(|v| v.checked_mul(bytes_per_sample))
        .ok_or_else(|| JxlError::OutOfMemory("bytes_per_row overflow".into()))?;
    let buf_size = bytes_per_row
        .checked_mul(height)
        .ok_or_else(|| JxlError::OutOfMemory("frame buffer size overflow".into()))?;
    Ok((bytes_per_row, buf_size))
}

/// Extract ICC profile and CICP from JXL color profile.
#[allow(clippy::type_complexity)]
pub(crate) fn extract_color_info(
    profile: &JxlColorProfile,
) -> (Option<Vec<u8>>, Option<(u8, u8, u8, bool)>) {
    match profile {
        JxlColorProfile::Icc(icc_bytes) => (Some(icc_bytes.clone()), None),
        JxlColorProfile::Simple(encoding) => {
            let cicp = jxl_encoding_to_cicp(encoding);
            // Try to synthesize an ICC profile from the structured encoding
            let icc = profile.try_as_icc().map(|cow| cow.into_owned());
            (icc, cicp)
        }
    }
}

/// Map JXL structured color encoding to CICP code points.
fn jxl_encoding_to_cicp(encoding: &JxlColorEncoding) -> Option<(u8, u8, u8, bool)> {
    match encoding {
        JxlColorEncoding::RgbColorSpace {
            primaries,
            transfer_function,
            white_point,
            ..
        } => {
            let cp = match (primaries, white_point) {
                (JxlPrimaries::SRGB, JxlWhitePoint::D65) => 1,
                (JxlPrimaries::BT2100, JxlWhitePoint::D65) => 9,
                (JxlPrimaries::P3, JxlWhitePoint::DCI) => 11,
                (JxlPrimaries::P3, JxlWhitePoint::D65) => 12,
                _ => return None,
            };
            let tc = transfer_to_cicp(transfer_function)?;
            // JXL is always full range RGB, matrix = Identity (0)
            Some((cp, tc, 0, true))
        }
        JxlColorEncoding::GrayscaleColorSpace {
            transfer_function, ..
        } => {
            let tc = transfer_to_cicp(transfer_function)?;
            // Grayscale: BT.709 primaries, Identity matrix
            Some((1, tc, 0, true))
        }
        JxlColorEncoding::XYB { .. } => None,
    }
}

fn transfer_to_cicp(tf: &JxlTransferFunction) -> Option<u8> {
    Some(match tf {
        JxlTransferFunction::BT709 => 1,
        JxlTransferFunction::SRGB => 13,
        JxlTransferFunction::Linear => 8,
        JxlTransferFunction::PQ => 16,
        JxlTransferFunction::HLG => 18,
        JxlTransferFunction::DCI => 17,
        JxlTransferFunction::Gamma(_) => return None,
    })
}

/// Returns true if CICP indicates HDR transfer (PQ/HLG) or wide gamut primaries
/// (BT.2020/P3). These signals mean values outside [0, 1] may be intentional.
pub(crate) fn is_hdr_or_wide_gamut(cicp: Option<(u8, u8, u8, bool)>) -> bool {
    let Some((cp, tc, _, _)) = cicp else {
        return false;
    };
    // PQ = 16, HLG = 18
    let hdr_transfer = matches!(tc, 16 | 18);
    // BT.2020 = 9, P3 = 11 | 12
    let wide_gamut = matches!(cp, 9 | 11 | 12);
    hdr_transfer || wide_gamut
}

/// Clamp all f32 values in a byte buffer to [0.0, 1.0].
pub(crate) fn clamp_f32_buf(buf: &mut [u8]) {
    for chunk in buf.as_chunks_mut::<4>().0.iter_mut() {
        let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let clamped = v.clamp(0.0, 1.0);
        if v != clamped {
            chunk.copy_from_slice(&clamped.to_ne_bytes());
        }
    }
}

/// Check if a JXL color profile indicates grayscale.
pub(crate) fn profile_is_grayscale(profile: &JxlColorProfile) -> bool {
    matches!(
        profile,
        JxlColorProfile::Simple(JxlColorEncoding::GrayscaleColorSpace { .. })
    )
}

/// Chosen output format for the JXL decoder.
#[derive(Clone, Debug)]
pub(crate) struct ChosenFormat {
    /// The JXL pixel format to request from the decoder.
    pub(crate) pixel_format: JxlPixelFormat,
    /// The color type we requested (for buffer interpretation).
    pub(crate) color_type: JxlColorType,
    /// The channel type we're decoding into.
    pub(crate) channel_type: SampleType,
}

/// Choose the output pixel format based on the image's native properties and
/// the caller's preference list.
///
/// If `preferred` is non-empty, picks the first descriptor we can produce without
/// lossy conversion. If empty, returns the native format (matching bit depth).
pub(crate) fn choose_pixel_format(
    bit_depth: &JxlBitDepth,
    has_alpha: bool,
    is_gray: bool,
    num_extra: usize,
    preferred: &[ImageSpec],
) -> ChosenFormat {
    let is_float = matches!(bit_depth, JxlBitDepth::Float { .. });
    let bps = bit_depth.bits_per_sample();

    // Determine native channel type (what we can produce losslessly)
    let native_channel_type = if is_float || bps > 16 {
        SampleType::F32
    } else if bps > 8 {
        SampleType::U16
    } else {
        SampleType::U8
    };

    // Determine native channel layout
    let native_layout = match (is_gray, has_alpha) {
        (true, false) => ChannelOrder::Gray,
        (true, true) => ChannelOrder::GrayAlpha,
        (false, false) => ChannelOrder::Rgb,
        (false, true) => ChannelOrder::Rgba,
    };

    // If preferred list is non-empty, find the first we can produce losslessly.
    // "Losslessly" means: we don't drop precision (channel_type >= native)
    // and we don't discard channels the caller wants.
    if !preferred.is_empty() {
        for desc in preferred {
            // Can we produce this channel type without precision loss?
            if !desc.layout.packing.is_full() {
                continue;
            }
            let ct = desc.layout.sample;
            if !can_produce_losslessly(native_channel_type, ct) {
                continue;
            }
            // Can we produce this layout from the native data?
            if !layout_compatible(native_layout, desc.layout.order) {
                continue;
            }
            // Only allow grayscale output when the source is actually grayscale.
            // XYB-encoded JXL files have 3 color channels internally even if
            // the color profile says "gray", and jxl-rs rejects grayscale output
            // for 3-channel images.
            if (desc.layout.order == ChannelOrder::Gray
                || desc.layout.order == ChannelOrder::GrayAlpha)
                && !is_gray
            {
                continue;
            }
            return build_chosen(ct, desc.layout.order, has_alpha, num_extra);
        }
    }

    // Default: native precision, but always use RGB/RGBA layout.
    // JXL's XYB encoding uses 3 color channels internally regardless of whether
    // the source was grayscale, and jxl-rs rejects grayscale output for 3-channel
    // images. Only use grayscale when both the profile says gray AND the caller
    // explicitly requests it (handled above).
    let default_layout = if is_gray {
        if has_alpha {
            ChannelOrder::GrayAlpha
        } else {
            ChannelOrder::Gray
        }
    } else if has_alpha {
        ChannelOrder::Rgba
    } else {
        ChannelOrder::Rgb
    };
    build_chosen(native_channel_type, default_layout, has_alpha, num_extra)
}

/// Returns true if we can produce `target` type from `native` without lossy conversion.
fn can_produce_losslessly(native: SampleType, target: SampleType) -> bool {
    match native {
        SampleType::U8 => matches!(target, SampleType::U8 | SampleType::U16 | SampleType::F32),
        SampleType::U16 => matches!(target, SampleType::U16 | SampleType::F32),
        SampleType::F32 => matches!(target, SampleType::F32),
        _ => false,
    }
}

/// Returns true if we can produce `target` layout from `native` layout.
///
/// Supported conversions:
/// - Same layout (trivial)
/// - RGB → Gray (luminance extraction — NOT lossless but the decoder handles it internally)
/// - Gray → RGB (replicate channels — lossless)
/// - RGB ↔ RGBA (add/drop alpha)
/// - RGB → BGRA / BGR variant (channel reorder — lossless)
/// - Gray ↔ GrayAlpha (add/drop alpha)
fn layout_compatible(native: ChannelOrder, target: ChannelOrder) -> bool {
    // The JXL decoder's set_pixel_format handles all color type conversions,
    // so we're quite flexible here. We just need to avoid conversions that
    // are clearly lossy in a way the caller wouldn't expect.
    match (native, target) {
        // Same layout always works
        (a, b) if a == b => true,
        // Adding/removing alpha is fine (decoder handles it)
        (ChannelOrder::Rgb, ChannelOrder::Rgba | ChannelOrder::Bgra) => true,
        (ChannelOrder::Rgba, ChannelOrder::Rgb | ChannelOrder::Bgra) => true,
        (ChannelOrder::Gray, ChannelOrder::GrayAlpha) => true,
        (ChannelOrder::GrayAlpha, ChannelOrder::Gray) => true,
        // Gray → RGB (replicate — lossless)
        (
            ChannelOrder::Gray | ChannelOrder::GrayAlpha,
            ChannelOrder::Rgb | ChannelOrder::Rgba | ChannelOrder::Bgra,
        ) => true,
        // RGB → Gray (luminance — conceptually lossy, but decoder does it correctly)
        (ChannelOrder::Rgb | ChannelOrder::Rgba, ChannelOrder::Gray | ChannelOrder::GrayAlpha) => {
            true
        }
        _ => false,
    }
}

fn build_chosen(
    channel_type: SampleType,
    layout: ChannelOrder,
    has_alpha: bool,
    num_extra: usize,
) -> ChosenFormat {
    let color_type = match layout {
        ChannelOrder::Gray => JxlColorType::Grayscale,
        ChannelOrder::GrayAlpha => JxlColorType::GrayscaleAlpha,
        ChannelOrder::Rgb => JxlColorType::Rgb,
        ChannelOrder::Rgba => JxlColorType::Rgba,
        ChannelOrder::Bgra => JxlColorType::Bgra,
        _ => JxlColorType::Rgba, // fallback for unknown layouts
    };

    let data_format = match channel_type {
        SampleType::U8 => jxl::api::JxlDataFormat::U8 { bit_depth: 8 },
        SampleType::U16 => jxl::api::JxlDataFormat::U16 {
            endianness: jxl::api::Endianness::native(),
            bit_depth: 16,
        },
        SampleType::F32 => jxl::api::JxlDataFormat::F32 {
            endianness: jxl::api::Endianness::native(),
        },
        _ => jxl::api::JxlDataFormat::U8 { bit_depth: 8 },
    };

    // extra_channel_format must have exactly num_extra entries (matching
    // frame_header.num_extra_channels). When the color output already
    // includes alpha (RGBA/BGRA/GrayscaleAlpha), the alpha extra channel
    // is consumed by the color output — set its entry to None.
    // Non-alpha extra channels (depth, spot color, etc.) also get None
    // since we don't need separate buffers for them.
    let color_includes_alpha = has_alpha
        && matches!(
            color_type,
            JxlColorType::Rgba | JxlColorType::Bgra | JxlColorType::GrayscaleAlpha
        );

    let extra_channel_format = if color_includes_alpha || num_extra == 0 {
        // Alpha is part of the color output; all extra channels get None
        vec![None; num_extra]
    } else {
        // No alpha in color output; provide format for extra channels that
        // the caller might want (e.g. if we later support separate extra
        // channel output). For now, use None to skip them.
        vec![None; num_extra]
    };

    let pixel_format = JxlPixelFormat {
        color_type,
        color_data_format: Some(data_format),
        extra_channel_format,
    };

    ChosenFormat {
        pixel_format,
        color_type,
        channel_type,
    }
}

/// Convert jxl-rs extra channel info to our public type.
pub(crate) fn convert_extra_channels(
    channels: &[jxl::api::JxlExtraChannel],
) -> Vec<JxlExtraChannelInfo> {
    channels
        .iter()
        .map(|ec| JxlExtraChannelInfo {
            channel_type: JxlExtraChannelType::from_jxl(&ec.ec_type),
            bits_per_sample: ec.bits_per_sample as u8,
            name: if ec.name.is_empty() {
                None
            } else {
                Some(ec.name.clone())
            },
            alpha_associated: ec.alpha_associated,
            dim_shift: ec.dim_shift as u8,
        })
        .collect()
}

/// Probe JXL metadata without decoding pixels.
///
/// Uses restrictive decoder limits to bound CPU/memory cost on untrusted input.
/// The probe only needs to parse the file header and ICC profile; it does not
/// decode any frame data. Tighter limits prevent malformed inputs from causing
/// excessive entropy table construction or large ICC allocations.
///
/// Reports the orientation-adjusted (display) geometry, matching the default
/// [`decode`] behavior: [`JxlInfo::width`]/[`height`](JxlInfo::height) are the
/// display dimensions and [`JxlInfo::orientation`] is `1` (Identity), while
/// [`JxlInfo::coded_width`]/[`coded_height`](JxlInfo::coded_height) and
/// [`JxlInfo::intrinsic_orientation`] surface the stored values.
pub fn probe(data: &[u8]) -> Result<JxlInfo, At<JxlError>> {
    probe_with_orientation(data, true)
}

/// Probe JXL metadata with explicit control over orientation reporting.
///
/// When `adjust_orientation` is `true` (the default [`probe`] behavior) the
/// reported `width`/`height` are the display dimensions and `orientation` is
/// Identity. When `false`, the reported `width`/`height` are the stored (coded)
/// dimensions and `orientation` equals the intrinsic EXIF orientation — the
/// "Preserve" contract, used by the zencodec adapter to surface stored pixels.
///
/// `coded_width`/`coded_height` and `intrinsic_orientation` are populated the
/// same way in both modes (always the stored values).
pub(crate) fn probe_with_orientation(
    data: &[u8],
    adjust_orientation: bool,
) -> Result<JxlInfo, At<JxlError>> {
    probe_output(data, adjust_orientation).map(|(info, _)| info)
}

pub(crate) fn probe_output(
    data: &[u8],
    adjust_orientation: bool,
) -> Result<(JxlInfo, ImageSpec), At<JxlError>> {
    let mut options = JxlDecoderOptions::default();
    options.adjust_orientation = adjust_orientation;
    // Probe-specific limits: only header parsing is needed, so use tight bounds.
    // - ICC 1MB: covers all real-world ICC profiles (typical sRGB is 0.5-3KB)
    // - 64MB memory: bounds total allocations during header+ICC parsing
    // - Minimal tree/patch/spline limits since we don't decode frames
    options.limits.max_icc_size = Some(1 << 20); // 1 MB (vs 256 MB default)
    options.limits.max_memory_bytes = Some(64 << 20); // 64 MB
    options.limits.max_tree_size = Some(1 << 16); // 64K nodes
    options.limits.max_patches = Some(0); // no patches during probe
    options.limits.max_spline_points = Some(0); // no splines during probe
    let decoder = JxlDecoder::new(options);

    let mut input = data;
    let result = decoder
        .process(&mut input)
        .map_err(|e| e.map_error(map_err))?;
    let mut decoder = match result {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => {
            return Err(whereat::at!(JxlError::UnexpectedEof(
                "JXL: insufficient data for header".into(),
            )));
        }
    };

    let info = decoder.basic_info();
    // `size` is the emitted geometry (display dims when adjusting, coded dims
    // when not); `coded_size` is always the stored geometry. `orientation` is
    // the residual (Identity when adjusting); `intrinsic_orientation` is always
    // the stored EXIF orientation.
    let (width, height) = info.size;
    let (coded_width, coded_height) = info.coded_size;
    let has_alpha = info
        .extra_channels
        .iter()
        .any(|ec| matches!(ec.ec_type, ExtraChannel::Alpha));
    let has_animation = info.animation.is_some();
    let bit_depth = info.bit_depth.bits_per_sample() as u8;
    let orientation = info.orientation as u8;
    let intrinsic_orientation = info.intrinsic_orientation as u8;

    let (icc_profile, cicp) = extract_color_info(decoder.embedded_color_profile());
    let is_gray = profile_is_grayscale(decoder.embedded_color_profile());

    let xyb_encoded = !info.uses_original_profile;

    let extra_channels = convert_extra_channels(&info.extra_channels);
    let preview_size = info.preview_size.map(|(w, h)| (w as u32, h as u32));
    let intrinsic_size = info.intrinsic_size.map(|(w, h)| (w as u32, h as u32));
    let tm = info.tone_mapping.clone();
    let alpha = output_alpha(info);
    let chosen = choose_pixel_format(
        &info.bit_depth,
        has_alpha,
        is_gray,
        info.extra_channels.len(),
        &[],
    );
    decoder.set_pixel_format(chosen.pixel_format.clone());
    let spec = chosen.image_spec(decoder.output_color_profile(), alpha);

    Ok((
        JxlInfo {
            width: dim_to_u32(width, "width").map_err(|e| whereat::at!(e))?,
            height: dim_to_u32(height, "height").map_err(|e| whereat::at!(e))?,
            coded_width: dim_to_u32(coded_width, "coded width").map_err(|e| whereat::at!(e))?,
            coded_height: dim_to_u32(coded_height, "coded height").map_err(|e| whereat::at!(e))?,
            has_alpha,
            has_animation,
            bit_depth: Some(bit_depth),
            icc_profile,
            orientation,
            intrinsic_orientation,
            cicp,
            is_gray,
            // Probing only parses headers; EXIF/XMP are in trailing container
            // boxes and require full decode to access.
            exif: None,
            xmp: None,
            extra_channels,
            preview_size,
            xyb_encoded,
            intensity_target: tm.intensity_target,
            hdr: hdr_metadata(data, tm.intensity_target),
            min_nits: tm.min_nits,
            relative_to_max_display: tm.relative_to_max_display,
            linear_below: tm.linear_below,
            intrinsic_size,
        },
        spec,
    ))
}

/// Decode JXL to pixels.
///
/// `preferred` is a ranked list of desired output formats. The decoder picks
/// the first it can produce without lossy conversion. Pass `&[]` for the
/// decoder's native format.
pub fn decode(
    data: &[u8],
    limits: Option<&JxlLimits>,
    preferred: &[ImageSpec],
) -> Result<JxlDecodeOutput, At<JxlError>> {
    decode_with_parallel(data, limits, preferred, None)
}

/// Decode a JXL image with explicit parallel control.
///
/// `parallel` overrides the decoder's default threading behavior:
/// - `Some(true)` = enable parallel decoding
/// - `Some(false)` = force single-threaded decoding
/// - `None` = use decoder default (parallel when `threads` feature is enabled)
pub fn decode_with_parallel(
    data: &[u8],
    limits: Option<&JxlLimits>,
    preferred: &[ImageSpec],
    parallel: Option<bool>,
) -> Result<JxlDecodeOutput, At<JxlError>> {
    decode_with_options(data, limits, preferred, parallel, None)
}

/// Decode a JXL image with explicit parallel control and optional stop token.
///
/// `parallel` overrides the decoder's default threading behavior:
/// - `Some(true)` = enable parallel decoding
/// - `Some(false)` = force single-threaded decoding
/// - `None` = use decoder default (parallel when `threads` feature is enabled)
///
/// `stop` provides cooperative cancellation — the decoder checks the token
/// periodically and aborts early if signalled.
///
/// Orientation is adjusted (display geometry, Identity residual), matching the
/// JXL decoder default. The zencodec adapter calls
/// `decode_with_options_oriented` directly when it needs the un-baked
/// (Preserve) path.
pub fn decode_with_options(
    data: &[u8],
    limits: Option<&JxlLimits>,
    preferred: &[ImageSpec],
    parallel: Option<bool>,
    stop: Option<alloc::sync::Arc<dyn enough::Stop>>,
) -> Result<JxlDecodeOutput, At<JxlError>> {
    decode_with_options_oriented(
        data,
        limits,
        preferred,
        parallel,
        stop,
        true,
        false,
        zencodec::AllocPreference::CodecDefault,
    )
}

/// Decode a JXL image with explicit orientation-adjustment control.
///
/// `adjust_orientation` mirrors [`jxl::api::JxlDecoderOptions::adjust_orientation`]:
/// - `true` (the [`decode_with_options`] default): the decoder bakes the stored
///   orientation into the output pixels — emitted geometry is the display size
///   and the reported residual [`JxlInfo::orientation`] is Identity.
/// - `false`: pixels are emitted in their stored orientation — emitted geometry
///   equals the coded size and [`JxlInfo::orientation`] surfaces the intrinsic
///   EXIF orientation for a later stage to bake.
///
/// In both modes [`JxlInfo::coded_width`]/[`coded_height`](JxlInfo::coded_height)
/// and [`JxlInfo::intrinsic_orientation`] report the stored values.
///
/// `reject_progressive` mirrors [`jxl::api::JxlDecoderOptions::reject_progressive`]:
/// when `true`, the decoder errors at the first progressive frame header
/// (multi-pass or LF frame) instead of decoding it, surfaced here as
/// [`JxlError::ProgressiveRejected`]. The header-only probe never decodes a
/// frame, so this gate only matters on the decode path.
///
/// `alloc_pref` is the [`AllocPreference`](zencodec::AllocPreference) for the
/// one untrusted-sized allocation the wrapper owns — the full-image output
/// buffer, sized from the decoded header dimensions. Its site default is
/// *fallible* (a forged header demanding gigabytes yields a graceful
/// [`JxlError::OutOfMemory`] instead of aborting); `Fallible`/`Infallible`
/// override it, `CodecDefault` keeps it. The public `decode*` entry points pass
/// `CodecDefault` (behaviour unchanged); the zencodec adapter forwards the
/// caller's `ResourceLimits::prefer_fallible_allocations`. The heavy
/// VarDCT/modular pass buffers are allocated inside `zenjxl-decoder` and lie
/// beyond this preference.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_with_options_oriented(
    data: &[u8],
    limits: Option<&JxlLimits>,
    preferred: &[ImageSpec],
    parallel: Option<bool>,
    stop: Option<alloc::sync::Arc<dyn enough::Stop>>,
    adjust_orientation: bool,
    reject_progressive: bool,
    alloc_pref: zencodec::AllocPreference,
) -> Result<JxlDecodeOutput, At<JxlError>> {
    let mut options = JxlDecoderOptions::default();
    options.adjust_orientation = adjust_orientation;
    options.reject_progressive = reject_progressive;

    if let Some(p) = parallel {
        options.parallel = p;
    }

    if let Some(lim) = limits {
        if let Some(max_px) = lim.max_pixels {
            // Saturate u64 → usize on 32-bit targets so a high u64 limit
            // doesn't truncate to a small usize cap.
            options.limits.max_pixels = Some(usize::try_from(max_px).unwrap_or(usize::MAX));
        }
        if let Some(max_mem) = lim.max_memory_bytes {
            // jxl-rs takes max_memory_bytes as u64; pass through directly so
            // the wrapper's memory cap is honored end-to-end.
            options.limits.max_memory_bytes = Some(max_mem);
        }
    }

    // Forward stop token for cooperative cancellation.
    if let Some(stop) = stop {
        options.stop = stop;
    }

    let completion_stop = options.stop.clone();
    let decoder = JxlDecoder::new(options);

    // Phase 1: parse header
    let mut input = data;
    let result = decoder
        .process(&mut input)
        .map_err(|e| e.map_error(map_err))?;
    let mut decoder = match result {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => {
            return Err(whereat::at!(JxlError::UnexpectedEof(
                "JXL: insufficient data for header".into(),
            )));
        }
    };

    let info = decoder.basic_info();
    // `size` is the emitted geometry — display dims when `adjust_orientation`
    // is set, coded dims otherwise — and is the buffer to allocate. `coded_size`
    // is always the stored geometry. `orientation` is the residual of the
    // emitted pixels (Identity when adjusting); `intrinsic_orientation` is
    // always the stored EXIF orientation.
    let (width, height) = info.size;
    let (coded_width, coded_height) = info.coded_size;
    let has_alpha = info
        .extra_channels
        .iter()
        .any(|ec| matches!(ec.ec_type, ExtraChannel::Alpha));
    let has_animation = info.animation.is_some();
    let jxl_bit_depth = &info.bit_depth;
    let bit_depth_u8 = jxl_bit_depth.bits_per_sample() as u8;
    let orientation = info.orientation as u8;
    let intrinsic_orientation = info.intrinsic_orientation as u8;
    let is_gray = profile_is_grayscale(decoder.embedded_color_profile());

    let (icc_profile, cicp) = extract_color_info(decoder.embedded_color_profile());

    let num_extra = info.extra_channels.len();
    let xyb_encoded = !info.uses_original_profile;
    let extra_channels = convert_extra_channels(&info.extra_channels);
    let preview_size = info.preview_size.map(|(w, h)| (w as u32, h as u32));
    let intrinsic_size = info.intrinsic_size.map(|(w, h)| (w as u32, h as u32));
    let intensity_target = info.tone_mapping.intensity_target;
    let min_nits = info.tone_mapping.min_nits;
    let relative_to_max_display = info.tone_mapping.relative_to_max_display;
    let linear_below = info.tone_mapping.linear_below;

    // Choose output format based on native properties and caller preferences
    let chosen = choose_pixel_format(jxl_bit_depth, has_alpha, is_gray, num_extra, preferred);
    let channels = chosen.color_type.samples_per_pixel();
    let bytes_per_sample = match chosen.channel_type {
        SampleType::U8 => 1,
        SampleType::U16 => 2,
        SampleType::F32 => 4,
        _ => 1,
    };

    let width_u32 = dim_to_u32(width, "width").map_err(|e| whereat::at!(e))?;
    let height_u32 = dim_to_u32(height, "height").map_err(|e| whereat::at!(e))?;
    let coded_width_u32 = dim_to_u32(coded_width, "coded width").map_err(|e| whereat::at!(e))?;
    let coded_height_u32 = dim_to_u32(coded_height, "coded height").map_err(|e| whereat::at!(e))?;

    if let Some(lim) = limits {
        let bpp = (channels * bytes_per_sample) as u32;
        lim.validate(width_u32, height_u32, bpp)
            .map_err(|e| whereat::at!(e))?;
    }

    let alpha = output_alpha(info);
    let origin = color_origin(decoder.embedded_color_profile(), cicp);
    decoder.set_pixel_format(chosen.pixel_format.clone());
    let spec = chosen.image_spec(decoder.output_color_profile(), alpha);

    // Phase 2: frame info
    let result = decoder
        .process(&mut input)
        .map_err(|e| e.map_error(map_err))?;
    let decoder = match result {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => {
            return Err(whereat::at!(JxlError::UnexpectedEof(
                "JXL: insufficient data for frame".into(),
            )));
        }
    };

    // Phase 3: decode pixels.
    // The output buffer is sized straight from the (untrusted) header
    // dimensions, so it honors `alloc_pref` with a *fallible* site default —
    // see the function docs.
    let (bytes_per_row, buf_size) =
        checked_buf_size(width, height, channels, bytes_per_sample).map_err(|e| whereat::at!(e))?;
    let mut buf = alloc_pixel_storage(chosen.channel_type, alloc_pref, buf_size)?;

    let output = JxlOutputBuffer::new(buf.as_bytes_mut(), height, bytes_per_row);
    let result = decoder
        .process(&mut input, &mut [output])
        .map_err(|e| e.map_error(map_err))?;
    let mut final_decoder = match result {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => {
            return Err(whereat::at!(JxlError::UnexpectedEof(
                "JXL: insufficient data for pixels".into(),
            )));
        }
    };

    // Container boxes after the last codestream box (`Exif`, `xml `, `jhgm`)
    // are parsed only when the box parser is polled again, which the pixel
    // pass does just sometimes (zenjxl#22). Take what is already parsed, then
    // poll once more so the rest of the input is parsed. A trailing box that
    // fails to parse ends the walk without failing an image that decoded.
    let mut gain_map = final_decoder.take_gain_map();
    let mut exif = final_decoder.take_exif();
    let mut xmp = final_decoder.take_xmp();
    macro_rules! merge_trailing {
        ($d:expr) => {{
            let mut d = $d;
            gain_map = d.take_gain_map().or(gain_map.take());
            exif = d.take_exif().or(exif.take());
            xmp = d.take_xmp().or(xmp.take());
        }};
    }
    match final_decoder.process(&mut input) {
        Ok(ProcessingResult::Complete { result }) => merge_trailing!(result),
        Ok(ProcessingResult::NeedsMoreInput { fallback, .. }) => merge_trailing!(fallback),
        Err(_) => {}
    }

    // Clamp f32 output to [0.0, 1.0] for SDR / BT.709 content.
    // Lossy JXL can produce values slightly outside range as compression artifacts.
    // HDR (PQ/HLG) and wide gamut (BT.2020/P3) content is left unclamped.
    // When CICP is absent (ICC-only images), we don't know the gamut — don't clamp.
    let hdr = hdr_metadata(data, intensity_target);
    if chosen.channel_type == SampleType::F32
        && hdr.cll.is_none()
        && hdr.mastering.is_none()
        && cicp.is_some()
        && !is_hdr_or_wide_gamut(cicp)
    {
        clamp_f32_buf(buf.as_bytes_mut());
    }

    let mut pixels = adopt_pixels(buf, width_u32, height_u32, bytes_per_row, spec)?
        .with_origin(origin)
        .with_hdr(hdr);
    measure_integer_depth(&mut pixels);

    completion_stop
        .check()
        .map_err(|reason| whereat::at!(JxlError::Cancelled(reason)))?;

    Ok(JxlDecodeOutput {
        pixels,
        info: JxlInfo {
            width: width_u32,
            height: height_u32,
            coded_width: coded_width_u32,
            coded_height: coded_height_u32,
            has_alpha,
            has_animation,
            bit_depth: Some(bit_depth_u8),
            icc_profile,
            orientation,
            intrinsic_orientation,
            cicp,
            is_gray,
            exif,
            xmp,
            extra_channels,
            preview_size,
            xyb_encoded,
            intensity_target,
            hdr,
            min_nits,
            relative_to_max_display,
            linear_below,
            intrinsic_size,
        },
        gain_map,
    })
}

impl ChosenFormat {
    pub(crate) fn image_spec(&self, profile: &JxlColorProfile, alpha: AlphaMode) -> ImageSpec {
        let order = match self.color_type {
            JxlColorType::Grayscale => ChannelOrder::Gray,
            JxlColorType::GrayscaleAlpha => ChannelOrder::GrayAlpha,
            JxlColorType::Rgb => ChannelOrder::Rgb,
            JxlColorType::Rgba => ChannelOrder::Rgba,
            JxlColorType::Bgr => ChannelOrder::Bgr,
            JxlColorType::Bgra => ChannelOrder::Bgra,
            JxlColorType::Cmyk => ChannelOrder::CmykInverted,
        };
        output_spec(SampleLayout::new(self.channel_type, order), profile, alpha)
    }
}

pub(crate) fn output_alpha(info: &jxl::api::JxlBasicInfo) -> AlphaMode {
    match info
        .extra_channels
        .iter()
        .find(|ec| ec.ec_type == ExtraChannel::Alpha)
    {
        Some(ec) if ec.alpha_associated => AlphaMode::Premultiplied,
        Some(_) => AlphaMode::Straight,
        None => AlphaMode::Opaque,
    }
}

fn output_transfer(transfer: &JxlTransferFunction) -> Transfer {
    match transfer {
        JxlTransferFunction::BT709 => Transfer::Bt709,
        JxlTransferFunction::SRGB => Transfer::Srgb,
        JxlTransferFunction::Linear => Transfer::Linear,
        JxlTransferFunction::PQ => Transfer::Pq,
        JxlTransferFunction::HLG => Transfer::Hlg(zenpixels::HlgMode::Scene),
        JxlTransferFunction::DCI => Transfer::Gamma(Gamma::DCI_2_6),
        JxlTransferFunction::Gamma(gamma) => {
            Gamma::from_encoding_gamma(*gamma).map_or(Transfer::Unknown, Transfer::Gamma)
        }
    }
}

fn output_primaries(primaries: &JxlPrimaries, white: &JxlWhitePoint) -> Primaries {
    match (primaries, white) {
        (JxlPrimaries::SRGB, JxlWhitePoint::D65) => Primaries::Bt709,
        (JxlPrimaries::BT2100, JxlWhitePoint::D65) => Primaries::Bt2020,
        (JxlPrimaries::P3, JxlWhitePoint::D65) => Primaries::DisplayP3,
        (JxlPrimaries::P3, JxlWhitePoint::DCI) => Primaries::DciP3,
        _ => {
            let [r, g, b] = primaries.to_xy_coords();
            CustomPrimaries::new(r, g, b, white.to_xy_coords())
                .map_or(Primaries::Unknown, Primaries::Custom)
        }
    }
}

pub(crate) fn output_spec(
    layout: SampleLayout,
    profile: &JxlColorProfile,
    alpha: AlphaMode,
) -> ImageSpec {
    let order = layout.order;
    let alpha = if order.has_alpha_slot() {
        alpha
    } else {
        AlphaMode::None
    };
    let color = match profile {
        JxlColorProfile::Icc(icc) if order.is_cmyk_family() => ColorSpace::Cmyk {
            inverted: true,
            icc: Some(alloc::sync::Arc::from(icc.as_slice())),
            alpha,
        },
        JxlColorProfile::Icc(icc) => ColorSpace::Icc {
            profile: alloc::sync::Arc::from(icc.as_slice()),
            channels: (order.channels() - usize::from(order.has_alpha_slot())) as u8,
            alpha,
            intent: RenderingIntent::RelativeColorimetric,
        },
        JxlColorProfile::Simple(encoding) => {
            let (primaries, transfer) = match encoding {
                JxlColorEncoding::RgbColorSpace {
                    primaries,
                    transfer_function,
                    white_point,
                    ..
                } => (
                    output_primaries(primaries, white_point),
                    output_transfer(transfer_function),
                ),
                JxlColorEncoding::GrayscaleColorSpace {
                    white_point,
                    transfer_function,
                    ..
                } => (
                    output_primaries(&JxlPrimaries::SRGB, white_point),
                    output_transfer(transfer_function),
                ),
                JxlColorEncoding::XYB { .. } => (Primaries::Unknown, Transfer::Unknown),
            };
            if order.is_gray_family() {
                ColorSpace::Gray {
                    transfer,
                    range: SignalRange::Full,
                    alpha,
                    white: None,
                }
            } else {
                ColorSpace::Rgb {
                    primaries,
                    transfer,
                    range: SignalRange::Full,
                    alpha,
                    white: None,
                }
            }
        }
    };
    ImageSpec::new(layout, color)
}

pub(crate) fn color_origin(
    profile: &JxlColorProfile,
    cicp: Option<(u8, u8, u8, bool)>,
) -> ColorOrigin {
    match profile {
        JxlColorProfile::Icc(bytes) => {
            ColorOrigin::from_icc(alloc::sync::Arc::from(bytes.as_slice()))
        }
        JxlColorProfile::Simple(_) => cicp.map_or_else(ColorOrigin::assumed, |(p, t, m, r)| {
            ColorOrigin::from_cicp(Cicp::new(p, t, m, r))
        }),
    }
}

pub(crate) fn hdr_metadata(data: &[u8], intensity_target: f32) -> HdrMetadata {
    crate::hdr::from_container(data).unwrap_or_else(|| HdrMetadata {
        cll: (intensity_target > 255.0).then(|| {
            zenpixels::ContentLightLevel::new(intensity_target.min(u16::MAX as f32) as u16, 0)
        }),
        mastering: None,
    })
}

pub(crate) fn alloc_pixel_storage(
    sample: SampleType,
    pref: zencodec::AllocPreference,
    bytes: usize,
) -> Result<PixelStorage, At<JxlError>> {
    PixelStorage::zeroed(sample, bytes).map_err(|_| {
        if !crate::alloc_util::resolve_fallible(pref, true) {
            let layout = core::alloc::Layout::from_size_align(bytes, sample.min_alignment())
                .expect("pixel capacity overflow");
            alloc::alloc::handle_alloc_error(layout);
        }
        whereat::at!(JxlError::OutOfMemory(alloc::format!(
            "allocating {bytes} pixel bytes"
        )))
    })
}

pub(crate) fn adopt_pixels(
    storage: PixelStorage,
    width: u32,
    height: u32,
    stride: usize,
    spec: ImageSpec,
) -> Result<PixelBuf, At<JxlError>> {
    PixelBuf::from_storage_parts(storage, 0, width, height, stride, spec)
        .map_err(|e| whereat::at!(JxlError::InvalidInput(e.to_string())))
}

fn measure_integer_depth(pixels: &mut PixelBuf) {
    let depth = zenpixels::sample::ExactDepth::measure(&pixels.as_view()).ok();
    pixels.assume_facts(zenpixels::SampleFacts::NONE.with_depth_opt(depth));
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// Helper: read a test file from the zenjxl-decoder resource directory.
    fn read_jxl_test_file(name: &str) -> Vec<u8> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../zenjxl-decoder/zenjxl-decoder/resources/test")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| {
            panic!("failed to read test file {}: {}", path.display(), e);
        })
    }

    #[test]
    fn extra_channel_alpha_from_rgba_image() {
        // 3x3a has an alpha channel
        let data = read_jxl_test_file("3x3a_srgb_lossless.jxl");
        let info = probe(&data).unwrap();

        assert!(info.has_alpha);
        assert_eq!(info.extra_channels.len(), 1);

        let alpha = &info.extra_channels[0];
        assert_eq!(alpha.channel_type, JxlExtraChannelType::Alpha);
        assert!(alpha.bits_per_sample > 0);
        assert_eq!(alpha.dim_shift, 0); // full resolution alpha
    }

    #[test]
    fn extra_channel_enumeration_multi_channel() {
        let data = read_jxl_test_file("extra_channels.jxl");
        let info = probe(&data).unwrap();

        // This file should have extra channels
        assert!(
            !info.extra_channels.is_empty(),
            "extra_channels.jxl should have extra channels"
        );

        // Verify all channels have valid metadata
        for ec in &info.extra_channels {
            assert!(ec.bits_per_sample > 0 && ec.bits_per_sample <= 32);
            assert!(ec.dim_shift <= 3);
        }
    }

    #[test]
    fn no_extra_channels_for_rgb_image() {
        let data = read_jxl_test_file("3x3_srgb_lossless.jxl");
        let info = probe(&data).unwrap();

        // RGB-only image: no alpha, no extra channels
        assert!(!info.has_alpha);
        assert!(
            info.extra_channels.is_empty(),
            "RGB image should have no extra channels"
        );
    }

    #[test]
    fn channel_type_mapping_covers_known_types() {
        // Verify the mapping function handles all ExtraChannel variants without panic
        let variants = [
            ExtraChannel::Alpha,
            ExtraChannel::Depth,
            ExtraChannel::SpotColor,
            ExtraChannel::SelectionMask,
            ExtraChannel::Black,
            ExtraChannel::CFA,
            ExtraChannel::Thermal,
            ExtraChannel::Optional,
            ExtraChannel::Unknown,
            ExtraChannel::Reserved0,
            ExtraChannel::Reserved1,
            ExtraChannel::Reserved2,
            ExtraChannel::Reserved3,
            ExtraChannel::Reserved4,
            ExtraChannel::Reserved5,
            ExtraChannel::Reserved6,
            ExtraChannel::Reserved7,
        ];

        for variant in &variants {
            let mapped = JxlExtraChannelType::from_jxl(variant);
            // Just verify it doesn't panic and returns something sensible
            match variant {
                ExtraChannel::Alpha => assert_eq!(mapped, JxlExtraChannelType::Alpha),
                ExtraChannel::Depth => assert_eq!(mapped, JxlExtraChannelType::Depth),
                ExtraChannel::SpotColor => assert_eq!(mapped, JxlExtraChannelType::SpotColor),
                ExtraChannel::SelectionMask => {
                    assert_eq!(mapped, JxlExtraChannelType::SelectionMask)
                }
                ExtraChannel::Black => assert_eq!(mapped, JxlExtraChannelType::Black),
                ExtraChannel::CFA => assert_eq!(mapped, JxlExtraChannelType::Cfa),
                ExtraChannel::Thermal => assert_eq!(mapped, JxlExtraChannelType::Thermal),
                ExtraChannel::Optional => assert_eq!(mapped, JxlExtraChannelType::Optional),
                _ => {
                    // Reserved and Unknown map to Unknown(n)
                    assert!(
                        matches!(mapped, JxlExtraChannelType::Unknown(_)),
                        "expected Unknown for {variant:?}, got {mapped:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn preview_detected_when_present() {
        let data = read_jxl_test_file("with_preview.jxl");
        let info = probe(&data).unwrap();

        let (pw, ph) = info
            .preview_size
            .expect("expected preview_size for with_preview.jxl");
        assert!(pw > 0 && ph > 0, "preview dimensions should be positive");
    }

    #[test]
    fn no_preview_for_regular_image() {
        let data = read_jxl_test_file("basic.jxl");
        let info = probe(&data).unwrap();

        assert!(
            info.preview_size.is_none(),
            "basic.jxl should not have a preview"
        );
    }

    #[test]
    fn extra_channels_survive_full_decode() {
        // Verify extra_channels are also populated after full decode, not just probe
        let data = read_jxl_test_file("3x3a_srgb_lossless.jxl");
        let output = decode(&data, None, &[]).unwrap();

        assert_eq!(output.info.extra_channels.len(), 1);
        assert_eq!(
            output.info.extra_channels[0].channel_type,
            JxlExtraChannelType::Alpha
        );
    }

    #[test]
    fn preview_size_survives_full_decode() {
        let data = read_jxl_test_file("with_preview.jxl");
        let output = decode(&data, None, &[]).unwrap();

        let (pw, ph) = output
            .info
            .preview_size
            .expect("preview_size should be set after full decode");
        assert!(pw > 0 && ph > 0);
    }

    // ── Audit H1/H3/H4 regression tests ────────────────────────────────

    /// H1: max_memory_bytes must reach the decoder.
    ///
    /// A pathologically tight memory cap (one byte) forwarded into
    /// jxl-rs's `options.limits.max_memory_bytes` should make the decoder
    /// abort, not silently allocate. Before the fix the field was dropped
    /// at the wrapper -> decoder hop.
    #[test]
    fn max_memory_bytes_propagates_to_decoder() {
        let data = read_jxl_test_file("3x3_srgb_lossless.jxl");
        let limits = JxlLimits {
            max_pixels: None,
            // 1 byte: well below anything jxl-rs can allocate during decode.
            max_memory_bytes: Some(1),
        };
        let result = decode(&data, Some(&limits), &[]);
        assert!(
            result.is_err(),
            "expected decode to fail under a 1-byte memory cap, got Ok"
        );
    }

    /// H1: a generous max_memory_bytes does not break the happy path.
    #[test]
    fn generous_max_memory_bytes_decodes_ok() {
        let data = read_jxl_test_file("3x3_srgb_lossless.jxl");
        let limits = JxlLimits {
            max_pixels: None,
            max_memory_bytes: Some(64 * 1024 * 1024),
        };
        let result = decode(&data, Some(&limits), &[]);
        assert!(result.is_ok(), "decode under 64 MB cap should succeed");
    }

    /// H3: header dimensions exceeding `u32::MAX` are flagged as malformed,
    /// not silently truncated.
    #[test]
    fn dim_to_u32_rejects_oversized_dimension() {
        // On 32-bit targets `usize == u32` and this can't overflow; skip.
        if usize::BITS <= 32 {
            return;
        }
        let too_big: usize = (u32::MAX as usize) + 1;
        let err = dim_to_u32(too_big, "width").expect_err("must reject");
        assert!(matches!(err, JxlError::MalformedImage(_)));
    }

    /// H3: dimensions that fit pass through unchanged.
    #[test]
    fn dim_to_u32_accepts_normal_dimensions() {
        assert_eq!(dim_to_u32(0, "w").unwrap(), 0);
        assert_eq!(dim_to_u32(1, "w").unwrap(), 1);
        assert_eq!(dim_to_u32(4096, "w").unwrap(), 4096);
        assert_eq!(dim_to_u32(u32::MAX as usize, "w").unwrap(), u32::MAX);
    }

    /// H4: the buffer-size product overflowing usize must surface as
    /// `OutOfMemory` (a size overflow, not a configured `LimitExceeded` cap),
    /// not wrap and produce an undersized allocation.
    #[test]
    fn checked_buf_size_rejects_overflow() {
        // Pick a width and height whose product overflows usize on both
        // 32- and 64-bit. usize::MAX as both factors guarantees that.
        let res = checked_buf_size(usize::MAX, 2, 1, 1);
        assert!(matches!(res, Err(JxlError::OutOfMemory(_))));

        // Overflow further down the multiplication chain.
        let res = checked_buf_size(usize::MAX / 4 + 1, 1, 4, 1);
        assert!(matches!(res, Err(JxlError::OutOfMemory(_))));
    }

    /// H4: small dimensions compute the expected sizes.
    #[test]
    fn checked_buf_size_happy_path() {
        let (row, total) = checked_buf_size(640, 480, 4, 1).unwrap();
        assert_eq!(row, 640 * 4);
        assert_eq!(total, 640 * 4 * 480);

        let (row, total) = checked_buf_size(1, 1, 1, 1).unwrap();
        assert_eq!(row, 1);
        assert_eq!(total, 1);
    }
}
