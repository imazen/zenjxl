//! JXL decoding and probing via jxl-rs.

use alloc::vec;
use alloc::vec::Vec;

use zenpixels::{ChannelLayout, ChannelType, PixelBuffer, PixelDescriptor};

use jxl::api::{
    ExtraChannel, JxlBitDepth, JxlColorEncoding, JxlColorProfile, JxlColorType, JxlDecoder,
    JxlDecoderOptions, JxlOutputBuffer, JxlPixelFormat, ProcessingResult,
};

use crate::error::JxlError;

/// JXL image metadata from probing.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct JxlInfo {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Whether the image has an alpha channel.
    pub has_alpha: bool,
    /// Whether the image contains animation.
    pub has_animation: bool,
    /// Bits per sample (e.g. 8, 16, 32).
    pub bit_depth: Option<u8>,
    /// Embedded ICC color profile.
    pub icc_profile: Option<Vec<u8>>,
    /// EXIF orientation (1-8). 1 = Normal.
    pub orientation: u8,
    /// CICP color description `(color_primaries, transfer_characteristics, matrix_coefficients, full_range)`.
    ///
    /// Derived from JXL's structured color encoding when the image does not use
    /// an ICC profile. `None` for ICC-profiled images or custom color spaces.
    pub cicp: Option<(u8, u8, u8, bool)>,
    /// Whether the image's color encoding is grayscale.
    pub is_gray: bool,
}

impl zencodec::SourceEncodingDetails for JxlInfo {
    fn source_generic_quality(&self) -> Option<f32> {
        // JXL headers don't expose the encoding quality/distance.
        None
    }
}

/// JXL decode output.
#[derive(Debug)]
pub struct JxlDecodeOutput {
    /// Decoded pixel data.
    pub pixels: PixelBuffer,
    /// Image metadata.
    pub info: JxlInfo,
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
                return Err(JxlError::LimitExceeded("pixel count exceeds limit".into()));
            }
        }
        if let Some(max_mem) = self.max_memory_bytes {
            let estimated = width as u64 * height as u64 * bytes_per_pixel as u64;
            if estimated > max_mem {
                return Err(JxlError::LimitExceeded(
                    "estimated memory exceeds limit".into(),
                ));
            }
        }
        Ok(())
    }
}

use jxl::api::{JxlPrimaries, JxlTransferFunction};

fn map_err(e: jxl::api::Error) -> JxlError {
    JxlError::Decode(e)
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
            ..
        } => {
            let cp = match primaries {
                JxlPrimaries::SRGB => 1,   // BT.709
                JxlPrimaries::BT2100 => 9, // BT.2020
                JxlPrimaries::P3 => 12,    // Display P3
                JxlPrimaries::Chromaticities { .. } => return None,
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
    for chunk in buf.chunks_exact_mut(4) {
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
    pub(crate) channel_type: ChannelType,
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
    preferred: &[PixelDescriptor],
) -> ChosenFormat {
    let is_float = matches!(bit_depth, JxlBitDepth::Float { .. });
    let bps = bit_depth.bits_per_sample();

    // Determine native channel type (what we can produce losslessly)
    let native_channel_type = if is_float || bps > 16 {
        ChannelType::F32
    } else if bps > 8 {
        ChannelType::U16
    } else {
        ChannelType::U8
    };

    // Determine native channel layout
    let native_layout = match (is_gray, has_alpha) {
        (true, false) => ChannelLayout::Gray,
        (true, true) => ChannelLayout::GrayAlpha,
        (false, false) => ChannelLayout::Rgb,
        (false, true) => ChannelLayout::Rgba,
    };

    // If preferred list is non-empty, find the first we can produce losslessly.
    // "Losslessly" means: we don't drop precision (channel_type >= native)
    // and we don't discard channels the caller wants.
    if !preferred.is_empty() {
        for desc in preferred {
            // Can we produce this channel type without precision loss?
            let ct = desc.channel_type();
            if !can_produce_losslessly(native_channel_type, ct) {
                continue;
            }
            // Can we produce this layout from the native data?
            if !layout_compatible(native_layout, desc.layout()) {
                continue;
            }
            // Only allow grayscale output when the source is actually grayscale.
            // XYB-encoded JXL files have 3 color channels internally even if
            // the color profile says "gray", and jxl-rs rejects grayscale output
            // for 3-channel images.
            if (desc.layout() == ChannelLayout::Gray || desc.layout() == ChannelLayout::GrayAlpha)
                && !is_gray
            {
                continue;
            }
            return build_chosen(ct, desc.layout(), has_alpha, num_extra);
        }
    }

    // Default: native precision, but always use RGB/RGBA layout.
    // JXL's XYB encoding uses 3 color channels internally regardless of whether
    // the source was grayscale, and jxl-rs rejects grayscale output for 3-channel
    // images. Only use grayscale when both the profile says gray AND the caller
    // explicitly requests it (handled above).
    let default_layout = if is_gray {
        if has_alpha {
            ChannelLayout::GrayAlpha
        } else {
            ChannelLayout::Gray
        }
    } else if has_alpha {
        ChannelLayout::Rgba
    } else {
        ChannelLayout::Rgb
    };
    build_chosen(native_channel_type, default_layout, has_alpha, num_extra)
}

/// Returns true if we can produce `target` type from `native` without lossy conversion.
fn can_produce_losslessly(native: ChannelType, target: ChannelType) -> bool {
    match native {
        ChannelType::U8 => matches!(
            target,
            ChannelType::U8 | ChannelType::U16 | ChannelType::F32
        ),
        ChannelType::U16 => matches!(target, ChannelType::U16 | ChannelType::F32),
        ChannelType::F32 => matches!(target, ChannelType::F32),
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
fn layout_compatible(native: ChannelLayout, target: ChannelLayout) -> bool {
    // The JXL decoder's set_pixel_format handles all color type conversions,
    // so we're quite flexible here. We just need to avoid conversions that
    // are clearly lossy in a way the caller wouldn't expect.
    match (native, target) {
        // Same layout always works
        (a, b) if a == b => true,
        // Adding/removing alpha is fine (decoder handles it)
        (ChannelLayout::Rgb, ChannelLayout::Rgba | ChannelLayout::Bgra) => true,
        (ChannelLayout::Rgba, ChannelLayout::Rgb | ChannelLayout::Bgra) => true,
        (ChannelLayout::Gray, ChannelLayout::GrayAlpha) => true,
        (ChannelLayout::GrayAlpha, ChannelLayout::Gray) => true,
        // Gray → RGB (replicate — lossless)
        (
            ChannelLayout::Gray | ChannelLayout::GrayAlpha,
            ChannelLayout::Rgb | ChannelLayout::Rgba | ChannelLayout::Bgra,
        ) => true,
        // RGB → Gray (luminance — conceptually lossy, but decoder does it correctly)
        (
            ChannelLayout::Rgb | ChannelLayout::Rgba,
            ChannelLayout::Gray | ChannelLayout::GrayAlpha,
        ) => true,
        _ => false,
    }
}

fn build_chosen(
    channel_type: ChannelType,
    layout: ChannelLayout,
    has_alpha: bool,
    num_extra: usize,
) -> ChosenFormat {
    let color_type = match layout {
        ChannelLayout::Gray => JxlColorType::Grayscale,
        ChannelLayout::GrayAlpha => JxlColorType::GrayscaleAlpha,
        ChannelLayout::Rgb => JxlColorType::Rgb,
        ChannelLayout::Rgba => JxlColorType::Rgba,
        ChannelLayout::Bgra => JxlColorType::Bgra,
        _ => JxlColorType::Rgba, // fallback for unknown layouts
    };

    let data_format = match channel_type {
        ChannelType::U8 => jxl::api::JxlDataFormat::U8 { bit_depth: 8 },
        ChannelType::U16 => jxl::api::JxlDataFormat::U16 {
            endianness: jxl::api::Endianness::native(),
            bit_depth: 16,
        },
        ChannelType::F32 => jxl::api::JxlDataFormat::F32 {
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

/// Probe JXL metadata without decoding pixels.
pub fn probe(data: &[u8]) -> Result<JxlInfo, JxlError> {
    let options = JxlDecoderOptions::default();
    let decoder = JxlDecoder::new(options);

    let mut input = data;
    let decoder = match decoder.process(&mut input).map_err(map_err)? {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => {
            return Err(JxlError::InvalidInput(
                "JXL: insufficient data for header".into(),
            ));
        }
    };

    let info = decoder.basic_info();
    let (width, height) = info.size;
    let has_alpha = info
        .extra_channels
        .iter()
        .any(|ec| matches!(ec.ec_type, ExtraChannel::Alpha));
    let has_animation = info.animation.is_some();
    let bit_depth = info.bit_depth.bits_per_sample() as u8;
    let orientation = info.orientation as u8;

    let (icc_profile, cicp) = extract_color_info(decoder.embedded_color_profile());
    let is_gray = profile_is_grayscale(decoder.embedded_color_profile());

    Ok(JxlInfo {
        width: width as u32,
        height: height as u32,
        has_alpha,
        has_animation,
        bit_depth: Some(bit_depth),
        icc_profile,
        orientation,
        cicp,
        is_gray,
    })
}

/// Decode JXL to pixels.
///
/// `preferred` is a ranked list of desired output formats. The decoder picks
/// the first it can produce without lossy conversion. Pass `&[]` for the
/// decoder's native format.
pub fn decode(
    data: &[u8],
    limits: Option<&JxlLimits>,
    preferred: &[PixelDescriptor],
) -> Result<JxlDecodeOutput, JxlError> {
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
    preferred: &[PixelDescriptor],
    parallel: Option<bool>,
) -> Result<JxlDecodeOutput, JxlError> {
    let mut options = JxlDecoderOptions::default();

    if let Some(p) = parallel {
        options.parallel = p;
    }

    if let Some(lim) = limits
        && let Some(max_px) = lim.max_pixels
    {
        options.limits.max_pixels = Some(max_px as usize);
    }

    let decoder = JxlDecoder::new(options);

    // Phase 1: parse header
    let mut input = data;
    let mut decoder = match decoder.process(&mut input).map_err(map_err)? {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => {
            return Err(JxlError::InvalidInput(
                "JXL: insufficient data for header".into(),
            ));
        }
    };

    let info = decoder.basic_info();
    let (width, height) = info.size;
    let has_alpha = info
        .extra_channels
        .iter()
        .any(|ec| matches!(ec.ec_type, ExtraChannel::Alpha));
    let has_animation = info.animation.is_some();
    let jxl_bit_depth = &info.bit_depth;
    let bit_depth_u8 = jxl_bit_depth.bits_per_sample() as u8;
    let orientation = info.orientation as u8;
    let is_gray = profile_is_grayscale(decoder.embedded_color_profile());

    let (icc_profile, cicp) = extract_color_info(decoder.embedded_color_profile());

    let num_extra = info.extra_channels.len();

    // Choose output format based on native properties and caller preferences
    let chosen = choose_pixel_format(jxl_bit_depth, has_alpha, is_gray, num_extra, preferred);
    let channels = chosen.color_type.samples_per_pixel();
    let bytes_per_sample = match chosen.channel_type {
        ChannelType::U8 => 1,
        ChannelType::U16 => 2,
        ChannelType::F32 => 4,
        _ => 1,
    };

    if let Some(lim) = limits {
        let bpp = (channels * bytes_per_sample) as u32;
        lim.validate(width as u32, height as u32, bpp)?;
    }

    decoder.set_pixel_format(chosen.pixel_format.clone());

    // Phase 2: frame info
    let decoder = match decoder.process(&mut input).map_err(map_err)? {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => {
            return Err(JxlError::InvalidInput(
                "JXL: insufficient data for frame".into(),
            ));
        }
    };

    // Phase 3: decode pixels
    let bytes_per_row = width * channels * bytes_per_sample;
    let buf_size = bytes_per_row * height;
    let mut buf = vec![0u8; buf_size];

    let output = JxlOutputBuffer::new(&mut buf, height, bytes_per_row);
    let _decoder = match decoder
        .process(&mut input, &mut [output])
        .map_err(map_err)?
    {
        ProcessingResult::Complete { result } => result,
        ProcessingResult::NeedsMoreInput { .. } => {
            return Err(JxlError::InvalidInput(
                "JXL: insufficient data for pixels".into(),
            ));
        }
    };

    // Clamp f32 output to [0.0, 1.0] for SDR / BT.709 content.
    // Lossy JXL can produce values slightly outside range as compression artifacts.
    // HDR (PQ/HLG) and wide gamut (BT.2020/P3) content is left unclamped.
    // When CICP is absent (ICC-only images), we don't know the gamut — don't clamp.
    if chosen.channel_type == ChannelType::F32 && cicp.is_some() && !is_hdr_or_wide_gamut(cicp) {
        clamp_f32_buf(&mut buf);
    }

    let pixels = build_pixel_data(&buf, width, height, &chosen);

    Ok(JxlDecodeOutput {
        pixels,
        info: JxlInfo {
            width: width as u32,
            height: height as u32,
            has_alpha,
            has_animation,
            bit_depth: Some(bit_depth_u8),
            icc_profile,
            orientation,
            cicp,
            is_gray,
        },
    })
}

/// Interpret the raw output buffer as a [`PixelBuffer`].
///
/// Returns a type-erased PixelBuffer with the base descriptor (Unknown transfer
/// function). The caller should set the correct transfer function from CICP metadata.
pub(crate) fn build_pixel_data(
    buf: &[u8],
    width: usize,
    height: usize,
    chosen: &ChosenFormat,
) -> PixelBuffer {
    use rgb::{Gray, Rgb, Rgba};

    let w = width as u32;
    let h = height as u32;
    // from_pixels validates count — unwrap is safe here because the decoder
    // guarantees buf has exactly width*height*bpp bytes.

    match (chosen.channel_type, &chosen.color_type) {
        // ── u8 variants ──────────────────────────────────────────────
        (ChannelType::U8, JxlColorType::Rgb) => {
            let pixels: Vec<Rgb<u8>> = buf
                .chunks_exact(3)
                .map(|c| Rgb {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::U8, JxlColorType::Rgba) => {
            let pixels: Vec<Rgba<u8>> = buf
                .chunks_exact(4)
                .map(|c| Rgba {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                    a: c[3],
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::U8, JxlColorType::Grayscale) => {
            let pixels: Vec<Gray<u8>> = buf.iter().map(|&v| Gray::new(v)).collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::U8, JxlColorType::GrayscaleAlpha) => {
            // GrayAlpha lacks bytemuck NoUninit, use from_vec with raw bytes
            PixelBuffer::from_vec(buf.to_vec(), w, h, PixelDescriptor::GRAYA8).unwrap()
        }
        (ChannelType::U8, JxlColorType::Bgra) => {
            let pixels: Vec<rgb::alt::BGRA<u8>> = buf
                .chunks_exact(4)
                .map(|c| rgb::alt::BGRA {
                    b: c[0],
                    g: c[1],
                    r: c[2],
                    a: c[3],
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::U8, JxlColorType::Bgr) => {
            // No Bgr pixel type, convert to Rgb
            let pixels: Vec<Rgb<u8>> = buf
                .chunks_exact(3)
                .map(|c| Rgb {
                    r: c[2],
                    g: c[1],
                    b: c[0],
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }

        // ── u16 variants ─────────────────────────────────────────────
        (ChannelType::U16, JxlColorType::Rgb) => {
            let pixels: Vec<Rgb<u16>> = buf
                .chunks_exact(6)
                .map(|c| Rgb {
                    r: u16::from_ne_bytes([c[0], c[1]]),
                    g: u16::from_ne_bytes([c[2], c[3]]),
                    b: u16::from_ne_bytes([c[4], c[5]]),
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::U16, JxlColorType::Rgba) => {
            let pixels: Vec<Rgba<u16>> = buf
                .chunks_exact(8)
                .map(|c| Rgba {
                    r: u16::from_ne_bytes([c[0], c[1]]),
                    g: u16::from_ne_bytes([c[2], c[3]]),
                    b: u16::from_ne_bytes([c[4], c[5]]),
                    a: u16::from_ne_bytes([c[6], c[7]]),
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::U16, JxlColorType::Grayscale) => {
            let pixels: Vec<Gray<u16>> = buf
                .chunks_exact(2)
                .map(|c| Gray::new(u16::from_ne_bytes([c[0], c[1]])))
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::U16, JxlColorType::GrayscaleAlpha) => {
            // GrayAlpha lacks bytemuck NoUninit, use from_vec with raw bytes
            PixelBuffer::from_vec(buf.to_vec(), w, h, PixelDescriptor::GRAYA16).unwrap()
        }

        // ── f32 variants ─────────────────────────────────────────────
        (ChannelType::F32, JxlColorType::Rgb) => {
            let pixels: Vec<Rgb<f32>> = buf
                .chunks_exact(12)
                .map(|c| Rgb {
                    r: f32::from_ne_bytes([c[0], c[1], c[2], c[3]]),
                    g: f32::from_ne_bytes([c[4], c[5], c[6], c[7]]),
                    b: f32::from_ne_bytes([c[8], c[9], c[10], c[11]]),
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::F32, JxlColorType::Rgba) => {
            let pixels: Vec<Rgba<f32>> = buf
                .chunks_exact(16)
                .map(|c| Rgba {
                    r: f32::from_ne_bytes([c[0], c[1], c[2], c[3]]),
                    g: f32::from_ne_bytes([c[4], c[5], c[6], c[7]]),
                    b: f32::from_ne_bytes([c[8], c[9], c[10], c[11]]),
                    a: f32::from_ne_bytes([c[12], c[13], c[14], c[15]]),
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::F32, JxlColorType::Grayscale) => {
            let pixels: Vec<Gray<f32>> = buf
                .chunks_exact(4)
                .map(|c| Gray::new(f32::from_ne_bytes([c[0], c[1], c[2], c[3]])))
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
        (ChannelType::F32, JxlColorType::GrayscaleAlpha) => {
            // GrayAlpha lacks bytemuck NoUninit, use from_vec with raw bytes
            PixelBuffer::from_vec(buf.to_vec(), w, h, PixelDescriptor::GRAYAF32).unwrap()
        }

        // Fallback: shouldn't happen given choose_pixel_format logic,
        // but decode as RGBA8 to be safe
        _ => {
            let pixels: Vec<Rgba<u8>> = buf
                .chunks_exact(4)
                .map(|c| Rgba {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                    a: c[3],
                })
                .collect();
            PixelBuffer::from_pixels(pixels, w, h).unwrap().into()
        }
    }
}
