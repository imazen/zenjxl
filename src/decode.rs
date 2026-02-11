//! JXL decoding and probing via jxl-rs.

use alloc::vec;
use alloc::vec::Vec;

use imgref::ImgVec;
use zencodec_types::PixelData;

use jxl::api::{JxlDecoder, JxlDecoderOptions, JxlOutputBuffer, JxlPixelFormat, ProcessingResult};
use jxl::headers::extra_channels::ExtraChannel;

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
    /// Embedded ICC color profile.
    pub icc_profile: Option<Vec<u8>>,
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

fn map_err(e: jxl::error::Error) -> JxlError {
    JxlError::Decode(e)
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

    Ok(JxlInfo {
        width: width as u32,
        height: height as u32,
        has_alpha,
        has_animation,
        icc_profile: None,
    })
}

/// Decode JXL to pixels.
pub fn decode(data: &[u8], limits: Option<&JxlLimits>) -> Result<JxlDecodeOutput, JxlError> {
    let mut options = JxlDecoderOptions::default();

    if let Some(lim) = limits {
        if let Some(max_px) = lim.max_pixels {
            options.pixel_limit = Some(max_px as usize);
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
            icc_profile: None,
        },
    })
}
