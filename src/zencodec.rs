//! zencodec-types trait implementations for JPEG XL.
//!
//! [`JxlEncoderConfig`] implements [`EncoderConfig`](zencodec_types::EncoderConfig)
//! (encode feature).
//! [`JxlDecoderConfig`] implements [`DecoderConfig`](zencodec_types::DecoderConfig)
//! (decode feature).

#[cfg(feature = "encode")]
use alloc::vec::Vec;

#[cfg(feature = "decode")]
use rgb::{Gray, Rgb, Rgba};

use zencodec_types::{
    CodecCapabilities, ImageFormat, PixelDescriptor, PixelSlice, PixelSliceMut, ResourceLimits,
    Stop,
};
#[cfg(feature = "decode")]
use zencodec_types::{DecodeOutput, ImageInfo, OutputInfo};
#[cfg(feature = "encode")]
use zencodec_types::{EncodeOutput, ImageMetadata};

use crate::error::JxlError;

// ── Encoding ────────────────────────────────────────────────────────────────

#[cfg(feature = "encode")]
mod encoding {
    use super::*;
    use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};

    /// Internal: lossy or lossless JXL config.
    #[derive(Clone, Debug)]
    enum JxlConfig {
        Lossy(LossyConfig),
        Lossless(LosslessConfig),
    }

    /// JPEG XL encoder configuration implementing [`zencodec_types::EncoderConfig`].
    ///
    /// Wraps [`LossyConfig`] or [`LosslessConfig`]. Defaults to lossy at distance 1.0.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use zencodec_types::EncoderConfig;
    /// use zenjxl::JxlEncoderConfig;
    ///
    /// let config = JxlEncoderConfig::lossy(1.0).with_effort(7);
    /// let output = config.encode_rgb8(img.as_ref()).unwrap();
    /// ```
    #[derive(Clone, Debug)]
    pub struct JxlEncoderConfig {
        config: JxlConfig,
        quality: Option<f32>,
        effort: Option<i32>,
    }

    impl JxlEncoderConfig {
        /// Create a lossy encoder config with the given butteraugli distance.
        #[must_use]
        pub fn lossy(distance: f32) -> Self {
            Self {
                config: JxlConfig::Lossy(LossyConfig::new(distance)),
                quality: None,
                effort: None,
            }
        }

        /// Create a lossless encoder config.
        #[must_use]
        pub fn lossless() -> Self {
            Self {
                config: JxlConfig::Lossless(LosslessConfig::new()),
                quality: None,
                effort: None,
            }
        }

        /// Access the underlying lossy config (if lossy mode).
        #[must_use]
        pub fn lossy_config(&self) -> Option<&LossyConfig> {
            match &self.config {
                JxlConfig::Lossy(c) => Some(c),
                JxlConfig::Lossless(_) => None,
            }
        }

        /// Access the underlying lossless config (if lossless mode).
        #[must_use]
        pub fn lossless_config(&self) -> Option<&LosslessConfig> {
            match &self.config {
                JxlConfig::Lossy(_) => None,
                JxlConfig::Lossless(c) => Some(c),
            }
        }

        /// Set quality as 0-100 percentage (inherent method).
        ///
        /// 100+ switches to lossless mode. Lower values map to higher butteraugli
        /// distances.
        #[must_use]
        pub fn with_quality(mut self, quality: f32) -> Self {
            if quality >= 100.0 {
                self.config = JxlConfig::Lossless(LosslessConfig::new());
            } else {
                let distance = percent_to_distance(quality);
                self.config = match self.config {
                    JxlConfig::Lossy(c) => {
                        JxlConfig::Lossy(LossyConfig::new(distance).with_effort(c.effort()))
                    }
                    JxlConfig::Lossless(_) => JxlConfig::Lossy(LossyConfig::new(distance)),
                };
            }
            self.quality = Some(quality);
            self
        }

        /// Set encoding effort 1-10 (inherent method).
        #[must_use]
        pub fn with_effort(mut self, effort: u32) -> Self {
            let effort_u8 = (effort.min(10)) as u8;
            self.config = match self.config {
                JxlConfig::Lossy(c) => JxlConfig::Lossy(c.with_effort(effort_u8)),
                JxlConfig::Lossless(c) => JxlConfig::Lossless(c.with_effort(effort_u8)),
            };
            self.effort = Some(effort as i32);
            self
        }

        /// Switch between lossy and lossless mode (inherent method).
        #[must_use]
        pub fn with_lossless(mut self, lossless: bool) -> Self {
            if lossless {
                let effort = match &self.config {
                    JxlConfig::Lossy(c) => c.effort(),
                    JxlConfig::Lossless(c) => c.effort(),
                };
                self.config = JxlConfig::Lossless(LosslessConfig::new().with_effort(effort));
            } else {
                let effort = match &self.config {
                    JxlConfig::Lossy(c) => c.effort(),
                    JxlConfig::Lossless(c) => c.effort(),
                };
                self.config = JxlConfig::Lossy(LossyConfig::new(1.0).with_effort(effort));
            }
            self
        }
    }

    impl Default for JxlEncoderConfig {
        fn default() -> Self {
            Self::lossy(1.0)
        }
    }

    static ENCODE_CAPS: CodecCapabilities = CodecCapabilities::new()
        .with_encode_icc(true)
        .with_encode_exif(true)
        .with_encode_xmp(true)
        .with_encode_cancel(true)
        .with_cheap_probe(true)
        .with_lossless(true)
        .with_effort_range(1, 10)
        .with_quality_range(0.0, 100.0);

    impl zencodec_types::EncoderConfig for JxlEncoderConfig {
        type Error = JxlError;
        type Job<'a> = JxlEncodeJob<'a>;

        fn supported_descriptors() -> &'static [PixelDescriptor] {
            &[
                PixelDescriptor::RGB8_SRGB,
                PixelDescriptor::RGBA8_SRGB,
                PixelDescriptor::GRAY8_SRGB,
                PixelDescriptor::BGRA8_SRGB,
                PixelDescriptor::RGBF32_LINEAR,
                PixelDescriptor::RGBAF32_LINEAR,
                PixelDescriptor::GRAYF32_LINEAR,
            ]
        }

        fn capabilities() -> &'static CodecCapabilities {
            &ENCODE_CAPS
        }

        fn calibrated_quality(&self) -> Option<f32> {
            self.quality
        }

        fn with_calibrated_quality(mut self, quality: f32) -> Self {
            self = self.with_quality(quality);
            self
        }

        fn effort(&self) -> Option<i32> {
            self.effort
        }

        fn with_effort(mut self, effort: i32) -> Self {
            let clamped = effort.clamp(1, 10) as u32;
            self = JxlEncoderConfig::with_effort(self, clamped);
            self
        }

        fn is_lossless(&self) -> Option<bool> {
            Some(matches!(self.config, JxlConfig::Lossless(_)))
        }

        fn with_lossless(self, lossless: bool) -> Self {
            JxlEncoderConfig::with_lossless(self, lossless)
        }

        fn job(&self) -> JxlEncodeJob<'_> {
            JxlEncodeJob {
                config: self,
                stop: None,
                icc: None,
                exif: None,
                xmp: None,
                limits: ResourceLimits::none(),
            }
        }
    }

    /// Per-operation JXL encode job.
    pub struct JxlEncodeJob<'a> {
        config: &'a JxlEncoderConfig,
        stop: Option<&'a dyn Stop>,
        icc: Option<&'a [u8]>,
        exif: Option<&'a [u8]>,
        xmp: Option<&'a [u8]>,
        limits: ResourceLimits,
    }

    impl<'a> JxlEncodeJob<'a> {
        /// Set ICC profile for this encode job (inherent method).
        #[must_use]
        pub fn with_icc(mut self, icc: &'a [u8]) -> Self {
            self.icc = Some(icc);
            self
        }

        /// Set EXIF data for this encode job (inherent method).
        #[must_use]
        pub fn with_exif(mut self, exif: &'a [u8]) -> Self {
            self.exif = Some(exif);
            self
        }

        /// Set XMP data for this encode job (inherent method).
        #[must_use]
        pub fn with_xmp(mut self, xmp: &'a [u8]) -> Self {
            self.xmp = Some(xmp);
            self
        }

        fn do_encode(
            self,
            pixels: &[u8],
            layout: PixelLayout,
            w: u32,
            h: u32,
        ) -> Result<EncodeOutput, JxlError> {
            let meta;
            let has_meta = self.icc.is_some() || self.exif.is_some() || self.xmp.is_some();
            if has_meta {
                let mut m = jxl_encoder::ImageMetadata::new();
                if let Some(icc) = self.icc {
                    m = m.with_icc_profile(icc);
                }
                if let Some(exif) = self.exif {
                    m = m.with_exif(exif);
                }
                if let Some(xmp) = self.xmp {
                    m = m.with_xmp(xmp);
                }
                meta = Some(m);
            } else {
                meta = None;
            }

            // Merge limits: job-level overrides config-level per field.
            let merged_pixels = self.limits.max_pixels;
            let merged_memory = self.limits.max_memory_bytes;
            let limits;
            let has_limits = merged_pixels.is_some() || merged_memory.is_some();
            if has_limits {
                let mut l = jxl_encoder::Limits::new();
                if let Some(p) = merged_pixels {
                    l = l.with_max_pixels(p);
                }
                if let Some(m) = merged_memory {
                    l = l.with_max_memory_bytes(m);
                }
                limits = Some(l);
            } else {
                limits = None;
            }

            let data = match &self.config.config {
                JxlConfig::Lossy(cfg) => {
                    let mut req = cfg.encode_request(w, h, layout);
                    if let Some(ref m) = meta {
                        req = req.with_metadata(m);
                    }
                    if let Some(ref l) = limits {
                        req = req.with_limits(l);
                    }
                    if let Some(stop) = self.stop {
                        req = req.with_stop(stop);
                    }
                    req.encode(pixels).map_err(|e| e.into_inner())?
                }
                JxlConfig::Lossless(cfg) => {
                    let mut req = cfg.encode_request(w, h, layout);
                    if let Some(ref m) = meta {
                        req = req.with_metadata(m);
                    }
                    if let Some(ref l) = limits {
                        req = req.with_limits(l);
                    }
                    if let Some(stop) = self.stop {
                        req = req.with_stop(stop);
                    }
                    req.encode(pixels).map_err(|e| e.into_inner())?
                }
            };

            Ok(EncodeOutput::new(data, ImageFormat::Jxl))
        }
    }

    impl<'a> zencodec_types::EncodeJob<'a> for JxlEncodeJob<'a> {
        type Error = JxlError;
        type Encoder = JxlEncoder<'a>;
        type FrameEncoder = JxlFrameEncoder;

        fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
            self.stop = Some(stop);
            self
        }

        fn with_metadata(mut self, meta: &'a ImageMetadata<'a>) -> Self {
            if let Some(icc) = meta.icc_profile {
                self.icc = Some(icc);
            }
            if let Some(exif) = meta.exif {
                self.exif = Some(exif);
            }
            if let Some(xmp) = meta.xmp {
                self.xmp = Some(xmp);
            }
            self
        }

        fn with_limits(mut self, limits: ResourceLimits) -> Self {
            self.limits = limits;
            self
        }

        fn encoder(self) -> JxlEncoder<'a> {
            JxlEncoder { job: self }
        }

        fn frame_encoder(self) -> Result<JxlFrameEncoder, JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL does not support animation encoding via this API".into(),
            ))
        }
    }

    /// JPEG XL single-image encoder.
    pub struct JxlEncoder<'a> {
        job: JxlEncodeJob<'a>,
    }

    impl<'a> zencodec_types::Encoder for JxlEncoder<'a> {
        type Error = JxlError;

        fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, JxlError> {
            let d = pixels.descriptor();
            let w = pixels.width();
            let h = pixels.rows();

            if d == PixelDescriptor::RGB8_SRGB {
                let raw = collect_contiguous_bytes(&pixels);
                self.job.do_encode(&raw, PixelLayout::Rgb8, w, h)
            } else if d == PixelDescriptor::RGBA8_SRGB {
                let raw = collect_contiguous_bytes(&pixels);
                self.job.do_encode(&raw, PixelLayout::Rgba8, w, h)
            } else if d == PixelDescriptor::BGRA8_SRGB {
                let raw = collect_contiguous_bytes(&pixels);
                self.job.do_encode(&raw, PixelLayout::Bgra8, w, h)
            } else if d == PixelDescriptor::GRAY8_SRGB {
                // Gray8 handling depends on lossy vs lossless
                let raw = collect_contiguous_bytes(&pixels);
                match &self.job.config.config {
                    JxlConfig::Lossless(_) => self.job.do_encode(&raw, PixelLayout::Gray8, w, h),
                    JxlConfig::Lossy(_) => {
                        // Expand gray to RGB for lossy
                        let rgb: Vec<u8> = raw.iter().flat_map(|&g| [g, g, g]).collect();
                        self.job.do_encode(&rgb, PixelLayout::Rgb8, w, h)
                    }
                }
            } else if d == PixelDescriptor::RGBF32_LINEAR {
                // JXL natively supports linear f32 RGB
                let raw = collect_contiguous_bytes(&pixels);
                self.job.do_encode(&raw, PixelLayout::RgbLinearF32, w, h)
            } else if d == PixelDescriptor::RGBAF32_LINEAR {
                // No native RGBA f32 layout — convert linear→sRGB u8, encode as RGBA8
                use linear_srgb::default::linear_to_srgb_u8;
                let raw = collect_contiguous_bytes(&pixels);
                let floats: &[f32] = bytemuck::cast_slice(&raw);
                let rgba: Vec<u8> = floats
                    .chunks_exact(4)
                    .flat_map(|p| {
                        [
                            linear_to_srgb_u8(p[0].clamp(0.0, 1.0)),
                            linear_to_srgb_u8(p[1].clamp(0.0, 1.0)),
                            linear_to_srgb_u8(p[2].clamp(0.0, 1.0)),
                            (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        ]
                    })
                    .collect();
                self.job.do_encode(&rgba, PixelLayout::Rgba8, w, h)
            } else if d == PixelDescriptor::GRAYF32_LINEAR {
                // Convert linear gray → sRGB u8
                use linear_srgb::default::linear_to_srgb_u8;
                let raw = collect_contiguous_bytes(&pixels);
                let floats: &[f32] = bytemuck::cast_slice(&raw);
                match &self.job.config.config {
                    JxlConfig::Lossless(_) => {
                        let bytes: Vec<u8> = floats
                            .iter()
                            .map(|g| linear_to_srgb_u8(g.clamp(0.0, 1.0)))
                            .collect();
                        self.job.do_encode(&bytes, PixelLayout::Gray8, w, h)
                    }
                    JxlConfig::Lossy(_) => {
                        let bytes: Vec<u8> = floats
                            .iter()
                            .flat_map(|g| {
                                let v = linear_to_srgb_u8(g.clamp(0.0, 1.0));
                                [v, v, v]
                            })
                            .collect();
                        self.job.do_encode(&bytes, PixelLayout::Rgb8, w, h)
                    }
                }
            } else {
                Err(JxlError::InvalidInput(alloc::format!(
                    "unsupported pixel format: {:?}",
                    d
                )))
            }
        }

        fn push_rows(&mut self, _rows: PixelSlice<'_>) -> Result<(), JxlError> {
            Err(JxlError::InvalidInput(
                "JXL encoder does not support row-level push".into(),
            ))
        }

        fn finish(self) -> Result<EncodeOutput, JxlError> {
            Err(JxlError::InvalidInput(
                "JXL encoder does not support row-level push/finish".into(),
            ))
        }

        fn encode_from(
            self,
            _source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
        ) -> Result<EncodeOutput, JxlError> {
            Err(JxlError::InvalidInput(
                "JXL encoder does not support pull-based encoding".into(),
            ))
        }
    }

    /// Stub frame encoder (JXL doesn't support animation encoding via this API).
    pub struct JxlFrameEncoder;

    impl zencodec_types::FrameEncoder for JxlFrameEncoder {
        type Error = JxlError;

        fn push_frame(
            &mut self,
            _pixels: PixelSlice<'_>,
            _duration_ms: u32,
        ) -> Result<(), JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL does not support animation encoding via this API".into(),
            ))
        }

        fn begin_frame(&mut self, _duration_ms: u32) -> Result<(), JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL does not support animation encoding via this API".into(),
            ))
        }

        fn push_rows(&mut self, _rows: PixelSlice<'_>) -> Result<(), JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL does not support animation encoding via this API".into(),
            ))
        }

        fn end_frame(&mut self) -> Result<(), JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL does not support animation encoding via this API".into(),
            ))
        }

        fn pull_frame(
            &mut self,
            _duration_ms: u32,
            _source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
        ) -> Result<(), JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL does not support animation encoding via this API".into(),
            ))
        }

        fn finish(self) -> Result<EncodeOutput, JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL does not support animation encoding via this API".into(),
            ))
        }
    }

    /// Collect contiguous bytes from a PixelSlice, row by row.
    fn collect_contiguous_bytes(pixels: &PixelSlice<'_>) -> Vec<u8> {
        let h = pixels.rows() as usize;
        let row_len = pixels.width() as usize * pixels.descriptor().bytes_per_pixel();
        let mut buf = Vec::with_capacity(h * row_len);
        for y in 0..h {
            buf.extend_from_slice(pixels.row(y as u32));
        }
        buf
    }

    /// Map 0-100 quality percentage to butteraugli distance.
    fn percent_to_distance(quality: f32) -> f32 {
        let q = quality.clamp(0.0, 99.9) as u32;
        if q >= 90 {
            (100 - q) as f32 / 10.0
        } else if q >= 70 {
            1.0 + (90 - q) as f32 / 20.0
        } else {
            2.0 + (70 - q) as f32 / 10.0
        }
    }
}

#[cfg(feature = "encode")]
pub use encoding::{JxlEncodeJob, JxlEncoder, JxlEncoderConfig, JxlFrameEncoder};

// ── Decoding ────────────────────────────────────────────────────────────────

#[cfg(feature = "decode")]
mod decoding {
    use super::*;

    /// JPEG XL decoder configuration implementing [`zencodec_types::DecoderConfig`].
    #[derive(Clone, Debug)]
    pub struct JxlDecoderConfig {
        limits: ResourceLimits,
    }

    impl JxlDecoderConfig {
        /// Create a default JXL decoder config.
        #[must_use]
        pub fn new() -> Self {
            Self {
                limits: ResourceLimits::none(),
            }
        }

        /// Set resource limits.
        #[must_use]
        pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
            self.limits = limits;
            self
        }
    }

    impl Default for JxlDecoderConfig {
        fn default() -> Self {
            Self::new()
        }
    }

    static DECODE_CAPS: CodecCapabilities = CodecCapabilities::new()
        .with_decode_icc(true)
        .with_cheap_probe(true);

    impl zencodec_types::DecoderConfig for JxlDecoderConfig {
        type Error = JxlError;
        type Job<'a> = JxlDecodeJob<'a>;

        fn supported_descriptors() -> &'static [PixelDescriptor] {
            &[
                PixelDescriptor::RGB8_SRGB,
                PixelDescriptor::RGBA8_SRGB,
                PixelDescriptor::GRAY8_SRGB,
                PixelDescriptor::BGRA8_SRGB,
                PixelDescriptor::RGBF32_LINEAR,
                PixelDescriptor::RGBAF32_LINEAR,
                PixelDescriptor::GRAYF32_LINEAR,
            ]
        }

        fn capabilities() -> &'static CodecCapabilities {
            &DECODE_CAPS
        }

        fn job(&self) -> JxlDecodeJob<'_> {
            JxlDecodeJob {
                config: self,
                limits: ResourceLimits::none(),
            }
        }

        fn probe_header(&self, data: &[u8]) -> Result<ImageInfo, Self::Error> {
            let info = crate::decode::probe(data)?;
            Ok(convert_info(&info))
        }
    }

    /// Per-operation JXL decode job.
    pub struct JxlDecodeJob<'a> {
        config: &'a JxlDecoderConfig,
        limits: ResourceLimits,
    }

    impl<'a> zencodec_types::DecodeJob<'a> for JxlDecodeJob<'a> {
        type Error = JxlError;
        type Decoder = JxlDecoder<'a>;
        type FrameDecoder = JxlFrameDecoder;

        fn with_stop(self, _stop: &'a dyn Stop) -> Self {
            self // JXL decoding is not cancellable
        }

        fn with_limits(mut self, limits: ResourceLimits) -> Self {
            self.limits = limits;
            self
        }

        fn output_info(&self, data: &[u8]) -> Result<OutputInfo, JxlError> {
            let info = crate::decode::probe(data)?;
            Ok(OutputInfo::full_decode(
                info.width,
                info.height,
                PixelDescriptor::RGB8_SRGB,
            ))
        }

        fn decoder(self) -> JxlDecoder<'a> {
            JxlDecoder {
                config: self.config,
                limits: self.limits,
            }
        }

        fn frame_decoder(self, _data: &[u8]) -> Result<JxlFrameDecoder, JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL animation decoding not yet supported via this API".into(),
            ))
        }
    }

    /// JPEG XL single-image decoder.
    pub struct JxlDecoder<'a> {
        config: &'a JxlDecoderConfig,
        limits: ResourceLimits,
    }

    impl<'a> zencodec_types::Decoder for JxlDecoder<'a> {
        type Error = JxlError;

        fn decode(self, data: &[u8]) -> Result<DecodeOutput, JxlError> {
            let merged_limits = self.merge_limits();
            let result = crate::decode::decode(data, merged_limits.as_ref())?;
            let info = convert_info(&result.info);
            Ok(DecodeOutput::new(result.pixels, info))
        }

        fn decode_into(
            self,
            data: &[u8],
            mut dst: PixelSliceMut<'_>,
        ) -> Result<ImageInfo, JxlError> {
            let d = dst.descriptor();

            // Decode to native format first
            let merged_limits = self.merge_limits();
            let result = crate::decode::decode(data, merged_limits.as_ref())?;
            let info = convert_info(&result.info);
            let src = result.pixels.into_rgb8();
            let w = src.width().min(dst.width() as usize);
            let h = src.height().min(dst.rows() as usize);

            if d == PixelDescriptor::RGB8_SRGB {
                for y in 0..h {
                    let src_row = &src.as_ref().rows().nth(y).unwrap();
                    let dst_row = dst.row_mut(y as u32);
                    let row_bytes: &[u8] = bytemuck::cast_slice(&src_row[..w]);
                    dst_row[..row_bytes.len()].copy_from_slice(row_bytes);
                }
            } else if d == PixelDescriptor::RGBA8_SRGB {
                for y in 0..h {
                    let src_row = &src.as_ref().rows().nth(y).unwrap();
                    let dst_row = dst.row_mut(y as u32);
                    let dst_pixels: &mut [Rgba<u8>] = bytemuck::cast_slice_mut(dst_row);
                    for (i, s) in src_row[..w].iter().enumerate() {
                        dst_pixels[i] = Rgba {
                            r: s.r,
                            g: s.g,
                            b: s.b,
                            a: 255,
                        };
                    }
                }
            } else if d == PixelDescriptor::GRAY8_SRGB {
                for y in 0..h {
                    let src_row = &src.as_ref().rows().nth(y).unwrap();
                    let dst_row = dst.row_mut(y as u32);
                    for (i, s) in src_row[..w].iter().enumerate() {
                        let luma =
                            ((s.r as u16 * 77 + s.g as u16 * 150 + s.b as u16 * 29) >> 8) as u8;
                        dst_row[i] = luma;
                    }
                }
            } else if d == PixelDescriptor::BGRA8_SRGB {
                for y in 0..h {
                    let src_row = &src.as_ref().rows().nth(y).unwrap();
                    let dst_row = dst.row_mut(y as u32);
                    let dst_pixels: &mut [[u8; 4]] = bytemuck::cast_slice_mut(dst_row);
                    for (i, s) in src_row[..w].iter().enumerate() {
                        dst_pixels[i] = [s.b, s.g, s.r, 255];
                    }
                }
            } else if d == PixelDescriptor::RGBF32_LINEAR {
                use linear_srgb::default::srgb_u8_to_linear;
                for y in 0..h {
                    let src_row = &src.as_ref().rows().nth(y).unwrap();
                    let dst_row = dst.row_mut(y as u32);
                    let dst_pixels: &mut [Rgb<f32>] = bytemuck::cast_slice_mut(dst_row);
                    for (i, s) in src_row[..w].iter().enumerate() {
                        dst_pixels[i] = Rgb {
                            r: srgb_u8_to_linear(s.r),
                            g: srgb_u8_to_linear(s.g),
                            b: srgb_u8_to_linear(s.b),
                        };
                    }
                }
            } else if d == PixelDescriptor::RGBAF32_LINEAR {
                use linear_srgb::default::srgb_u8_to_linear;
                for y in 0..h {
                    let src_row = &src.as_ref().rows().nth(y).unwrap();
                    let dst_row = dst.row_mut(y as u32);
                    let dst_pixels: &mut [Rgba<f32>] = bytemuck::cast_slice_mut(dst_row);
                    for (i, s) in src_row[..w].iter().enumerate() {
                        dst_pixels[i] = Rgba {
                            r: srgb_u8_to_linear(s.r),
                            g: srgb_u8_to_linear(s.g),
                            b: srgb_u8_to_linear(s.b),
                            a: 1.0,
                        };
                    }
                }
            } else if d == PixelDescriptor::GRAYF32_LINEAR {
                use linear_srgb::default::srgb_u8_to_linear;
                for y in 0..h {
                    let src_row = &src.as_ref().rows().nth(y).unwrap();
                    let dst_row = dst.row_mut(y as u32);
                    let dst_pixels: &mut [Gray<f32>] = bytemuck::cast_slice_mut(dst_row);
                    for (i, s) in src_row[..w].iter().enumerate() {
                        let r = srgb_u8_to_linear(s.r);
                        let g = srgb_u8_to_linear(s.g);
                        let b = srgb_u8_to_linear(s.b);
                        dst_pixels[i] = Gray::new(0.2126 * r + 0.7152 * g + 0.0722 * b);
                    }
                }
            } else {
                return Err(JxlError::InvalidInput(alloc::format!(
                    "unsupported pixel format: {:?}",
                    d
                )));
            }

            Ok(info)
        }

        fn decode_rows(
            self,
            data: &[u8],
            _sink: &mut dyn FnMut(u32, PixelSlice<'_>),
        ) -> Result<ImageInfo, JxlError> {
            // JXL decoder doesn't support streaming rows — decode fully
            let output = zencodec_types::Decoder::decode(self, data)?;
            Ok(output.info().clone())
        }
    }

    impl<'a> JxlDecoder<'a> {
        fn merge_limits(&self) -> Option<crate::decode::JxlLimits> {
            let merged_pixels = self.limits.max_pixels.or(self.config.limits.max_pixels);
            let merged_memory = self
                .limits
                .max_memory_bytes
                .or(self.config.limits.max_memory_bytes);

            if merged_pixels.is_some() || merged_memory.is_some() {
                Some(crate::decode::JxlLimits {
                    max_pixels: merged_pixels,
                    max_memory_bytes: merged_memory,
                })
            } else {
                None
            }
        }
    }

    /// Stub frame decoder (JXL animation not yet supported via this API).
    pub struct JxlFrameDecoder;

    impl zencodec_types::FrameDecoder for JxlFrameDecoder {
        type Error = JxlError;

        fn next_frame(&mut self) -> Result<Option<zencodec_types::DecodeFrame>, JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL animation decoding not yet supported via this API".into(),
            ))
        }

        fn next_frame_into(
            &mut self,
            _dst: PixelSliceMut<'_>,
            _prior_frame: Option<u32>,
        ) -> Result<Option<ImageInfo>, JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL animation decoding not yet supported via this API".into(),
            ))
        }

        fn next_frame_rows(
            &mut self,
            _sink: &mut dyn FnMut(u32, PixelSlice<'_>),
        ) -> Result<Option<ImageInfo>, JxlError> {
            Err(JxlError::InvalidInput(
                "JPEG XL animation decoding not yet supported via this API".into(),
            ))
        }
    }

    fn convert_info(info: &crate::decode::JxlInfo) -> ImageInfo {
        let mut zi = ImageInfo::new(info.width, info.height, ImageFormat::Jxl);
        if info.has_alpha {
            zi = zi.with_alpha(true);
        }
        if info.has_animation {
            zi = zi.with_animation(true);
        }
        if let Some(ref icc) = info.icc_profile {
            zi = zi.with_icc_profile(icc.clone());
        }
        zi
    }
}

#[cfg(feature = "decode")]
pub use decoding::{JxlDecodeJob, JxlDecoder, JxlDecoderConfig, JxlFrameDecoder};

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use alloc::vec;
    use imgref::Img;

    #[cfg(feature = "encode")]
    use super::*;

    #[test]
    #[cfg(feature = "encode")]
    fn encoding_lossy_default() {
        use zencodec_types::EncoderConfig;
        let enc = JxlEncoderConfig::lossy(1.0);
        let pixels = vec![
            Rgb {
                r: 128,
                g: 64,
                b: 32
            };
            64
        ];
        let img = Img::new(pixels, 8, 8);
        let output = enc.encode_rgb8(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());
        assert_eq!(output.format(), ImageFormat::Jxl);
        assert_eq!(&output.bytes()[0..2], &[0xFF, 0x0A]);
    }

    #[test]
    #[cfg(feature = "encode")]
    fn encoding_lossless() {
        use zencodec_types::EncoderConfig;
        let enc = JxlEncoderConfig::lossless();
        let pixels = vec![
            Rgb {
                r: 100,
                g: 200,
                b: 50
            };
            16
        ];
        let img = Img::new(pixels, 4, 4);
        let output = enc.encode_rgb8(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());
    }

    #[test]
    #[cfg(feature = "encode")]
    fn encoding_quality_100_becomes_lossless() {
        let enc = JxlEncoderConfig::default().with_quality(100.0);
        assert!(enc.lossless_config().is_some());
    }

    #[test]
    #[cfg(feature = "encode")]
    fn effort_and_quality_getters() {
        use zencodec_types::EncoderConfig;
        let enc = JxlEncoderConfig::lossy(1.0)
            .with_calibrated_quality(75.0)
            .with_effort(7);
        assert_eq!(enc.effort(), Some(7));
        assert_eq!(enc.calibrated_quality(), Some(75.0));
        assert_eq!(enc.is_lossless(), Some(false));

        let enc = enc.with_lossless(true);
        assert_eq!(enc.is_lossless(), Some(true));
        // Effort is preserved across lossless switch
        assert_eq!(enc.effort(), Some(7));
    }

    #[test]
    #[cfg(all(feature = "encode", feature = "decode"))]
    fn roundtrip() {
        use zencodec_types::{DecoderConfig, EncoderConfig};
        let enc = JxlEncoderConfig::lossless();
        let pixels = vec![
            Rgb {
                r: 200,
                g: 100,
                b: 50
            };
            16
        ];
        let img = Img::new(pixels, 4, 4);
        let encoded = enc.encode_rgb8(img.as_ref()).unwrap();

        let dec = JxlDecoderConfig::new();
        let output = dec.decode(encoded.bytes()).unwrap();
        assert_eq!(output.info().width, 4);
        assert_eq!(output.info().height, 4);
        assert_eq!(output.info().format, ImageFormat::Jxl);
    }

    #[test]
    #[cfg(all(feature = "encode", feature = "decode"))]
    fn f32_roundtrip_all_simd_tiers() {
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        use imgref::ImgVec;
        use zencodec_types::{DecoderConfig, EncoderConfig};

        let report = for_each_token_permutation(CompileTimePolicy::Warn, |_perm| {
            // Encode linear f32 → JXL (native f32 path) → decode back to f32
            let pixels: Vec<Rgb<f32>> = (0..16 * 16)
                .map(|i| {
                    let t = i as f32 / 255.0;
                    Rgb {
                        r: t,
                        g: (t * 0.7),
                        b: (t * 0.3),
                    }
                })
                .collect();
            let img = ImgVec::new(pixels, 16, 16);

            // Use lossy with small distance for near-lossless f32 encoding
            let enc = JxlEncoderConfig::lossy(1.0);
            let output = enc.encode_rgb_f32(img.as_ref()).unwrap();
            assert!(!output.bytes().is_empty());

            let dec = JxlDecoderConfig::new();
            let dst = vec![
                Rgb {
                    r: 0.0f32,
                    g: 0.0,
                    b: 0.0,
                };
                16 * 16
            ];
            let mut dst_img = ImgVec::new(dst, 16, 16);
            let _info = dec
                .decode_into_rgb_f32(output.bytes(), dst_img.as_mut())
                .unwrap();

            // Verify values are in valid range
            for p in dst_img.buf().iter() {
                assert!(p.r >= 0.0 && p.r <= 1.0, "r out of range: {}", p.r);
                assert!(p.g >= 0.0 && p.g <= 1.0, "g out of range: {}", p.g);
                assert!(p.b >= 0.0 && p.b <= 1.0, "b out of range: {}", p.b);
            }
        });
        assert!(report.permutations_run >= 1);
    }

    #[test]
    #[cfg(all(feature = "encode", feature = "decode"))]
    fn f32_rgba_decode_from_rgb() {
        use imgref::ImgVec;
        use zencodec_types::{DecoderConfig, EncoderConfig, Rgba};

        // Encode as RGB f32 (native path), decode into RGBA f32 buffer
        let pixels: Vec<Rgb<f32>> = (0..16 * 16)
            .map(|i| {
                let t = i as f32 / 255.0;
                Rgb {
                    r: t,
                    g: (t * 0.7),
                    b: (t * 0.3),
                }
            })
            .collect();
        let img = ImgVec::new(pixels, 16, 16);

        let enc = JxlEncoderConfig::lossy(1.0);
        let output = enc.encode_rgb_f32(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());

        let dec = JxlDecoderConfig::new();
        let mut dst_img = ImgVec::new(
            vec![
                Rgba {
                    r: 0.0f32,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0
                };
                16 * 16
            ],
            16,
            16,
        );
        dec.decode_into_rgba_f32(output.bytes(), dst_img.as_mut())
            .unwrap();

        for p in dst_img.buf().iter() {
            assert!(p.r >= 0.0 && p.r <= 1.0, "r out of range: {}", p.r);
            assert!(p.g >= 0.0 && p.g <= 1.0, "g out of range: {}", p.g);
            assert!(p.b >= 0.0 && p.b <= 1.0, "b out of range: {}", p.b);
            assert!(p.a >= 0.0 && p.a <= 1.0, "a out of range: {}", p.a);
        }
    }

    #[test]
    #[cfg(all(feature = "encode", feature = "decode"))]
    fn f32_gray_roundtrip() {
        use imgref::ImgVec;
        use zencodec_types::{DecoderConfig, EncoderConfig, Gray};

        let pixels: Vec<Gray<f32>> = (0..16 * 16).map(|i| Gray(i as f32 / 255.0)).collect();
        let img = ImgVec::new(pixels, 16, 16);

        let enc = JxlEncoderConfig::lossy(1.0);
        let output = enc.encode_gray_f32(img.as_ref()).unwrap();
        assert!(!output.bytes().is_empty());

        let dec = JxlDecoderConfig::new();
        let mut dst_img = ImgVec::new(vec![Gray(0.0f32); 16 * 16], 16, 16);
        dec.decode_into_gray_f32(output.bytes(), dst_img.as_mut())
            .unwrap();

        for p in dst_img.buf().iter() {
            assert!(
                p.value() >= 0.0 && p.value() <= 1.0,
                "gray out of range: {}",
                p.value()
            );
        }
    }

    #[test]
    #[cfg(all(feature = "encode", feature = "decode"))]
    fn four_layer_encode_flow() {
        use zencodec_types::{EncodeJob, Encoder, EncoderConfig, PixelSlice};

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200
            };
            8 * 8
        ];
        let img = imgref::ImgVec::new(pixels, 8, 8);

        let config = JxlEncoderConfig::lossy(1.0);
        let output = config
            .job()
            .encoder()
            .encode(PixelSlice::from(img.as_ref()))
            .unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.format(), ImageFormat::Jxl);
    }

    #[test]
    #[cfg(all(feature = "encode", feature = "decode"))]
    fn four_layer_decode_flow() {
        use zencodec_types::{DecodeJob, Decoder, DecoderConfig, EncoderConfig};

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200
            };
            8 * 8
        ];
        let img = imgref::ImgVec::new(pixels, 8, 8);
        let encoded = JxlEncoderConfig::lossless()
            .encode_rgb8(img.as_ref())
            .unwrap();

        let config = JxlDecoderConfig::new();
        let decoded = config.job().decoder().decode(encoded.bytes()).unwrap();
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.height(), 8);
    }
}
