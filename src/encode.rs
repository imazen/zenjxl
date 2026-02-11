//! JXL encoding via jxl-encoder.

use alloc::vec::Vec;
use imgref::ImgRef;
use rgb::{Gray, Rgb, Rgba};

use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};

use crate::error::JxlError;

/// Encode RGB8 pixels to lossy JXL.
pub fn encode_rgb8(img: ImgRef<Rgb<u8>>, config: &LossyConfig) -> Result<Vec<u8>, JxlError> {
    let (buf, w, h) = img.to_contiguous_buf();
    let bytes = rgb_to_bytes(&buf);
    let data = config
        .encode(&bytes, w as u32, h as u32, PixelLayout::Rgb8)
        .map_err(|e| e.into_inner())?;
    Ok(data)
}

/// Encode RGBA8 pixels to lossy JXL.
pub fn encode_rgba8(img: ImgRef<Rgba<u8>>, config: &LossyConfig) -> Result<Vec<u8>, JxlError> {
    let (buf, w, h) = img.to_contiguous_buf();
    let bytes = rgba_to_bytes(&buf);
    let data = config
        .encode(&bytes, w as u32, h as u32, PixelLayout::Rgba8)
        .map_err(|e| e.into_inner())?;
    Ok(data)
}

/// Encode Gray8 pixels to lossy JXL (expanded to RGB).
pub fn encode_gray8(img: ImgRef<Gray<u8>>, config: &LossyConfig) -> Result<Vec<u8>, JxlError> {
    let (buf, w, h) = img.to_contiguous_buf();
    let bytes = gray_to_rgb_bytes(&buf);
    let data = config
        .encode(&bytes, w as u32, h as u32, PixelLayout::Rgb8)
        .map_err(|e| e.into_inner())?;
    Ok(data)
}

/// Encode RGB8 pixels to lossless JXL.
pub fn encode_rgb8_lossless(
    img: ImgRef<Rgb<u8>>,
    config: &LosslessConfig,
) -> Result<Vec<u8>, JxlError> {
    let (buf, w, h) = img.to_contiguous_buf();
    let bytes = rgb_to_bytes(&buf);
    let data = config
        .encode(&bytes, w as u32, h as u32, PixelLayout::Rgb8)
        .map_err(|e| e.into_inner())?;
    Ok(data)
}

/// Encode RGBA8 pixels to lossless JXL.
pub fn encode_rgba8_lossless(
    img: ImgRef<Rgba<u8>>,
    config: &LosslessConfig,
) -> Result<Vec<u8>, JxlError> {
    let (buf, w, h) = img.to_contiguous_buf();
    let bytes = rgba_to_bytes(&buf);
    let data = config
        .encode(&bytes, w as u32, h as u32, PixelLayout::Rgba8)
        .map_err(|e| e.into_inner())?;
    Ok(data)
}

/// Encode Gray8 pixels to lossless JXL.
pub fn encode_gray8_lossless(
    img: ImgRef<Gray<u8>>,
    config: &LosslessConfig,
) -> Result<Vec<u8>, JxlError> {
    let (buf, w, h) = img.to_contiguous_buf();
    let bytes: Vec<u8> = buf.iter().map(|g| g.value()).collect();
    let data = config
        .encode(&bytes, w as u32, h as u32, PixelLayout::Gray8)
        .map_err(|e| e.into_inner())?;
    Ok(data)
}

pub(crate) fn rgb_to_bytes(pixels: &[Rgb<u8>]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pixels.len() * 3);
    for p in pixels {
        bytes.push(p.r);
        bytes.push(p.g);
        bytes.push(p.b);
    }
    bytes
}

pub(crate) fn rgba_to_bytes(pixels: &[Rgba<u8>]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pixels.len() * 4);
    for p in pixels {
        bytes.push(p.r);
        bytes.push(p.g);
        bytes.push(p.b);
        bytes.push(p.a);
    }
    bytes
}

pub(crate) fn gray_to_rgb_bytes(pixels: &[Gray<u8>]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pixels.len() * 3);
    for g in pixels {
        let v = g.value();
        bytes.push(v);
        bytes.push(v);
        bytes.push(v);
    }
    bytes
}
