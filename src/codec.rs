//! zencodec trait implementations for JPEG XL.
//!
//! Thin adapter layer over the native `zenjxl` encode/decode API.
//!
//! # Trait mapping
//!
//! | zencodec | zenjxl adapter |
//! |----------------|----------------|
//! | `EncoderConfig` | [`JxlEncoderConfig`] |
//! | `EncodeJob<'a>` | [`JxlEncodeJob`] |
//! | `Encoder` | [`JxlEncoder`] |
//! | `FullFrameEncoder` | [`JxlFullFrameEncoder`] |
//! | `DecoderConfig` | [`JxlDecoderConfig`] |
//! | `DecodeJob<'a>` | [`JxlDecodeJob`] |
//! | `Decode` | [`JxlDecoder`] |
//! | `FullFrameDecoder` | [`JxlFullFrameDecoder`] |

use alloc::sync::Arc;
use zencodec::ImageFormat;
use zenpixels::PixelDescriptor;

use crate::error::JxlError;

type At<E> = whereat::At<E>;

/// Convert quality on 0–100 scale to JXL butteraugli distance.
///
/// Matches the jxl-encoder's own `percent_to_distance` piecewise mapping:
/// - 90–100 → distance 0.0–1.0  (perceptually lossless zone)
/// - 70–90  → distance 1.0–2.0  (high quality)
/// - 0–70   → distance 2.0–9.0  (lower quality)
fn quality_to_distance(quality: f32) -> f32 {
    let q = quality.clamp(0.0, 100.0);
    if q >= 100.0 {
        0.0
    } else if q >= 90.0 {
        (100.0 - q) / 10.0
    } else if q >= 70.0 {
        1.0 + (90.0 - q) / 20.0
    } else {
        2.0 + (70.0 - q) / 10.0
    }
}

/// Map generic quality (libjpeg-turbo scale) to JXL native quality.
///
/// Calibrated on CID22-512 corpus (209 images) to produce the same median
/// SSIMULACRA2 as libjpeg-turbo at each quality level. The native quality
/// is then mapped to Butteraugli distance by [`quality_to_distance`].
fn calibrated_jxl_quality(generic_q: f32) -> f32 {
    let clamped = generic_q.clamp(0.0, 100.0);
    const TABLE: &[(f32, f32)] = &[
        (5.0, 5.0),
        (10.0, 5.0),
        (15.0, 5.0),
        (20.0, 5.0),
        (25.0, 9.3),
        (30.0, 22.7),
        (35.0, 33.0),
        (40.0, 38.8),
        (45.0, 43.8),
        (50.0, 48.5),
        (55.0, 51.9),
        (60.0, 55.1),
        (65.0, 58.0),
        (70.0, 61.3),
        (72.0, 63.2),
        (75.0, 65.5),
        (78.0, 67.9),
        (80.0, 69.1),
        (82.0, 71.8),
        (85.0, 76.1),
        (87.0, 79.3),
        (90.0, 84.2),
        (92.0, 86.9),
        (95.0, 91.2),
        (97.0, 92.8),
        (99.0, 93.8),
    ];
    interp_quality(TABLE, clamped)
}

/// Piecewise linear interpolation with clamping at table bounds.
fn interp_quality(table: &[(f32, f32)], x: f32) -> f32 {
    if x <= table[0].0 {
        return table[0].1;
    }
    if x >= table[table.len() - 1].0 {
        return table[table.len() - 1].1;
    }
    for i in 1..table.len() {
        if x <= table[i].0 {
            let (x0, y0) = table[i - 1];
            let (x1, y1) = table[i];
            let t = (x - x0) / (x1 - x0);
            return y0 + t * (y1 - y0);
        }
    }
    table[table.len() - 1].1
}

// ── Encoding ────────────────────────────────────────────────────────────────

#[cfg(feature = "encode")]
mod encoding {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec::Vec;
    use jxl_encoder::{AnimationFrame, AnimationParams, LosslessConfig, LossyConfig, PixelLayout};
    use zencodec::encode::{EncodeCapabilities, EncodeOutput, EncodePolicy};
    use zencodec::{Metadata, ResourceLimits, UnsupportedOperation};
    use zenpixels::{ChannelLayout, ChannelType, PixelSlice};

    use enough::Stop;

    /// Helper to wrap a JxlError with location tracking.
    fn at(e: JxlError) -> At<JxlError> {
        whereat::at(e)
    }

    /// Map a [`ThreadingPolicy`] to the jxl-encoder thread count parameter.
    ///
    /// - `0` = auto (use all available cores)
    /// - `1` = single-threaded
    /// - `N` = use N threads
    fn policy_to_threads(policy: zencodec::ThreadingPolicy) -> usize {
        match policy {
            zencodec::ThreadingPolicy::SingleThread => 1,
            zencodec::ThreadingPolicy::LimitOrSingle { max_threads } => max_threads as usize,
            zencodec::ThreadingPolicy::LimitOrAny {
                preferred_max_threads,
            } => preferred_max_threads as usize,
            zencodec::ThreadingPolicy::Balanced => {
                // no_std: can't query available_parallelism; use 0 (auto) and
                // let the encoder's rayon pool decide.
                0
            }
            zencodec::ThreadingPolicy::Unlimited => 0, // 0 = auto
            _ => 0,                              // future variants default to auto
        }
    }

    /// Apply threading policy from [`ResourceLimits`] to a [`JxlEncMode`].
    fn apply_threads(mode: &JxlEncMode, limits: &Option<ResourceLimits>) -> JxlEncMode {
        let threads = limits
            .as_ref()
            .map(|l| policy_to_threads(l.threading()))
            .unwrap_or(0);
        match mode {
            JxlEncMode::Lossy(cfg) => JxlEncMode::Lossy(cfg.clone().with_threads(threads)),
            JxlEncMode::Lossless(cfg) => JxlEncMode::Lossless(cfg.clone().with_threads(threads)),
        }
    }

    // ── Capabilities ────────────────────────────────────────────────────

    static JXL_ENCODE_CAPS: EncodeCapabilities = EncodeCapabilities::new()
        .with_lossy(true)
        .with_lossless(true)
        .with_hdr(true)
        .with_native_gray(true)
        .with_native_alpha(true)
        .with_native_16bit(true)
        .with_native_f32(true)
        .with_push_rows(true)
        .with_animation(true)
        .with_effort_range(1, 10)
        .with_quality_range(0.0, 100.0)
        .with_icc(true)
        .with_exif(true)
        .with_xmp(true)
        .with_enforces_max_pixels(true)
        .with_enforces_max_memory(true)
        .with_stop(true)
        .with_threads_supported_range(1, u16::MAX);

    /// Supported pixel descriptors for encoding.
    ///
    /// jxl-encoder supports U8/U16/F32 × RGB/RGBA/Gray/GrayAlpha + BGRA8.
    /// F32 layouts are linear-light (jxl-encoder assumes linear for float input).
    static JXL_ENCODE_DESCRIPTORS: &[PixelDescriptor] = &[
        // 8-bit sRGB
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::BGRA8_SRGB,
        PixelDescriptor::GRAY8_SRGB,
        PixelDescriptor::GRAYA8_SRGB,
        // 16-bit sRGB
        PixelDescriptor::RGB16_SRGB,
        PixelDescriptor::RGBA16_SRGB,
        PixelDescriptor::GRAY16_SRGB,
        PixelDescriptor::GRAYA16_SRGB,
        // f32 linear
        PixelDescriptor::RGBF32_LINEAR,
        PixelDescriptor::RGBAF32_LINEAR,
        PixelDescriptor::GRAYF32_LINEAR,
        PixelDescriptor::GRAYAF32_LINEAR,
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
    /// Implements [`zencodec::encode::EncoderConfig`].
    #[derive(Clone, Debug)]
    pub struct JxlEncoderConfig {
        mode: JxlEncMode,
        /// The calibrated JXL-native quality (mapped from generic quality).
        /// Used internally for distance calculation.
        calibrated_quality: Option<f32>,
        /// The original generic quality value (0-100, libjpeg-turbo scale).
        /// Returned by `generic_quality()` for roundtrip fidelity.
        generic_quality: Option<f32>,
        effort: Option<i32>,
        lossless: bool,
    }

    impl JxlEncoderConfig {
        /// Create a default lossy encoder config (distance 1.0, effort 7).
        pub fn new() -> Self {
            Self {
                mode: JxlEncMode::Lossy(LossyConfig::new(1.0)),
                calibrated_quality: None,
                generic_quality: None,
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

        /// Rebuild the lossy mode from current quality + effort state.
        fn rebuild_lossy(&mut self) {
            let distance = self
                .calibrated_quality
                .map(quality_to_distance)
                .unwrap_or(1.0);
            let mut cfg = LossyConfig::new(distance);
            if let Some(e) = self.effort {
                cfg = cfg.with_effort(e.clamp(1, 10) as u8);
            }
            self.mode = JxlEncMode::Lossy(cfg);
        }

        /// Rebuild lossless mode from current effort state.
        fn rebuild_lossless(&mut self) {
            let mut cfg = LosslessConfig::default();
            if let Some(e) = self.effort {
                cfg = cfg.with_effort(e.clamp(1, 10) as u8);
            }
            self.mode = JxlEncMode::Lossless(cfg);
        }
    }

    impl Default for JxlEncoderConfig {
        fn default() -> Self {
            Self::new()
        }
    }

    impl zencodec::encode::EncoderConfig for JxlEncoderConfig {
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
            self.generic_quality = Some(quality);
            self.calibrated_quality = Some(calibrated_jxl_quality(quality));
            if !self.lossless {
                self.rebuild_lossy();
            }
            self
        }

        fn with_generic_effort(mut self, effort: i32) -> Self {
            self.effort = Some(effort.clamp(1, 10));
            match self.lossless {
                true => self.rebuild_lossless(),
                false => self.rebuild_lossy(),
            }
            self
        }

        fn with_lossless(mut self, lossless: bool) -> Self {
            self.lossless = lossless;
            match lossless {
                true => self.rebuild_lossless(),
                false => self.rebuild_lossy(),
            }
            self
        }

        fn generic_quality(&self) -> Option<f32> {
            self.generic_quality
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
                stop: None,
                limits: None,
                metadata: None,
                policy: EncodePolicy::none(),
                loop_count: None,
            }
        }
    }

    // ── JxlEncodeJob ────────────────────────────────────────────────────

    /// Per-operation encode job for JPEG XL.
    pub struct JxlEncodeJob<'a> {
        config: &'a JxlEncoderConfig,
        stop: Option<&'a dyn Stop>,
        limits: Option<ResourceLimits>,
        metadata: Option<Metadata>,
        policy: EncodePolicy,
        loop_count: Option<u32>,
    }

    impl<'a> zencodec::encode::EncodeJob<'a> for JxlEncodeJob<'a> {
        type Error = At<JxlError>;
        type Enc = JxlEncoder<'a>;
        type FullFrameEnc = JxlFullFrameEncoder;

        fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
            self.stop = Some(stop);
            self
        }

        fn with_limits(mut self, limits: ResourceLimits) -> Self {
            self.limits = Some(limits);
            self
        }

        fn with_metadata(mut self, meta: &Metadata) -> Self {
            self.metadata = Some(meta.clone());
            self
        }

        fn with_policy(mut self, policy: EncodePolicy) -> Self {
            self.policy = policy;
            self
        }

        fn with_loop_count(mut self, count: Option<u32>) -> Self {
            self.loop_count = count;
            self
        }

        fn encoder(self) -> Result<JxlEncoder<'a>, At<JxlError>> {
            let mode = apply_threads(&self.config.mode, &self.limits);
            Ok(JxlEncoder {
                mode,
                metadata: self.metadata,
                policy: self.policy,
                limits: self.limits,
                stop: self.stop,
                stream: StreamState::Empty,
            })
        }

        fn full_frame_encoder(self) -> Result<JxlFullFrameEncoder, At<JxlError>> {
            let mode = apply_threads(&self.config.mode, &self.limits);
            Ok(JxlFullFrameEncoder::from_job(
                mode,
                self.metadata.as_ref(),
                &self.policy,
                self.limits,
                self.loop_count,
            ))
        }
    }

    // ── JxlEncoder ──────────────────────────────────────────────────────

    /// Streaming state for incremental row pushing.
    enum StreamState {
        /// No rows pushed yet.
        Empty,
        /// Accumulating raw pixel bytes.
        Accumulating {
            width: u32,
            layout: PixelLayout,
            descriptor: PixelDescriptor,
            data: Vec<u8>,
            rows_pushed: u32,
        },
    }

    /// Single-image JPEG XL encoder.
    ///
    /// Supports both one-shot encoding via [`encode()`](zencodec::encode::Encoder::encode)
    /// and incremental row-level encoding via
    /// [`push_rows()`](zencodec::encode::Encoder::push_rows) +
    /// [`finish()`](zencodec::encode::Encoder::finish).
    pub struct JxlEncoder<'a> {
        mode: JxlEncMode,
        metadata: Option<Metadata>,
        policy: EncodePolicy,
        limits: Option<ResourceLimits>,
        stop: Option<&'a dyn Stop>,
        stream: StreamState,
    }

    impl JxlEncoder<'_> {
        /// Map a PixelDescriptor to the jxl-encoder PixelLayout.
        fn descriptor_to_layout(desc: PixelDescriptor) -> Result<PixelLayout, At<JxlError>> {
            let layout = desc.layout();
            let ct = desc.channel_type();
            match (layout, ct) {
                // U8
                (ChannelLayout::Rgb, ChannelType::U8) => Ok(PixelLayout::Rgb8),
                (ChannelLayout::Rgba, ChannelType::U8) => Ok(PixelLayout::Rgba8),
                (ChannelLayout::Bgra, ChannelType::U8) => Ok(PixelLayout::Bgra8),
                (ChannelLayout::Gray, ChannelType::U8) => Ok(PixelLayout::Gray8),
                (ChannelLayout::GrayAlpha, ChannelType::U8) => Ok(PixelLayout::GrayAlpha8),
                // U16
                (ChannelLayout::Rgb, ChannelType::U16) => Ok(PixelLayout::Rgb16),
                (ChannelLayout::Rgba, ChannelType::U16) => Ok(PixelLayout::Rgba16),
                (ChannelLayout::Gray, ChannelType::U16) => Ok(PixelLayout::Gray16),
                (ChannelLayout::GrayAlpha, ChannelType::U16) => Ok(PixelLayout::GrayAlpha16),
                // F32 (jxl-encoder assumes linear-light for float)
                (ChannelLayout::Rgb, ChannelType::F32) => Ok(PixelLayout::RgbLinearF32),
                (ChannelLayout::Rgba, ChannelType::F32) => Ok(PixelLayout::RgbaLinearF32),
                (ChannelLayout::Gray, ChannelType::F32) => Ok(PixelLayout::GrayLinearF32),
                (ChannelLayout::GrayAlpha, ChannelType::F32) => Ok(PixelLayout::GrayAlphaLinearF32),
                _ => Err(at(JxlError::UnsupportedOperation(
                    UnsupportedOperation::PixelFormat,
                ))),
            }
        }
    }

    impl JxlEncoder<'_> {
        /// Build jxl-encoder ImageMetadata from the zencodec Metadata,
        /// respecting the EncodePolicy for what to embed.
        fn build_jxl_metadata(&self) -> Option<jxl_encoder::ImageMetadata<'_>> {
            let meta = self.metadata.as_ref()?;
            let mut jxl_meta = jxl_encoder::ImageMetadata::new();
            let mut has_any = false;

            if self.policy.resolve_icc(true) {
                if let Some(ref icc) = meta.icc_profile {
                    jxl_meta = jxl_meta.with_icc_profile(icc);
                    has_any = true;
                }
            }
            if self.policy.resolve_exif(true) {
                if let Some(ref exif) = meta.exif {
                    jxl_meta = jxl_meta.with_exif(exif);
                    has_any = true;
                }
            }
            if self.policy.resolve_xmp(true) {
                if let Some(ref xmp) = meta.xmp {
                    jxl_meta = jxl_meta.with_xmp(xmp);
                    has_any = true;
                }
            }

            has_any.then_some(jxl_meta)
        }

        /// Check ResourceLimits against the given dimensions and bytes-per-pixel.
        fn check_limits(&self, width: u32, height: u32, bpp: u32) -> Result<(), At<JxlError>> {
            if let Some(ref limits) = self.limits {
                limits
                    .check_dimensions(width, height)
                    .map_err(|e| at(JxlError::LimitExceeded(e.to_string())))?;
                let estimated = width as u64 * height as u64 * bpp as u64;
                limits
                    .check_memory(estimated)
                    .map_err(|e| at(JxlError::LimitExceeded(e.to_string())))?;
            }
            Ok(())
        }

        /// Encode pixel data using the encode_request API with metadata + stop support.
        fn encode_with_metadata(
            &self,
            data: &[u8],
            width: u32,
            height: u32,
            layout: PixelLayout,
        ) -> Result<Vec<u8>, At<JxlError>> {
            let jxl_meta = self.build_jxl_metadata();

            let encode = |req: jxl_encoder::EncodeRequest<'_>| -> Result<Vec<u8>, At<JxlError>> {
                let req = if let Some(ref meta) = jxl_meta {
                    req.with_metadata(meta)
                } else {
                    req
                };
                let req = if let Some(stop) = self.stop {
                    req.with_stop(stop)
                } else {
                    req
                };
                req.encode(data)
                    .map_err(|e| at(JxlError::Encode(e.into_inner())))
            };

            match &self.mode {
                JxlEncMode::Lossy(cfg) => encode(cfg.encode_request(width, height, layout)),
                JxlEncMode::Lossless(cfg) => encode(cfg.encode_request(width, height, layout)),
            }
        }
    }

    impl zencodec::encode::Encoder for JxlEncoder<'_> {
        type Error = At<JxlError>;

        fn reject(op: UnsupportedOperation) -> At<JxlError> {
            at(JxlError::UnsupportedOperation(op))
        }

        fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<JxlError>> {
            let layout = Self::descriptor_to_layout(pixels.descriptor())?;
            let width = pixels.width();
            let height = pixels.rows();
            let bpp = pixels.descriptor().bytes_per_pixel() as u32;
            self.check_limits(width, height, bpp)?;

            let data = pixels.contiguous_bytes();
            let encoded = self.encode_with_metadata(&data, width, height, layout)?;

            Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                .with_mime_type("image/jxl")
                .with_extension("jxl"))
        }

        fn encode_srgba8(
            self,
            data: &mut [u8],
            make_opaque: bool,
            width: u32,
            height: u32,
            stride_pixels: u32,
        ) -> Result<EncodeOutput, At<JxlError>> {
            let w = width as usize;
            let h = height as usize;
            let stride = stride_pixels as usize;

            if make_opaque {
                // Encode as RGB — strip alpha entirely for smaller output.
                self.check_limits(width, height, 3)?;
                let mut rgb = Vec::with_capacity(w * h * 3);
                for y in 0..h {
                    let row_start = y * stride * 4;
                    let row = &data[row_start..row_start + w * 4];
                    for px in row.chunks_exact(4) {
                        rgb.push(px[0]);
                        rgb.push(px[1]);
                        rgb.push(px[2]);
                    }
                }
                let encoded = self.encode_with_metadata(&rgb, width, height, PixelLayout::Rgb8)?;
                Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                    .with_mime_type("image/jxl")
                    .with_extension("jxl"))
            } else {
                // RGBA path — copy contiguous if strided, otherwise zero-copy.
                self.check_limits(width, height, 4)?;
                let pixel_data: alloc::borrow::Cow<'_, [u8]> = if stride == w {
                    alloc::borrow::Cow::Borrowed(&data[..w * h * 4])
                } else {
                    let mut buf = Vec::with_capacity(w * h * 4);
                    for y in 0..h {
                        let row_start = y * stride * 4;
                        buf.extend_from_slice(&data[row_start..row_start + w * 4]);
                    }
                    alloc::borrow::Cow::Owned(buf)
                };
                let encoded =
                    self.encode_with_metadata(&pixel_data, width, height, PixelLayout::Rgba8)?;
                Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                    .with_mime_type("image/jxl")
                    .with_extension("jxl"))
            }
        }

        fn push_rows(&mut self, rows: PixelSlice<'_>) -> Result<(), At<JxlError>> {
            let desc = rows.descriptor();
            let layout = Self::descriptor_to_layout(desc)?;
            let width = rows.width();
            let num_rows = rows.rows();
            let bytes = rows.contiguous_bytes();

            match &mut self.stream {
                StreamState::Empty => {
                    let mut data = Vec::new();
                    data.extend_from_slice(&bytes);
                    self.stream = StreamState::Accumulating {
                        width,
                        layout,
                        descriptor: desc,
                        data,
                        rows_pushed: num_rows,
                    };
                }
                StreamState::Accumulating {
                    width: w,
                    descriptor: d,
                    data,
                    rows_pushed,
                    ..
                } => {
                    if width != *w || desc != *d {
                        return Err(at(JxlError::InvalidInput(
                            "push_rows: width or pixel format changed between calls".into(),
                        )));
                    }
                    data.extend_from_slice(&bytes);
                    *rows_pushed += num_rows;
                }
            }
            Ok(())
        }

        fn finish(self) -> Result<EncodeOutput, At<JxlError>> {
            let StreamState::Accumulating {
                width,
                layout,
                descriptor,
                ref data,
                rows_pushed,
                ..
            } = self.stream
            else {
                return Err(at(JxlError::InvalidInput(
                    "finish: no rows were pushed".into(),
                )));
            };

            let bpp = descriptor.bytes_per_pixel() as u32;
            self.check_limits(width, rows_pushed, bpp)?;

            let encoded = self.encode_with_metadata(data, width, rows_pushed, layout)?;

            Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                .with_mime_type("image/jxl")
                .with_extension("jxl"))
        }
    }

    // ── JxlFullFrameEncoder ──────────────────────────────────────────────

    /// Owned metadata for animation encoding (must be `'static` per trait bounds).
    struct OwnedAnimMeta {
        exif: Option<Vec<u8>>,
        xmp: Option<Vec<u8>>,
    }

    /// Animation JPEG XL encoder.
    ///
    /// Collects frames, then encodes them all at once via
    /// `jxl-encoder`'s `encode_animation`.
    ///
    /// # Limitations
    ///
    /// ICC profile embedding is not supported for animation encoding.
    /// The jxl-encoder animation API (`encode_animation`) generates the
    /// codestream internally and does not accept metadata parameters.
    /// ICC profiles are embedded in the JXL codestream image header, not
    /// in container boxes, so they cannot be wrapped via `wrap_in_container`.
    /// Single-frame encoding supports ICC via `EncodeRequest::with_metadata`.
    pub struct JxlFullFrameEncoder {
        mode: JxlEncMode,
        anim_meta: Option<OwnedAnimMeta>,
        limits: Option<ResourceLimits>,
        loop_count: Option<u32>,
        /// Duration per frame in milliseconds.
        frames: Vec<u32>,
        /// Raw pixel data for each frame (owned copies).
        pixel_data: Vec<Vec<u8>>,
        /// Total accumulated pixel data in bytes (for memory limit checking).
        accumulated_bytes: u64,
        width: u32,
        height: u32,
        layout: Option<PixelLayout>,
    }

    impl JxlFullFrameEncoder {
        /// Create from job state, copying metadata we need for container wrapping.
        fn from_job(
            mode: JxlEncMode,
            metadata: Option<&Metadata>,
            policy: &EncodePolicy,
            limits: Option<ResourceLimits>,
            loop_count: Option<u32>,
        ) -> Self {
            // For animation, ICC is handled in the codestream by jxl-encoder.
            // EXIF/XMP go in the container — copy them out now so we're 'static.
            let anim_meta = metadata.and_then(|meta| {
                let exif = if policy.resolve_exif(true) {
                    meta.exif.as_deref().map(|b| b.to_vec())
                } else {
                    None
                };
                let xmp = if policy.resolve_xmp(true) {
                    meta.xmp.as_deref().map(|b| b.to_vec())
                } else {
                    None
                };
                if exif.is_some() || xmp.is_some() {
                    Some(OwnedAnimMeta { exif, xmp })
                } else {
                    None
                }
            });

            Self {
                mode,
                anim_meta,
                limits,
                loop_count,
                frames: Vec::new(),
                pixel_data: Vec::new(),
                accumulated_bytes: 0,
                width: 0,
                height: 0,
                layout: None,
            }
        }

        /// Wrap an encoded animation codestream with EXIF/XMP metadata boxes.
        fn wrap_with_metadata(self, codestream: Vec<u8>) -> Vec<u8> {
            let meta = match self.anim_meta {
                Some(m) => m,
                None => return codestream,
            };

            let exif = meta.exif.as_deref();
            let xmp = meta.xmp.as_deref();

            if exif.is_none() && xmp.is_none() {
                return codestream;
            }

            jxl_encoder::container::wrap_in_container(&codestream, exif, xmp)
        }
    }

    impl zencodec::encode::FullFrameEncoder for JxlFullFrameEncoder {
        type Error = At<JxlError>;

        fn reject(op: UnsupportedOperation) -> At<JxlError> {
            at(JxlError::UnsupportedOperation(op))
        }

        fn push_frame(
            &mut self,
            pixels: PixelSlice<'_>,
            duration_ms: u32,
            stop: Option<&dyn Stop>,
        ) -> Result<(), At<JxlError>> {
            // Check cancellation before doing any work.
            if let Some(stop) = stop {
                stop.check().map_err(|e| at(JxlError::Cancelled(e)))?;
            }

            let layout = JxlEncoder::descriptor_to_layout(pixels.descriptor())?;
            let w = pixels.width();
            let h = pixels.rows();

            if self.pixel_data.is_empty() {
                // Validate dimensions against limits on first frame.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_dimensions(w, h)
                        .map_err(|e| at(JxlError::LimitExceeded(e.to_string())))?;
                }
                self.width = w;
                self.height = h;
                self.layout = Some(layout);
            } else if w != self.width || h != self.height {
                return Err(at(JxlError::InvalidInput(
                    "animation frame dimensions must match first frame".into(),
                )));
            }

            // Check max_frames limit.
            if let Some(ref limits) = self.limits {
                limits
                    .check_frames(self.pixel_data.len() as u32 + 1)
                    .map_err(|e| at(JxlError::LimitExceeded(e.to_string())))?;
            }

            let frame_data = pixels.contiguous_bytes().into_owned();
            let frame_bytes = frame_data.len() as u64;
            self.accumulated_bytes += frame_bytes;

            // Check accumulated memory across ALL frames, not just the first.
            if let Some(ref limits) = self.limits {
                limits
                    .check_memory(self.accumulated_bytes)
                    .map_err(|e| at(JxlError::LimitExceeded(e.to_string())))?;
            }

            self.pixel_data.push(frame_data);
            self.frames.push(duration_ms);
            Ok(())
        }

        fn finish(self, stop: Option<&dyn Stop>) -> Result<EncodeOutput, At<JxlError>> {
            // Check cancellation before expensive encode.
            if let Some(stop) = stop {
                stop.check().map_err(|e| at(JxlError::Cancelled(e)))?;
            }

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

            let encoded = self.wrap_with_metadata(encoded);

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
    use alloc::borrow::Cow;
    use alloc::collections::VecDeque;
    use alloc::vec;
    use alloc::vec::Vec;

    use jxl::api::{
        ExtraChannel, JxlDecoder as JxlRsDecoder, JxlDecoderOptions, JxlOutputBuffer,
        ProcessingResult,
    };
    use zencodec::Unsupported;
    use zencodec::decode::{
        DecodeCapabilities, DecodeOutput, OutputInfo, SinkError,
    };
    use zencodec::{FullFrame, OwnedFullFrame};
    use zencodec::{ImageInfo, ResourceLimits, UnsupportedOperation};
    use zenpixels::Cicp;

    use enough::Stop;

    use crate::decode::{
        JxlInfo, JxlLimits, build_pixel_data, choose_pixel_format, decode_with_parallel,
        extract_color_info, is_hdr_or_wide_gamut, probe,
    };

    /// Helper to wrap a JxlError with location tracking.
    fn at(e: JxlError) -> At<JxlError> {
        whereat::at(e)
    }

    /// Determine the decoder `parallel` flag from a [`ThreadingPolicy`].
    ///
    /// Returns `Some(false)` for single-threaded, `Some(true)` for explicitly
    /// multi-threaded, or `None` to keep the decoder default.
    fn policy_to_parallel(limits: &Option<ResourceLimits>) -> Option<bool> {
        limits
            .as_ref()
            .map(|l| !matches!(l.threading(), zencodec::ThreadingPolicy::SingleThread))
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
        .with_enforces_max_pixels(true)
        .with_enforces_max_memory(true)
        .with_threads_supported_range(1, u16::MAX);

    /// Supported pixel descriptors for decoding.
    ///
    /// jxl-rs can decode to U8/U16/F32 × Gray/GrayAlpha/RGB/RGBA.
    static JXL_DECODE_DESCRIPTORS: &[PixelDescriptor] = &[
        // 8-bit
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::GRAY8_SRGB,
        PixelDescriptor::GRAYA8_SRGB,
        // 16-bit
        PixelDescriptor::RGB16_SRGB,
        PixelDescriptor::RGBA16_SRGB,
        PixelDescriptor::GRAY16_SRGB,
        PixelDescriptor::GRAYA16_SRGB,
        // f32 linear
        PixelDescriptor::RGBF32_LINEAR,
        PixelDescriptor::RGBAF32_LINEAR,
        PixelDescriptor::GRAYF32_LINEAR,
        PixelDescriptor::GRAYAF32_LINEAR,
    ];

    // ── JxlDecoderConfig ────────────────────────────────────────────────

    /// JPEG XL decoder configuration.
    ///
    /// Implements [`zencodec::decode::DecoderConfig`].
    #[derive(Clone, Debug, Default)]
    pub struct JxlDecoderConfig {
        _priv: (),
    }

    impl JxlDecoderConfig {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl zencodec::decode::DecoderConfig for JxlDecoderConfig {
        type Error = At<JxlError>;
        type Job<'a> = JxlDecodeJob<'a>;

        fn formats() -> &'static [ImageFormat] {
            &[ImageFormat::Jxl]
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
                start_frame_index: 0,
            }
        }
    }

    // ── JxlDecodeJob ────────────────────────────────────────────────────

    /// Per-operation decode job for JPEG XL.
    pub struct JxlDecodeJob<'a> {
        limits: Option<ResourceLimits>,
        _stop: Option<&'a dyn Stop>,
        start_frame_index: u32,
    }

    impl JxlDecodeJob<'_> {
        /// Convert native JxlInfo into zencodec ImageInfo.
        fn jxl_info_to_image_info(info: &JxlInfo) -> ImageInfo {
            let mut image_info = ImageInfo::new(info.width, info.height, ImageFormat::Jxl)
                .with_alpha(info.has_alpha)
                .with_animation(info.has_animation);

            image_info =
                image_info.with_orientation(zencodec::Orientation::from_exif(info.orientation as u16));

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

        /// Determine the native output pixel descriptor from probe info.
        fn native_descriptor(info: &JxlInfo) -> PixelDescriptor {
            let bit_depth_u8 = info.bit_depth.unwrap_or(8);
            let is_float = bit_depth_u8 == 32;
            let is_16 = bit_depth_u8 > 8 && !is_float;

            match (info.is_gray, info.has_alpha, is_float, is_16) {
                // f32
                (true, true, true, _) => PixelDescriptor::GRAYAF32_LINEAR,
                (true, false, true, _) => PixelDescriptor::GRAYF32_LINEAR,
                (false, true, true, _) => PixelDescriptor::RGBAF32_LINEAR,
                (false, false, true, _) => PixelDescriptor::RGBF32_LINEAR,
                // u16
                (true, true, _, true) => PixelDescriptor::GRAYA16_SRGB,
                (true, false, _, true) => PixelDescriptor::GRAY16_SRGB,
                (false, true, _, true) => PixelDescriptor::RGBA16_SRGB,
                (false, false, _, true) => PixelDescriptor::RGB16_SRGB,
                // u8
                (true, true, _, _) => PixelDescriptor::GRAYA8_SRGB,
                (true, false, _, _) => PixelDescriptor::GRAY8_SRGB,
                (false, true, _, _) => PixelDescriptor::RGBA8_SRGB,
                (false, false, _, _) => PixelDescriptor::RGB8_SRGB,
            }
        }
    }

    impl<'a> zencodec::decode::DecodeJob<'a> for JxlDecodeJob<'a> {
        type Error = At<JxlError>;
        type Dec = JxlDecoder<'a>;
        type StreamDec = Unsupported<At<JxlError>>;
        type FullFrameDec = JxlFullFrameDecoder;

        fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
            self._stop = Some(stop);
            self
        }

        fn with_limits(mut self, limits: ResourceLimits) -> Self {
            self.limits = Some(limits);
            self
        }

        fn with_start_frame_index(mut self, index: u32) -> Self {
            self.start_frame_index = index;
            self
        }

        fn probe(&self, data: &[u8]) -> Result<ImageInfo, At<JxlError>> {
            let info = probe(data).map_err(at)?;
            let image_info = Self::jxl_info_to_image_info(&info).with_source_encoding_details(info);
            Ok(image_info)
        }

        fn output_info(&self, data: &[u8]) -> Result<OutputInfo, At<JxlError>> {
            let info = probe(data).map_err(at)?;
            let native_desc = Self::native_descriptor(&info);
            Ok(
                OutputInfo::full_decode(info.width, info.height, native_desc)
                    .with_alpha(info.has_alpha),
            )
        }

        fn decoder(
            self,
            data: Cow<'a, [u8]>,
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
            _data: Cow<'a, [u8]>,
            _preferred: &[PixelDescriptor],
        ) -> Result<Unsupported<At<JxlError>>, At<JxlError>> {
            Err(at(JxlError::UnsupportedOperation(
                UnsupportedOperation::RowLevelDecode,
            )))
        }

        fn push_decoder(
            self,
            data: Cow<'a, [u8]>,
            sink: &mut dyn zencodec::decode::DecodeRowSink,
            preferred: &[PixelDescriptor],
        ) -> Result<OutputInfo, At<JxlError>> {
            zencodec::helpers::copy_decode_to_sink(self, data, sink, preferred, |e| at(JxlError::Sink(e)))
        }

        fn full_frame_decoder(
            self,
            data: Cow<'a, [u8]>,
            preferred: &[PixelDescriptor],
        ) -> Result<JxlFullFrameDecoder, At<JxlError>> {
            // Eagerly probe to populate image_info so info() never panics.
            let info = probe(&data).map_err(at)?;
            let image_info = Arc::new(Self::jxl_info_to_image_info(&info));
            Ok(JxlFullFrameDecoder {
                data: data.into_owned(),
                limits: self.limits,
                preferred: preferred.to_vec(),
                frames: None,
                image_info: Some(image_info),
                current: None,
                start_frame_index: self.start_frame_index,
            })
        }
    }

    // ── JxlDecoder ──────────────────────────────────────────────────────

    /// Single-image JPEG XL decoder.
    pub struct JxlDecoder<'a> {
        data: Cow<'a, [u8]>,
        limits: Option<ResourceLimits>,
        preferred: Vec<PixelDescriptor>,
    }

    impl zencodec::decode::Decode for JxlDecoder<'_> {
        type Error = At<JxlError>;

        fn decode(self) -> Result<DecodeOutput, At<JxlError>> {
            let native_limits = JxlDecodeJob::to_native_limits(&self.limits);
            let parallel = policy_to_parallel(&self.limits);
            let result = decode_with_parallel(
                &self.data,
                native_limits.as_ref(),
                &self.preferred,
                parallel,
            )
            .map_err(at)?;

            let info = JxlDecodeJob::jxl_info_to_image_info(&result.info);
            Ok(DecodeOutput::new(result.pixels, info).with_source_encoding_details(result.info))
        }
    }

    // ── JxlFullFrameDecoder ──────────────────────────────────────────────

    /// Animation JPEG XL decoder (fully composited frames).
    ///
    /// Decodes all frames eagerly on first call to `render_next_frame()` — the
    /// jxl-rs decoder handles blending/disposal internally, producing
    /// fully composited frames.
    pub struct JxlFullFrameDecoder {
        data: Vec<u8>,
        limits: Option<ResourceLimits>,
        preferred: Vec<PixelDescriptor>,
        /// Pre-decoded frames (lazily populated on first render_next_frame call).
        frames: Option<DecodedFrames>,
        /// Image info, set after decoding.
        image_info: Option<Arc<ImageInfo>>,
        /// Current frame for borrowed access via `render_next_frame`.
        current: Option<OwnedFullFrame>,
        /// Number of displayed frames to skip from the front.
        start_frame_index: u32,
    }

    struct DecodedFrames {
        frames: VecDeque<OwnedFullFrame>,
        loop_count: Option<u32>,
    }

    impl JxlFullFrameDecoder {
        /// Decode all frames up front.
        fn decode_all_frames(&mut self) -> Result<(), At<JxlError>> {
            let mut options = JxlDecoderOptions::default();

            if let Some(p) = policy_to_parallel(&self.limits) {
                options.parallel = p;
            }

            if let Some(ref lim) = self.limits
                && let Some(max_px) = lim.max_pixels
            {
                options.limits.max_pixels = Some(max_px as usize);
            }

            let decoder = JxlRsDecoder::new(options);

            // Parse header
            let mut input: &[u8] = &self.data;
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
                is_gray,
            };
            let image_info = Arc::new(JxlDecodeJob::jxl_info_to_image_info(&jxl_info));

            let is_f32 = matches!(
                chosen.pixel_format.color_data_format,
                Some(jxl::api::JxlDataFormat::F32 { .. })
            );
            // Only clamp when CICP explicitly tells us it's SDR.
            // When CICP is absent (ICC-only), we don't know the gamut.
            let clamp = is_f32 && cicp.is_some() && !is_hdr_or_wide_gamut(cicp);

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

                // Skip frames before start_frame_index: decode them (jxl-rs
                // requires sequential decode) but drop immediately instead of
                // storing in the VecDeque.  This avoids holding all skipped
                // frames in memory at peak.
                if frame_index >= self.start_frame_index {
                    if clamp {
                        crate::decode::clamp_f32_buf(&mut buf);
                    }
                    let pixel_buf = build_pixel_data(&buf, width, height, &chosen);
                    frames.push_back(OwnedFullFrame::new(pixel_buf, duration_ms, frame_index));
                }

                frame_index += 1;

                if !next_decoder.has_more_frames() {
                    break;
                }
                decoder = next_decoder;
            }

            self.image_info = Some(image_info);
            self.frames = Some(DecodedFrames { frames, loop_count });

            Ok(())
        }
    }

    impl zencodec::decode::FullFrameDecoder for JxlFullFrameDecoder {
        type Error = At<JxlError>;

        fn wrap_sink_error(err: SinkError) -> At<JxlError> {
            at(JxlError::Sink(err))
        }

        fn info(&self) -> &ImageInfo {
            self.image_info
                .as_ref()
                .expect("info() called before decode_all_frames()")
        }

        fn frame_count(&self) -> Option<u32> {
            self.frames.as_ref().map(|f| f.frames.len() as u32)
        }

        fn loop_count(&self) -> Option<u32> {
            self.frames.as_ref().and_then(|f| f.loop_count)
        }

        fn render_next_frame(
            &mut self,
            _stop: Option<&dyn Stop>,
        ) -> Result<Option<FullFrame<'_>>, At<JxlError>> {
            if self.frames.is_none() {
                self.decode_all_frames()?;
            }

            let decoded = self.frames.as_mut().unwrap();
            self.current = decoded.frames.pop_front();
            Ok(self.current.as_ref().map(|f| f.as_full_frame()))
        }

        fn render_next_frame_to_sink(
            &mut self,
            stop: Option<&dyn Stop>,
            sink: &mut dyn zencodec::decode::DecodeRowSink,
        ) -> Result<Option<OutputInfo>, At<JxlError>> {
            zencodec::helpers::copy_frame_to_sink(self, stop, sink)
        }

        fn render_next_frame_owned(
            &mut self,
            _stop: Option<&dyn Stop>,
        ) -> Result<Option<OwnedFullFrame>, At<JxlError>> {
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
pub use encoding::{JxlEncodeJob, JxlEncoder, JxlEncoderConfig, JxlFullFrameEncoder};

#[cfg(feature = "decode")]
pub use decoding::{JxlDecodeJob, JxlDecoder, JxlDecoderConfig, JxlFullFrameDecoder};

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::borrow::Cow;
    use alloc::vec;
    use alloc::vec::Vec;

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_config_defaults() {
        use zencodec::encode::EncoderConfig;
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
        use zencodec::encode::EncoderConfig;
        let config = JxlEncoderConfig::new()
            .with_generic_quality(85.0)
            .with_generic_effort(7);
        assert_eq!(config.generic_quality(), Some(85.0));
        assert_eq!(config.generic_effort(), Some(7));
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encoder_config_lossless() {
        use zencodec::encode::EncoderConfig;
        let config = JxlEncoderConfig::new().with_lossless(true);
        assert_eq!(config.is_lossless(), Some(true));
        assert!(config.lossless_config().is_some());
        assert!(config.lossy_config().is_none());
    }

    #[cfg(feature = "encode")]
    #[test]
    fn quality_mapping_matches_jxl_encoder() {
        // Verify our piecewise mapping matches jxl-encoder's percent_to_distance.
        assert_eq!(quality_to_distance(100.0), 0.0);
        assert_eq!(quality_to_distance(90.0), 1.0); // visually lossless
        assert_eq!(quality_to_distance(80.0), 1.5);
        assert_eq!(quality_to_distance(70.0), 2.0);
        assert_eq!(quality_to_distance(50.0), 4.0);
        assert_eq!(quality_to_distance(0.0), 9.0);
        // Clamped above 100
        assert_eq!(quality_to_distance(110.0), 0.0);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn quality_sets_correct_distance() {
        use zencodec::encode::EncoderConfig;
        // Generic quality 90 is calibrated to JXL-native ~84.2,
        // which maps to butteraugli distance via quality_to_distance().
        let config = JxlEncoderConfig::new().with_generic_quality(90.0);
        let calibrated = calibrated_jxl_quality(90.0);
        let expected_distance = quality_to_distance(calibrated);
        let lossy = config.lossy_config().unwrap();
        assert!(
            (lossy.distance() - expected_distance).abs() < 0.001,
            "expected distance {expected_distance}, got {}",
            lossy.distance()
        );
    }

    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn roundtrip_rgb8() {
        use zencodec::decode::Decode;
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zencodec::decode::{DecodeJob, DecoderConfig};
        let dec_config = JxlDecoderConfig::new();
        let decoder = dec_config
            .job()
            .decoder(Cow::Borrowed(output.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
    }

    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn roundtrip_rgba8() {
        use zencodec::decode::Decode;
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

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

        use zencodec::decode::{DecodeJob, DecoderConfig};
        let dec_config = JxlDecoderConfig::new();
        let decoder = dec_config
            .job()
            .decoder(Cow::Borrowed(output.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
    }

    #[cfg(feature = "decode")]
    #[test]
    fn probe_returns_info() {
        use zencodec::decode::{DecodeJob, DecoderConfig};

        // Encode a minimal image to probe
        #[cfg(feature = "encode")]
        {
            use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

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
        use zencodec::decode::{DecodeJob, DecoderConfig};
        use zencodec::{CodecErrorExt, UnsupportedOperation};

        let dec_config = JxlDecoderConfig::new();
        let job = dec_config.job();
        let result = job.streaming_decoder(Cow::Borrowed(&[0xFF]), &[]);
        match result {
            Err(err) => {
                assert_eq!(
                    err.error().unsupported_operation(),
                    Some(&UnsupportedOperation::RowLevelDecode)
                );
            }
            Ok(_) => panic!("expected error"),
        }
    }

    #[cfg(feature = "encode")]
    #[test]
    fn capabilities_correct() {
        use zencodec::encode::EncoderConfig;
        let caps = JxlEncoderConfig::capabilities();
        assert!(caps.lossy());
        assert!(caps.lossless());
        assert!(caps.hdr());
        assert!(caps.native_gray());
        assert!(caps.native_alpha());
        assert!(caps.native_16bit(), "native_16bit should be reported");
        assert!(caps.native_f32());
        assert!(
            caps.push_rows(),
            "push_rows should be reported since push_rows/finish are implemented"
        );
        assert!(caps.animation());
        assert!(caps.enforces_max_pixels());
        assert!(caps.enforces_max_memory());
        assert!(caps.stop());
        assert_eq!(caps.threads_supported_range(), (1, u16::MAX));
    }

    #[cfg(feature = "decode")]
    #[test]
    fn decode_capabilities_correct() {
        use zencodec::decode::DecoderConfig;
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
        assert!(caps.enforces_max_pixels());
        assert!(
            caps.enforces_max_memory(),
            "enforces_max_memory should be reported"
        );
        assert_eq!(caps.threads_supported_range(), (1, u16::MAX));
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_descriptors_cover_all_layouts() {
        use zencodec::encode::EncoderConfig;
        let descs = JxlEncoderConfig::supported_descriptors();
        // Should include RGB, RGBA, BGRA, Gray, GrayAlpha across U8/U16/F32
        assert!(descs.len() >= 13);
    }

    #[cfg(feature = "decode")]
    #[test]
    fn decode_descriptors_cover_all_layouts() {
        use zencodec::decode::DecoderConfig;
        let descs = JxlDecoderConfig::supported_descriptors();
        // Should include RGB, RGBA, Gray, GrayAlpha across U8/U16/F32
        assert!(descs.len() >= 12);
    }

    /// Encode with SingleThread threading policy and verify output is valid.
    #[cfg(feature = "encode")]
    #[test]
    fn encode_single_thread() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zencodec::{ResourceLimits, ThreadingPolicy};

        let width = 16u32;
        let height = 16u32;
        let pixels: Vec<rgb::Rgb<u8>> = (0..width * height)
            .map(|i| {
                let v = (i % 256) as u8;
                rgb::Rgb { r: v, g: v, b: v }
            })
            .collect();
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        let limits = ResourceLimits::none().with_threading(ThreadingPolicy::SingleThread);
        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().with_limits(limits).encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        assert!(!output.data().is_empty());
        assert_eq!(output.format(), ImageFormat::Jxl);
    }

    /// Roundtrip encode+decode with SingleThread threading policy.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn roundtrip_single_thread() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zencodec::{ResourceLimits, ThreadingPolicy};

        let width = 16u32;
        let height = 16u32;
        let pixels: Vec<rgb::Rgb<u8>> = (0..width * height)
            .map(|i| {
                let v = (i % 256) as u8;
                rgb::Rgb { r: v, g: v, b: v }
            })
            .collect();
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        let limits = ResourceLimits::none().with_threading(ThreadingPolicy::SingleThread);

        // Encode with single thread
        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().with_limits(limits.clone()).encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();
        assert!(!output.data().is_empty());

        // Decode with single thread
        let dec_config = JxlDecoderConfig::new();
        let decoder = dec_config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(output.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
    }

    /// Verify generic_quality() returns the original value, not the calibrated one.
    #[cfg(feature = "encode")]
    #[test]
    fn generic_quality_roundtrips() {
        use zencodec::encode::EncoderConfig;
        for q in [0.0, 10.0, 25.0, 50.0, 75.0, 85.0, 90.0, 95.0, 100.0] {
            let config = JxlEncoderConfig::new().with_generic_quality(q);
            assert_eq!(
                config.generic_quality(),
                Some(q),
                "generic_quality() should return original value {q}, not calibrated"
            );
        }
    }

    /// Verify calibrated quality is used internally for distance.
    #[cfg(feature = "encode")]
    #[test]
    fn calibrated_quality_used_for_distance() {
        use zencodec::encode::EncoderConfig;
        // Generic quality 50 calibrates to ~48.5, which gives distance ~4.15
        let config = JxlEncoderConfig::new().with_generic_quality(50.0);
        let calibrated = calibrated_jxl_quality(50.0);
        let expected_distance = quality_to_distance(calibrated);
        let lossy = config.lossy_config().unwrap();
        assert!(
            (lossy.distance() - expected_distance).abs() < 0.01,
            "distance should reflect calibrated quality, not raw generic quality"
        );
        // The calibrated distance should differ from non-calibrated
        let naive_distance = quality_to_distance(50.0);
        assert!(
            (expected_distance - naive_distance).abs() > 0.01,
            "calibration should change the distance (got same value)"
        );
    }

    /// Verify info() works before render_next_frame is called.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn full_frame_decoder_info_before_render() {
        use zencodec::decode::{DecodeJob, DecoderConfig, FullFrameDecoder};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        // Encode a minimal image.
        let width = 4u32;
        let height = 4u32;
        let pixels: Vec<rgb::Rgb<u8>> = vec![
            rgb::Rgb {
                r: 128,
                g: 64,
                b: 32
            };
            16
        ];
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();
        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        // Create a full frame decoder but do NOT call render_next_frame yet.
        let dec_config = JxlDecoderConfig::new();
        let ffd = dec_config
            .job()
            .full_frame_decoder(Cow::Borrowed(output.data()), &[])
            .unwrap();

        // info() should return valid data without panicking.
        let info = ffd.info();
        assert_eq!(info.width, width);
        assert_eq!(info.height, height);
        assert_eq!(info.format, ImageFormat::Jxl);
    }
}
