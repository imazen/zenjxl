//! JXL decoding and probing via jxl-rs.

use alloc::vec;
use alloc::vec::Vec;

use imgref::ImgVec;
use zencodec_types::PixelData;

use jxl::api::{
    ExtraChannel, JxlDecoder, JxlDecoderOptions, JxlOutputBuffer, JxlPixelFormat, ProcessingResult,
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
}

/// JXL decode output.
#[derive(Debug)]
pub struct JxlDecodeOutput {
    /// Decoded pixel data.
    pub pixels: PixelData,
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

use jxl::api::{JxlColorEncoding, JxlColorProfile, JxlPrimaries, JxlTransferFunction};

fn map_err(e: jxl::api::Error) -> JxlError {
    JxlError::Decode(e)
}

/// Extract ICC profile and CICP from JXL color profile.
#[allow(clippy::type_complexity)]
fn extract_color_info(profile: &JxlColorProfile) -> (Option<Vec<u8>>, Option<(u8, u8, u8, bool)>) {
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

    Ok(JxlInfo {
        width: width as u32,
        height: height as u32,
        has_alpha,
        has_animation,
        bit_depth: Some(bit_depth),
        icc_profile,
        orientation,
        cicp,
    })
}

/// Decode JXL to pixels.
pub fn decode(data: &[u8], limits: Option<&JxlLimits>) -> Result<JxlDecodeOutput, JxlError> {
    let mut options = JxlDecoderOptions::default();

    if let Some(lim) = limits {
        if let Some(max_px) = lim.max_pixels {
            options.limits.max_pixels = Some(max_px as usize);
        }
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
    let bit_depth = info.bit_depth.bits_per_sample() as u8;
    let orientation = info.orientation as u8;

    let (icc_profile, cicp) = extract_color_info(decoder.embedded_color_profile());

    if let Some(lim) = limits {
        let bpp: u32 = if has_alpha { 4 } else { 3 };
        lim.validate(width as u32, height as u32, bpp)?;
    }

    let num_extra = info
        .extra_channels
        .iter()
        .filter(|ec| !matches!(ec.ec_type, ExtraChannel::Alpha))
        .count();

    let pixel_format = JxlPixelFormat::rgba8(num_extra);
    decoder.set_pixel_format(pixel_format);

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
    let bytes_per_row = width * 4;
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

    let pixels = if has_alpha {
        let rgba_pixels: Vec<rgb::Rgba<u8>> = buf
            .chunks_exact(4)
            .map(|c| rgb::Rgba {
                r: c[0],
                g: c[1],
                b: c[2],
                a: c[3],
            })
            .collect();
        PixelData::Rgba8(ImgVec::new(rgba_pixels, width, height))
    } else {
        let rgb_pixels: Vec<rgb::Rgb<u8>> = buf
            .chunks_exact(4)
            .map(|c| rgb::Rgb {
                r: c[0],
                g: c[1],
                b: c[2],
            })
            .collect();
        PixelData::Rgb8(ImgVec::new(rgb_pixels, width, height))
    };

    Ok(JxlDecodeOutput {
        pixels,
        info: JxlInfo {
            width: width as u32,
            height: height as u32,
            has_alpha,
            has_animation,
            bit_depth: Some(bit_depth),
            icc_profile,
            orientation,
            cicp,
        },
    })
}
