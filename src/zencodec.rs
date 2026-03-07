//! zencodec-types trait implementations for JPEG XL.
//!
//! Thin adapter layer over the native `zenjxl` encode/decode API.
//!
//! # Trait mapping
//!
//! | zencodec-types | zenjxl adapter |
//! |----------------|----------------|
//! | `EncoderConfig` | [`JxlEncoderConfig`] |
//! | `EncodeJob<'a>` | [`JxlEncodeJob`] |
//! | `Encoder` | [`JxlEncoder`] |
//! | `FrameEncoder` | [`JxlFrameEncoder`] |
//! | `DecoderConfig` | [`JxlDecoderConfig`] |
//! | `DecodeJob<'a>` | [`JxlDecodeJob`] |
//! | `Decode` | [`JxlDecoder`] |
//! | `FrameDecode` | [`JxlFrameDecoder`] |

use alloc::sync::Arc;
use zc::ImageFormat;
use zenpixels::PixelDescriptor;

use crate::error::JxlError;

type At<E> = whereat::At<E>;

// ── Encoding ────────────────────────────────────────────────────────────────

#[cfg(feature = "encode")]
mod encoding {
    use super::*;
    use alloc::vec::Vec;
    use jxl_encoder::{AnimationFrame, AnimationParams, LosslessConfig, LossyConfig, PixelLayout};
    use zc::encode::{EncodeCapabilities, EncodeOutput};
    use zc::{MetadataView, ResourceLimits, UnsupportedOperation};
    use zenpixels::{ChannelLayout, ChannelType, PixelSlice};

    use enough::Stop;

    /// Helper to wrap a JxlError with location tracking.
    fn at(e: JxlError) -> At<JxlError> {
        whereat::at(e)
    }

    // ── Capabilities ────────────────────────────────────────────────────

    static JXL_ENCODE_CAPS: EncodeCapabilities = EncodeCapabilities::new()
        .with_lossy(true)
        .with_lossless(true)
        .with_hdr(true)
        .with_native_gray(true)
        .with_native_alpha(true)
        .with_native_f32(true)
        .with_animation(true)
        .with_effort_range(1, 10)
        .with_quality_range(0.0, 100.0);

    /// Supported pixel descriptors for encoding.
    ///
    /// jxl-encoder supports RGB8, RGBA8, BGRA8, and Gray8 via PixelLayout.
    static JXL_ENCODE_DESCRIPTORS: &[PixelDescriptor] = &[
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::GRAY8,
    ];

    // ── Internal encoder config ─────────────────────────────────────────

    /// Internal: lossy or lossless JXL config.
    #[derive(Clone, Debug)]
    enum JxlEncMode {
        Lossy(LossyConfig),
        Lossless(LosslessConfig),
    }

    // ── JxlEncoderConfig ────────────────────────────────────────────────

    /// JPEG XL encoder configuration.
    ///
    /// Implements [`zc::EncoderConfig`].
    #[derive(Clone, Debug)]
    pub struct JxlEncoderConfig {
        mode: JxlEncMode,
        quality: Option<f32>,
        effort: Option<i32>,
        lossless: bool,
    }

    impl JxlEncoderConfig {
        /// Create a default lossy encoder config.
        pub fn new() -> Self {
            Self {
                mode: JxlEncMode::Lossy(LossyConfig::new(1.0)),
                quality: None,
                effort: None,
                lossless: false,
            }
        }

        /// Access the underlying lossy config for codec-specific tuning.
        pub fn lossy_config(&self) -> Option<&LossyConfig> {
            match &self.mode {
                JxlEncMode::Lossy(c) => Some(c),
                JxlEncMode::Lossless(_) => None,
            }
        }

        /// Access the underlying lossless config for codec-specific tuning.
        pub fn lossless_config(&self) -> Option<&LosslessConfig> {
            match &self.mode {
                JxlEncMode::Lossless(c) => Some(c),
                JxlEncMode::Lossy(_) => None,
            }
        }
    }

    impl Default for JxlEncoderConfig {
        fn default() -> Self {
            Self::new()
        }
    }

    impl zc::encode::EncoderConfig for JxlEncoderConfig {
        type Error = At<JxlError>;
        type Job<'a> = JxlEncodeJob<'a>;

        fn format() -> ImageFormat {
            ImageFormat::Jxl
        }

        fn supported_descriptors() -> &'static [PixelDescriptor] {
            JXL_ENCODE_DESCRIPTORS
        }

        fn capabilities() -> &'static EncodeCapabilities {
            &JXL_ENCODE_CAPS
        }

        fn with_generic_quality(mut self, quality: f32) -> Self {
            self.quality = Some(quality);
            if !self.lossless {
                // Map 0-100 quality to JXL distance.
                // quality 100 → distance 0.0 (lossless-quality lossy)
                // quality 0 → distance 25.0 (very lossy)
                let distance = (100.0 - quality.clamp(0.0, 100.0)) * 0.25;
                let effort = self.effort.unwrap_or(7) as u8;
                self.mode = JxlEncMode::Lossy(LossyConfig::new(distance).with_effort(effort));
            }
            self
        }

        fn with_generic_effort(mut self, effort: i32) -> Self {
            let effort = effort.clamp(1, 10);
            self.effort = Some(effort);
            self.mode = match self.mode {
                JxlEncMode::Lossy(cfg) => JxlEncMode::Lossy(cfg.with_effort(effort as u8)),
                JxlEncMode::Lossless(cfg) => JxlEncMode::Lossless(cfg.with_effort(effort as u8)),
            };
            self
        }

        fn with_lossless(mut self, lossless: bool) -> Self {
            self.lossless = lossless;
            if lossless {
                let mut cfg = LosslessConfig::default();
                if let Some(e) = self.effort {
                    cfg = cfg.with_effort(e.clamp(1, 10) as u8);
                }
                self.mode = JxlEncMode::Lossless(cfg);
            } else {
                let distance = self
                    .quality
                    .map(|q| (100.0 - q.clamp(0.0, 100.0)) * 0.25)
                    .unwrap_or(1.0);
                let mut cfg = LossyConfig::new(distance);
                if let Some(e) = self.effort {
                    cfg = cfg.with_effort(e.clamp(1, 10) as u8);
                }
                self.mode = JxlEncMode::Lossy(cfg);
            }
            self
        }

        fn generic_quality(&self) -> Option<f32> {
            self.quality
        }

        fn generic_effort(&self) -> Option<i32> {
            self.effort
        }

        fn is_lossless(&self) -> Option<bool> {
            Some(self.lossless)
        }

        fn job(&self) -> JxlEncodeJob<'_> {
            JxlEncodeJob {
                config: self,
                _stop: None,
                limits: None,
                metadata: None,
                loop_count: None,
            }
        }
    }

    // ── JxlEncodeJob ────────────────────────────────────────────────────

    /// Per-operation encode job for JPEG XL.
    pub struct JxlEncodeJob<'a> {
        config: &'a JxlEncoderConfig,
        _stop: Option<&'a dyn Stop>,
        limits: Option<ResourceLimits>,
        metadata: Option<&'a MetadataView<'a>>,
        loop_count: Option<u32>,
    }

    impl<'a> zc::encode::EncodeJob<'a> for JxlEncodeJob<'a> {
        type Error = At<JxlError>;
        type Enc = JxlEncoder<'a>;
        type FrameEnc = JxlFrameEncoder;

        fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
            self._stop = Some(stop);
            self
        }

        fn with_limits(mut self, limits: ResourceLimits) -> Self {
            self.limits = Some(limits);
            self
        }

        fn with_metadata(mut self, meta: &'a MetadataView<'a>) -> Self {
            self.metadata = Some(meta);
            self
        }

        fn with_loop_count(mut self, count: Option<u32>) -> Self {
            self.loop_count = count;
            self
        }

        fn encoder(self) -> Result<JxlEncoder<'a>, At<JxlError>> {
            Ok(JxlEncoder {
                mode: self.config.mode.clone(),
                _metadata: self.metadata,
            })
        }

        fn frame_encoder(self) -> Result<JxlFrameEncoder, At<JxlError>> {
            Ok(JxlFrameEncoder {
                mode: self.config.mode.clone(),
                loop_count: self.loop_count,
                frames: Vec::new(),
                pixel_data: Vec::new(),
                width: 0,
                height: 0,
                layout: None,
            })
        }
    }

    // ── JxlEncoder ──────────────────────────────────────────────────────

    /// Single-image JPEG XL encoder.
    pub struct JxlEncoder<'a> {
        mode: JxlEncMode,
        _metadata: Option<&'a MetadataView<'a>>,
    }

    impl JxlEncoder<'_> {
        /// Map a PixelDescriptor to the jxl-encoder PixelLayout.
        fn descriptor_to_layout(desc: PixelDescriptor) -> Result<PixelLayout, At<JxlError>> {
            let layout = desc.layout();
            let ct = desc.channel_type();
            match (layout, ct) {
                (ChannelLayout::Rgb, ChannelType::U8) => Ok(PixelLayout::Rgb8),
                (ChannelLayout::Rgba, ChannelType::U8) => Ok(PixelLayout::Rgba8),
                (ChannelLayout::Bgra, ChannelType::U8) => Ok(PixelLayout::Bgra8),
                (ChannelLayout::Gray, ChannelType::U8) => Ok(PixelLayout::Gray8),
                _ => Err(at(JxlError::UnsupportedOperation(
                    UnsupportedOperation::PixelFormat,
                ))),
            }
        }
    }

    impl zc::encode::Encoder for JxlEncoder<'_> {
        type Error = At<JxlError>;

        fn reject(op: UnsupportedOperation) -> At<JxlError> {
            at(JxlError::UnsupportedOperation(op))
        }

        fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<JxlError>> {
            let layout = Self::descriptor_to_layout(pixels.descriptor())?;
            let width = pixels.width();
            let height = pixels.rows();
            let data = pixels.contiguous_bytes();

            let encoded = match &self.mode {
                JxlEncMode::Lossy(cfg) => cfg
                    .encode(&data, width, height, layout)
                    .map_err(|e| at(JxlError::Encode(e.into_inner())))?,
                JxlEncMode::Lossless(cfg) => cfg
                    .encode(&data, width, height, layout)
                    .map_err(|e| at(JxlError::Encode(e.into_inner())))?,
            };

            Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                .with_mime_type("image/jxl")
                .with_extension("jxl"))
        }
    }

    // ── JxlFrameEncoder ─────────────────────────────────────────────────

    /// Animation JPEG XL encoder.
    ///
    /// Collects frames, then encodes them all at once via
    /// `jxl-encoder`'s `encode_animation`.
    pub struct JxlFrameEncoder {
        mode: JxlEncMode,
        loop_count: Option<u32>,
        /// Duration per frame in milliseconds.
        frames: Vec<u32>,
        /// Raw pixel data for each frame (owned copies).
        pixel_data: Vec<Vec<u8>>,
        width: u32,
        height: u32,
        layout: Option<PixelLayout>,
    }

    impl zc::encode::FrameEncoder for JxlFrameEncoder {
        type Error = At<JxlError>;

        fn reject(op: UnsupportedOperation) -> At<JxlError> {
            at(JxlError::UnsupportedOperation(op))
        }

        fn push_frame(
            &mut self,
            pixels: PixelSlice<'_>,
            duration_ms: u32,
        ) -> Result<(), At<JxlError>> {
            let layout = JxlEncoder::descriptor_to_layout(pixels.descriptor())?;
            let w = pixels.width();
            let h = pixels.rows();

            if self.pixel_data.is_empty() {
                self.width = w;
                self.height = h;
                self.layout = Some(layout);
            } else if w != self.width || h != self.height {
                return Err(at(JxlError::InvalidInput(
                    "animation frame dimensions must match first frame".into(),
                )));
            }

            self.pixel_data.push(pixels.contiguous_bytes().into_owned());
            // Convert ms to ticks at 1000 tps (1ms precision)
            self.frames.push(duration_ms);
            Ok(())
        }

        fn finish(self) -> Result<EncodeOutput, At<JxlError>> {
            let layout = self
                .layout
                .ok_or_else(|| at(JxlError::InvalidInput("no frames pushed".into())))?;

            let animation = AnimationParams {
                tps_numerator: 1000,
                tps_denominator: 1,
                num_loops: self.loop_count.unwrap_or(0),
            };

            let anim_frames: Vec<AnimationFrame<'_>> = self
                .pixel_data
                .iter()
                .zip(&self.frames)
                .map(|(data, &duration)| AnimationFrame {
                    pixels: data,
                    duration,
                })
                .collect();

            let encoded = match &self.mode {
                JxlEncMode::Lossy(cfg) => cfg
                    .encode_animation(self.width, self.height, layout, &animation, &anim_frames)
                    .map_err(|e| at(JxlError::Encode(e.into_inner())))?,
                JxlEncMode::Lossless(cfg) => cfg
                    .encode_animation(self.width, self.height, layout, &animation, &anim_frames)
                    .map_err(|e| at(JxlError::Encode(e.into_inner())))?,
            };

            Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                .with_mime_type("image/jxl")
                .with_extension("jxl"))
        }
    }
}

// ── Decoding ────────────────────────────────────────────────────────────────

#[cfg(feature = "decode")]
mod decoding {
    use super::*;
    use alloc::collections::VecDeque;
    use alloc::vec;
    use alloc::vec::Vec;

    use jxl::api::{
        ExtraChannel, JxlDecoder as JxlRsDecoder, JxlDecoderOptions, JxlOutputBuffer,
        ProcessingResult,
    };
    use zc::Unsupported;
    use zc::decode::{DecodeCapabilities, DecodeFrame, DecodeOutput, OutputInfo};
    use zc::{ImageInfo, ResourceLimits, UnsupportedOperation};
    use zenpixels::Cicp;

    use enough::Stop;

    use crate::decode::{
        JxlInfo, JxlLimits, build_pixel_data, choose_pixel_format, decode, extract_color_info,
        is_hdr_or_wide_gamut, probe,
    };

    /// Helper to wrap a JxlError with location tracking.
    fn at(e: JxlError) -> At<JxlError> {
        whereat::at(e)
    }

    // ── Capabilities ────────────────────────────────────────────────────

    static JXL_DECODE_CAPS: DecodeCapabilities = DecodeCapabilities::new()
        .with_icc(true)
        .with_cicp(true)
        .with_hdr(true)
        .with_native_gray(true)
        .with_native_16bit(true)
        .with_native_f32(true)
        .with_native_alpha(true)
        .with_animation(true)
        .with_cheap_probe(true)
        .with_enforces_max_pixels(true);

    /// Supported pixel descriptors for decoding.
    ///
    /// jxl-rs can decode to any combination of U8/U16/F32 × Gray/RGB/RGBA/BGRA.
    static JXL_DECODE_DESCRIPTORS: &[PixelDescriptor] = &[
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::GRAY8,
        PixelDescriptor::RGB16,
        PixelDescriptor::RGBA16,
        PixelDescriptor::GRAY16,
        PixelDescriptor::RGBF32,
        PixelDescriptor::RGBAF32,
        PixelDescriptor::GRAYF32,
    ];

    // ── JxlDecoderConfig ────────────────────────────────────────────────

    /// JPEG XL decoder configuration.
    ///
    /// Implements [`zc::DecoderConfig`].
    #[derive(Clone, Debug, Default)]
    pub struct JxlDecoderConfig {
        _priv: (),
    }

    impl JxlDecoderConfig {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl zc::decode::DecoderConfig for JxlDecoderConfig {
        type Error = At<JxlError>;
        type Job<'a> = JxlDecodeJob<'a>;

        fn format() -> ImageFormat {
            ImageFormat::Jxl
        }

        fn supported_descriptors() -> &'static [PixelDescriptor] {
            JXL_DECODE_DESCRIPTORS
        }

        fn capabilities() -> &'static DecodeCapabilities {
            &JXL_DECODE_CAPS
        }

        fn job(&self) -> JxlDecodeJob<'_> {
            JxlDecodeJob {
                limits: None,
                _stop: None,
            }
        }
    }

    // ── JxlDecodeJob ────────────────────────────────────────────────────

    /// Per-operation decode job for JPEG XL.
    pub struct JxlDecodeJob<'a> {
        limits: Option<ResourceLimits>,
        _stop: Option<&'a dyn Stop>,
    }

    impl JxlDecodeJob<'_> {
        /// Convert native JxlInfo into zencodec-types ImageInfo.
        fn jxl_info_to_image_info(info: &JxlInfo) -> ImageInfo {
            let mut image_info = ImageInfo::new(info.width, info.height, ImageFormat::Jxl)
                .with_alpha(info.has_alpha)
                .with_animation(info.has_animation);

            image_info =
                image_info.with_orientation(zc::Orientation::from_exif(info.orientation as u16));

            if let Some((cp, tc, mc, fr)) = info.cicp {
                image_info = image_info.with_cicp(Cicp::new(cp, tc, mc, fr));
            }

            if let Some(ref icc) = info.icc_profile {
                image_info = image_info.with_icc_profile(icc.clone());
            }

            image_info
        }

        /// Convert ResourceLimits to native JxlLimits.
        fn to_native_limits(limits: &Option<ResourceLimits>) -> Option<JxlLimits> {
            limits.as_ref().map(|l| JxlLimits {
                max_pixels: l.max_pixels,
                max_memory_bytes: l.max_memory_bytes,
            })
        }
    }

    impl<'a> zc::decode::DecodeJob<'a> for JxlDecodeJob<'a> {
        type Error = At<JxlError>;
        type Dec = JxlDecoder<'a>;
        type StreamDec = Unsupported<At<JxlError>>;
        type FrameDec = JxlFrameDecoder<'a>;

        fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
            self._stop = Some(stop);
            self
        }

        fn with_limits(mut self, limits: ResourceLimits) -> Self {
            self.limits = Some(limits);
            self
        }

        fn probe(&self, data: &[u8]) -> Result<ImageInfo, At<JxlError>> {
            let info = probe(data).map_err(at)?;
            Ok(Self::jxl_info_to_image_info(&info))
        }

        fn output_info(&self, data: &[u8]) -> Result<OutputInfo, At<JxlError>> {
            let info = probe(data).map_err(at)?;
            // Determine the native output descriptor based on what choose_pixel_format
            // would select (without actually decoding).
            let bit_depth_u8 = info.bit_depth.unwrap_or(8);
            let is_float = bit_depth_u8 == 32;
            let native_desc = match (info.has_alpha, is_float, bit_depth_u8 > 8) {
                (false, true, _) => PixelDescriptor::RGBF32,
                (true, true, _) => PixelDescriptor::RGBAF32,
                (false, _, true) => PixelDescriptor::RGB16,
                (true, _, true) => PixelDescriptor::RGBA16,
                (false, _, _) => PixelDescriptor::RGB8_SRGB,
                (true, _, _) => PixelDescriptor::RGBA8_SRGB,
            };
            Ok(
                OutputInfo::full_decode(info.width, info.height, native_desc)
                    .with_alpha(info.has_alpha),
            )
        }

        fn decoder(
            self,
            data: &'a [u8],
            preferred: &[PixelDescriptor],
        ) -> Result<JxlDecoder<'a>, At<JxlError>> {
            Ok(JxlDecoder {
                data,
                limits: self.limits,
                preferred: preferred.to_vec(),
            })
        }

        fn streaming_decoder(
            self,
            _data: &'a [u8],
            _preferred: &[PixelDescriptor],
        ) -> Result<Unsupported<At<JxlError>>, At<JxlError>> {
            Err(at(JxlError::UnsupportedOperation(
                UnsupportedOperation::RowLevelDecode,
            )))
        }

        fn frame_decoder(
            self,
            data: &'a [u8],
            preferred: &[PixelDescriptor],
        ) -> Result<JxlFrameDecoder<'a>, At<JxlError>> {
            Ok(JxlFrameDecoder {
                data,
                limits: self.limits,
                preferred: preferred.to_vec(),
                frames: None,
            })
        }
    }

    // ── JxlDecoder ──────────────────────────────────────────────────────

    /// Single-image JPEG XL decoder.
    pub struct JxlDecoder<'a> {
        data: &'a [u8],
        limits: Option<ResourceLimits>,
        preferred: Vec<PixelDescriptor>,
    }

    impl zc::decode::Decode for JxlDecoder<'_> {
        type Error = At<JxlError>;

        fn decode(self) -> Result<DecodeOutput, At<JxlError>> {
            let native_limits = JxlDecodeJob::to_native_limits(&self.limits);
            let result = decode(self.data, native_limits.as_ref(), &self.preferred).map_err(at)?;

            let info = JxlDecodeJob::jxl_info_to_image_info(&result.info);
            Ok(DecodeOutput::new(result.pixels, info))
        }
    }

    // ── JxlFrameDecoder ─────────────────────────────────────────────────

    /// Animation JPEG XL decoder (full composited frames).
    ///
    /// Decodes all frames eagerly on first call to `next_frame()` — the
    /// jxl-rs decoder handles blending/disposal internally, producing
    /// fully composited frames.
    pub struct JxlFrameDecoder<'a> {
        data: &'a [u8],
        limits: Option<ResourceLimits>,
        preferred: Vec<PixelDescriptor>,
        /// Pre-decoded frames (lazily populated on first next_frame call).
        frames: Option<DecodedFrames>,
    }

    struct DecodedFrames {
        frames: VecDeque<DecodeFrame>,
        loop_count: Option<u32>,
    }

    impl JxlFrameDecoder<'_> {
        /// Decode all frames up front.
        fn decode_all_frames(&mut self) -> Result<(), At<JxlError>> {
            let mut options = JxlDecoderOptions::default();

            if let Some(ref lim) = self.limits
                && let Some(max_px) = lim.max_pixels
            {
                options.limits.max_pixels = Some(max_px as usize);
            }

            let decoder = JxlRsDecoder::new(options);

            // Parse header
            let mut input: &[u8] = self.data;
            let mut decoder = match decoder
                .process(&mut input)
                .map_err(|e| at(JxlError::Decode(e)))?
            {
                ProcessingResult::Complete { result } => result,
                ProcessingResult::NeedsMoreInput { .. } => {
                    return Err(at(JxlError::InvalidInput(
                        "JXL: insufficient data for header".into(),
                    )));
                }
            };

            let basic_info = decoder.basic_info();
            let (width, height) = basic_info.size;
            let has_alpha = basic_info
                .extra_channels
                .iter()
                .any(|ec| matches!(ec.ec_type, ExtraChannel::Alpha));
            let has_animation = basic_info.animation.is_some();
            let loop_count = basic_info.animation.as_ref().map(|a| a.num_loops);
            let bit_depth_u8 = basic_info.bit_depth.bits_per_sample() as u8;
            let orientation = basic_info.orientation as u8;
            let is_gray = crate::decode::profile_is_grayscale(decoder.embedded_color_profile());
            let num_extra = basic_info.extra_channels.len();

            let (icc_profile, cicp) = extract_color_info(decoder.embedded_color_profile());

            let chosen = choose_pixel_format(
                &basic_info.bit_depth,
                has_alpha,
                is_gray,
                num_extra,
                &self.preferred,
            );

            let channels = chosen.color_type.samples_per_pixel();
            let bytes_per_sample = match chosen.channel_type {
                zenpixels::ChannelType::U8 => 1,
                zenpixels::ChannelType::U16 => 2,
                zenpixels::ChannelType::F32 => 4,
                _ => 1,
            };

            decoder.set_pixel_format(chosen.pixel_format.clone());

            let bytes_per_row = width * channels * bytes_per_sample;

            let jxl_info = JxlInfo {
                width: width as u32,
                height: height as u32,
                has_alpha,
                has_animation,
                bit_depth: Some(bit_depth_u8),
                icc_profile,
                orientation,
                cicp,
            };
            let image_info = Arc::new(JxlDecodeJob::jxl_info_to_image_info(&jxl_info));

            let is_f32 = matches!(
                chosen.pixel_format.color_data_format,
                Some(jxl::api::JxlDataFormat::F32 { .. })
            );
            let clamp = is_f32 && !is_hdr_or_wide_gamut(cicp);

            let mut frames = VecDeque::new();
            let mut frame_index = 0u32;

            loop {
                // Advance to frame info
                let decoder_fi = match decoder
                    .process(&mut input)
                    .map_err(|e| at(JxlError::Decode(e)))?
                {
                    ProcessingResult::Complete { result } => result,
                    ProcessingResult::NeedsMoreInput { .. } => break,
                };

                let frame_header = decoder_fi.frame_header();
                let duration_ms = frame_header.duration.map(|d| d as u32).unwrap_or(0);

                // Decode pixels
                let buf_size = bytes_per_row * height;
                let mut buf = vec![0u8; buf_size];
                let output = JxlOutputBuffer::new(&mut buf, height, bytes_per_row);

                let next_decoder = match decoder_fi
                    .process(&mut input, &mut [output])
                    .map_err(|e| at(JxlError::Decode(e)))?
                {
                    ProcessingResult::Complete { result } => result,
                    ProcessingResult::NeedsMoreInput { .. } => {
                        return Err(at(JxlError::InvalidInput(
                            "JXL: insufficient data for frame pixels".into(),
                        )));
                    }
                };

                if clamp {
                    crate::decode::clamp_f32_buf(&mut buf);
                }

                let pixel_buf = build_pixel_data(&buf, width, height, &chosen);

                frames.push_back(DecodeFrame::new(
                    pixel_buf,
                    image_info.clone(),
                    duration_ms,
                    frame_index,
                ));

                frame_index += 1;

                if !next_decoder.has_more_frames() {
                    break;
                }
                decoder = next_decoder;
            }

            self.frames = Some(DecodedFrames { frames, loop_count });

            Ok(())
        }
    }

    impl zc::decode::FrameDecode for JxlFrameDecoder<'_> {
        type Error = At<JxlError>;

        fn frame_count(&self) -> Option<u32> {
            self.frames.as_ref().map(|f| f.frames.len() as u32)
        }

        fn loop_count(&self) -> Option<u32> {
            self.frames.as_ref().and_then(|f| f.loop_count)
        }

        fn next_frame(&mut self) -> Result<Option<DecodeFrame>, At<JxlError>> {
            if self.frames.is_none() {
                self.decode_all_frames()?;
            }

            let decoded = self.frames.as_mut().unwrap();
            Ok(decoded.frames.pop_front())
        }
    }
}

// ── Re-exports ──────────────────────────────────────────────────────────────

#[cfg(feature = "encode")]
pub use encoding::{JxlEncodeJob, JxlEncoder, JxlEncoderConfig, JxlFrameEncoder};

#[cfg(feature = "decode")]
pub use decoding::{JxlDecodeJob, JxlDecoder, JxlDecoderConfig, JxlFrameDecoder};

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_config_defaults() {
        use zc::encode::EncoderConfig;
        let config = JxlEncoderConfig::new();
        assert_eq!(JxlEncoderConfig::format(), ImageFormat::Jxl);
        assert!(!JxlEncoderConfig::supported_descriptors().is_empty());
        assert!(config.generic_quality().is_none());
        assert!(config.generic_effort().is_none());
        assert_eq!(config.is_lossless(), Some(false));
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_config_quality_effort() {
        use zc::encode::EncoderConfig;
        let config = JxlEncoderConfig::new()
            .with_generic_quality(85.0)
            .with_generic_effort(7);
        assert_eq!(config.generic_quality(), Some(85.0));
        assert_eq!(config.generic_effort(), Some(7));
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_config_lossless() {
        use zc::encode::EncoderConfig;
        let config = JxlEncoderConfig::new().with_lossless(true);
        assert_eq!(config.is_lossless(), Some(true));
        assert!(config.lossless_config().is_some());
        assert!(config.lossy_config().is_none());
    }

    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn roundtrip_rgb8() {
        use zc::decode::Decode;
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 64u32;
        let height = 64u32;
        let pixels: Vec<rgb::Rgb<u8>> = (0..width * height)
            .map(|i| {
                let v = (i % 256) as u8;
                rgb::Rgb { r: v, g: v, b: v }
            })
            .collect();
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        assert!(!output.data().is_empty());
        assert_eq!(output.format(), ImageFormat::Jxl);

        // Decode back
        use zc::decode::{DecodeJob, DecoderConfig};
        let dec_config = JxlDecoderConfig::new();
        let decoder = dec_config.job().decoder(output.data(), &[]).unwrap();
        let decoded = decoder.decode().unwrap();
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
    }

    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn roundtrip_rgba8() {
        use zc::decode::Decode;
        use zc::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 32u32;
        let height = 32u32;
        let pixels: Vec<rgb::Rgba<u8>> = (0..width * height)
            .map(|i| {
                let v = (i % 256) as u8;
                rgb::Rgba {
                    r: v,
                    g: v,
                    b: v,
                    a: 255,
                }
            })
            .collect();
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgba<u8>>::from_pixels(pixels, width, height).unwrap();

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        assert!(!output.data().is_empty());

        use zc::decode::{DecodeJob, DecoderConfig};
        let dec_config = JxlDecoderConfig::new();
        let decoder = dec_config.job().decoder(output.data(), &[]).unwrap();
        let decoded = decoder.decode().unwrap();
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
    }

    #[cfg(feature = "decode")]
    #[test]
    fn probe_returns_info() {
        use zc::decode::{DecodeJob, DecoderConfig};

        // Encode a minimal image to probe
        #[cfg(feature = "encode")]
        {
            use zc::encode::{EncodeJob, Encoder, EncoderConfig};

            let pixels: Vec<rgb::Rgb<u8>> = vec![rgb::Rgb { r: 0, g: 0, b: 0 }; 4];
            let buf = zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, 2, 2).unwrap();

            let config = JxlEncoderConfig::new().with_lossless(true);
            let encoder = config.job().encoder().unwrap();
            let output = encoder.encode(buf.as_slice().into()).unwrap();

            let dec_config = JxlDecoderConfig::new();
            let job = dec_config.job();
            let info = job.probe(output.data()).unwrap();
            assert_eq!(info.width, 2);
            assert_eq!(info.height, 2);
            assert_eq!(info.format, ImageFormat::Jxl);
        }
    }

    #[cfg(feature = "decode")]
    #[test]
    fn streaming_decoder_unsupported() {
        use zc::decode::{DecodeJob, DecoderConfig};
        use zc::{HasUnsupportedOperation, UnsupportedOperation};

        let dec_config = JxlDecoderConfig::new();
        let job = dec_config.job();
        let result = job.streaming_decoder(&[0xFF], &[]);
        match result {
            Err(err) => {
                assert_eq!(
                    err.error().unsupported_operation(),
                    Some(UnsupportedOperation::RowLevelDecode)
                );
            }
            Ok(_) => panic!("expected error"),
        }
    }

    #[cfg(feature = "encode")]
    #[test]
    fn capabilities_correct() {
        use zc::encode::EncoderConfig;
        let caps = JxlEncoderConfig::capabilities();
        assert!(caps.lossy());
        assert!(caps.lossless());
        assert!(caps.hdr());
        assert!(caps.native_gray());
        assert!(caps.native_alpha());
        assert!(caps.animation());
    }

    #[cfg(feature = "decode")]
    #[test]
    fn decode_capabilities_correct() {
        use zc::decode::DecoderConfig;
        let caps = JxlDecoderConfig::capabilities();
        assert!(caps.icc());
        assert!(caps.cicp());
        assert!(caps.hdr());
        assert!(caps.native_gray());
        assert!(caps.native_16bit());
        assert!(caps.native_f32());
        assert!(caps.native_alpha());
        assert!(caps.animation());
        assert!(caps.cheap_probe());
    }
}
