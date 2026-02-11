//! zencodec-types trait implementations for JPEG XL.
//!
//! [`JxlEncoding`] implements [`Encoding`] (encode feature).
//! [`JxlDecoding`] implements [`Decoding`] (decode feature).

use alloc::vec::Vec;

use imgref::ImgRef;
use rgb::{Gray, Rgb, Rgba};

use zencodec_types::{
    DecodeOutput as ZDecodeOutput, EncodeOutput as ZEncodeOutput, ImageFormat as ZImageFormat,
    ImageInfo as ZImageInfo, ImageMetadata as ZImageMetadata, Stop,
};

use crate::error::JxlError;

// ── Encoding ────────────────────────────────────────────────────────────────

#[cfg(feature = "encode")]
mod encoding {
    use super::*;
    use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};
    use zencodec_types::{Encoding, EncodingJob};

    /// Internal: lossy or lossless JXL config.
    #[derive(Clone, Debug)]
    enum JxlConfig {
        Lossy(LossyConfig),
        Lossless(LosslessConfig),
    }

    /// JPEG XL encoder configuration implementing [`Encoding`].
    ///
    /// Wraps [`LossyConfig`] or [`LosslessConfig`]. Defaults to lossy at distance 1.0.
    #[derive(Clone, Debug)]
    pub struct JxlEncoding {
        config: JxlConfig,
        limit_pixels: Option<u64>,
        limit_memory: Option<u64>,
        limit_output: Option<u64>,
    }

    impl JxlEncoding {
        /// Create a lossy encoder config with the given butteraugli distance.
        #[must_use]
        pub fn lossy(distance: f32) -> Self {
            Self {
                config: JxlConfig::Lossy(LossyConfig::new(distance)),
                limit_pixels: None,
                limit_memory: None,
                limit_output: None,
            }
        }

        /// Create a lossless encoder config.
        #[must_use]
        pub fn lossless() -> Self {
            Self {
                config: JxlConfig::Lossless(LosslessConfig::new()),
                limit_pixels: None,
                limit_memory: None,
                limit_output: None,
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
    }

    impl Default for JxlEncoding {
        fn default() -> Self {
            Self::lossy(1.0)
        }
    }

    impl Encoding for JxlEncoding {
        type Error = JxlError;
        type Job<'a> = JxlEncodeJob<'a>;

        fn with_quality(mut self, quality: f32) -> Self {
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
            self
        }

        fn with_effort(mut self, effort: u32) -> Self {
            let effort_u8 = (effort.min(10)) as u8;
            self.config = match self.config {
                JxlConfig::Lossy(c) => JxlConfig::Lossy(c.with_effort(effort_u8)),
                JxlConfig::Lossless(c) => JxlConfig::Lossless(c.with_effort(effort_u8)),
            };
            self
        }

        fn with_lossless(mut self, lossless: bool) -> Self {
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

        fn with_alpha_quality(self, _quality: f32) -> Self {
            self // JXL handles alpha uniformly
        }

        fn with_limit_pixels(mut self, max: u64) -> Self {
            self.limit_pixels = Some(max);
            self
        }

        fn with_limit_memory(mut self, bytes: u64) -> Self {
            self.limit_memory = Some(bytes);
            self
        }

        fn with_limit_output(mut self, bytes: u64) -> Self {
            self.limit_output = Some(bytes);
            self
        }

        fn job(&self) -> JxlEncodeJob<'_> {
            JxlEncodeJob {
                config: self,
                stop: None,
                icc: None,
                exif: None,
                xmp: None,
                limit_pixels: None,
                limit_memory: None,
            }
        }
    }

    /// Per-operation JXL encode job.
    pub struct JxlEncodeJob<'a> {
        config: &'a JxlEncoding,
        stop: Option<&'a dyn Stop>,
        icc: Option<&'a [u8]>,
        exif: Option<&'a [u8]>,
        xmp: Option<&'a [u8]>,
        limit_pixels: Option<u64>,
        limit_memory: Option<u64>,
    }

    impl<'a> JxlEncodeJob<'a> {
        fn do_encode(
            self,
            pixels: &[u8],
            layout: PixelLayout,
            w: u32,
            h: u32,
        ) -> Result<ZEncodeOutput, JxlError> {
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

            let limits;
            let has_limits = self.limit_pixels.is_some()
                || self.limit_memory.is_some()
                || self.config.limit_pixels.is_some()
                || self.config.limit_memory.is_some();
            if has_limits {
                let mut l = jxl_encoder::Limits::new();
                if let Some(p) = self.limit_pixels.or(self.config.limit_pixels) {
                    l = l.with_max_pixels(p);
                }
                if let Some(m) = self.limit_memory.or(self.config.limit_memory) {
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

            Ok(ZEncodeOutput::new(data, ZImageFormat::Jxl))
        }
    }

    impl<'a> EncodingJob<'a> for JxlEncodeJob<'a> {
        type Error = JxlError;

        fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
            self.stop = Some(stop);
            self
        }

        fn with_metadata(mut self, meta: &'a ZImageMetadata<'a>) -> Self {
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

        fn with_icc(mut self, icc: &'a [u8]) -> Self {
            self.icc = Some(icc);
            self
        }

        fn with_exif(mut self, exif: &'a [u8]) -> Self {
            self.exif = Some(exif);
            self
        }

        fn with_xmp(mut self, xmp: &'a [u8]) -> Self {
            self.xmp = Some(xmp);
            self
        }

        fn with_limit_pixels(mut self, max: u64) -> Self {
            self.limit_pixels = Some(max);
            self
        }

        fn with_limit_memory(mut self, bytes: u64) -> Self {
            self.limit_memory = Some(bytes);
            self
        }

        fn encode_rgb8(self, img: ImgRef<'_, Rgb<u8>>) -> Result<ZEncodeOutput, Self::Error> {
            let (buf, w, h) = img.to_contiguous_buf();
            let bytes = crate::encode::rgb_to_bytes(&buf);
            self.do_encode(&bytes, PixelLayout::Rgb8, w as u32, h as u32)
        }

        fn encode_rgba8(self, img: ImgRef<'_, Rgba<u8>>) -> Result<ZEncodeOutput, Self::Error> {
            let (buf, w, h) = img.to_contiguous_buf();
            let bytes = crate::encode::rgba_to_bytes(&buf);
            self.do_encode(&bytes, PixelLayout::Rgba8, w as u32, h as u32)
        }

        fn encode_gray8(self, img: ImgRef<'_, Gray<u8>>) -> Result<ZEncodeOutput, Self::Error> {
            let (buf, w, h) = img.to_contiguous_buf();
            match &self.config.config {
                JxlConfig::Lossless(_) => {
                    let bytes: Vec<u8> = buf.iter().map(|g| g.value()).collect();
                    self.do_encode(&bytes, PixelLayout::Gray8, w as u32, h as u32)
                }
                JxlConfig::Lossy(_) => {
                    let bytes = crate::encode::gray_to_rgb_bytes(&buf);
                    self.do_encode(&bytes, PixelLayout::Rgb8, w as u32, h as u32)
                }
            }
        }
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
pub use encoding::{JxlEncodeJob, JxlEncoding};

// ── Decoding ────────────────────────────────────────────────────────────────

#[cfg(feature = "decode")]
mod decoding {
    use super::*;
    use zencodec_types::{Decoding, DecodingJob};

    /// JPEG XL decoder configuration implementing [`Decoding`].
    #[derive(Clone, Debug)]
    pub struct JxlDecoding {
        limit_pixels: Option<u64>,
        limit_memory: Option<u64>,
        limit_file_size: Option<u64>,
    }

    impl JxlDecoding {
        /// Create a default JXL decoder config.
        #[must_use]
        pub fn new() -> Self {
            Self {
                limit_pixels: None,
                limit_memory: None,
                limit_file_size: None,
            }
        }
    }

    impl Default for JxlDecoding {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Decoding for JxlDecoding {
        type Error = JxlError;
        type Job<'a> = JxlDecodeJob<'a>;

        fn with_limit_pixels(mut self, max: u64) -> Self {
            self.limit_pixels = Some(max);
            self
        }

        fn with_limit_memory(mut self, bytes: u64) -> Self {
            self.limit_memory = Some(bytes);
            self
        }

        fn with_limit_dimensions(mut self, width: u32, height: u32) -> Self {
            self.limit_pixels = Some(width as u64 * height as u64);
            self
        }

        fn with_limit_file_size(mut self, bytes: u64) -> Self {
            self.limit_file_size = Some(bytes);
            self
        }

        fn job(&self) -> JxlDecodeJob<'_> {
            JxlDecodeJob {
                config: self,
                limit_pixels: None,
                limit_memory: None,
            }
        }

        fn probe(&self, data: &[u8]) -> Result<ZImageInfo, Self::Error> {
            let info = crate::decode::probe(data)?;
            Ok(convert_info(&info))
        }
    }

    /// Per-operation JXL decode job.
    pub struct JxlDecodeJob<'a> {
        config: &'a JxlDecoding,
        limit_pixels: Option<u64>,
        limit_memory: Option<u64>,
    }

    impl<'a> DecodingJob<'a> for JxlDecodeJob<'a> {
        type Error = JxlError;

        fn with_stop(self, _stop: &'a dyn Stop) -> Self {
            self // JXL decoding is not cancellable
        }

        fn with_limit_pixels(mut self, max: u64) -> Self {
            self.limit_pixels = Some(max);
            self
        }

        fn with_limit_memory(mut self, bytes: u64) -> Self {
            self.limit_memory = Some(bytes);
            self
        }

        fn decode(self, data: &[u8]) -> Result<ZDecodeOutput, Self::Error> {
            let limits = if self.limit_pixels.is_some()
                || self.limit_memory.is_some()
                || self.config.limit_pixels.is_some()
                || self.config.limit_memory.is_some()
            {
                Some(crate::decode::JxlLimits {
                    max_pixels: self.limit_pixels.or(self.config.limit_pixels),
                    max_memory_bytes: self.limit_memory.or(self.config.limit_memory),
                })
            } else {
                None
            };

            let result = crate::decode::decode(data, limits.as_ref())?;
            let info = convert_info(&result.info);
            Ok(ZDecodeOutput::new(result.pixels, info))
        }
    }

    fn convert_info(info: &crate::decode::JxlInfo) -> ZImageInfo {
        let mut zi = ZImageInfo::new(info.width, info.height, ZImageFormat::Jxl);
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
pub use decoding::{JxlDecodeJob, JxlDecoding};

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
        use zencodec_types::Encoding;
        let enc = JxlEncoding::lossy(1.0);
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
        assert_eq!(output.format(), ZImageFormat::Jxl);
        assert_eq!(&output.bytes()[0..2], &[0xFF, 0x0A]);
    }

    #[test]
    #[cfg(feature = "encode")]
    fn encoding_lossless() {
        use zencodec_types::Encoding;
        let enc = JxlEncoding::lossless();
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
        use zencodec_types::Encoding;
        let enc = JxlEncoding::default().with_quality(100.0);
        assert!(enc.lossless_config().is_some());
    }

    #[test]
    #[cfg(all(feature = "encode", feature = "decode"))]
    fn roundtrip() {
        use zencodec_types::{Decoding, Encoding};
        let enc = JxlEncoding::lossless();
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

        let dec = JxlDecoding::new();
        let output = dec.decode(encoded.bytes()).unwrap();
        assert_eq!(output.info().width, 4);
        assert_eq!(output.info().height, 4);
        assert_eq!(output.info().format, ZImageFormat::Jxl);
    }
}
