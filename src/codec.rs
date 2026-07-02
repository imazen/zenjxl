//! zencodec trait implementations for JPEG XL.
//!
//! Thin adapter layer over the native `zenjxl` encode/decode API.
//!
//! # Trait mapping
//!
//! | zencodec | zenjxl adapter |
//! |----------------|----------------|
//! | `EncoderConfig` | [`JxlEncoderConfig`] |
//! | `EncodeJob` | [`JxlEncodeJob`] |
//! | `Encoder` | [`JxlEncoder`] |
//! | `AnimationFrameEncoder` | [`JxlAnimationFrameEncoder`] |
//! | `DecoderConfig` | [`JxlDecoderConfig`] |
//! | `DecodeJob<'a>` | [`JxlDecodeJob`] |
//! | `Decode` | [`JxlDecoder`] |
//! | `AnimationFrameDecoder` | [`JxlAnimationFrameDecoder`] |

#[cfg(any(feature = "encode", feature = "decode"))]
use alloc::sync::Arc;
#[cfg(any(feature = "encode", feature = "decode"))]
use zencodec::ImageFormat;
#[cfg(any(feature = "encode", feature = "decode"))]
use zenpixels::PixelDescriptor;

#[cfg(any(feature = "encode", feature = "decode"))]
use crate::error::JxlError;
// `map_err_at` is only called inside the `encode`/`decode` modules; gate the
// import to match so the `zencodec`-only build doesn't see it as unused.
#[cfg(any(feature = "encode", feature = "decode"))]
use whereat::ResultAtExt;

#[cfg(any(feature = "encode", feature = "decode"))]
type At<E> = whereat::At<E>;

#[cfg(feature = "encode")]
use jxl_encoder::{calibrated_jxl_quality, quality_to_distance};

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

    /// Apply threading policy from [`ResourceLimits`] to a [`JxlEncMode`].
    ///
    /// `is_parallel() == true` → threads=0 (ambient rayon pool).
    /// `is_parallel() == false` → threads=1 (sequential).
    fn apply_threads(mode: &JxlEncMode, limits: &Option<ResourceLimits>) -> JxlEncMode {
        let threads = if limits
            .as_ref()
            .is_some_and(|l| !l.threading().is_parallel())
        {
            1
        } else {
            0
        };
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
        // Effort floor is 1, not 0: validate() rejects effort < 1 and the
        // generic-effort setter clamps to 1..=10 (see EFFORT_RANGE).
        .with_effort_range(1, 10)
        .with_quality_range(0.0, 100.0)
        .with_icc(true)
        .with_exif(true)
        .with_xmp(true)
        // JXL's codestream enum color is a standardized CICP carrier, and it is
        // safe as the sole color carrier (matches libjxl's want_icc=false).
        .with_cicp(true)
        .with_cicp_is_valid_carrier(true)
        .with_cicp_safe_sole_carrier(true)
        .with_gain_map(true)
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
        PixelDescriptor::RGBX8_SRGB,
        PixelDescriptor::BGRX8_SRGB,
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

    /// Pre-serialized gain map data for embedding in the JXL container.
    ///
    /// This holds the raw jhgm box payload (the output of
    /// [`GainMapBundle::serialize()`]). Wrapped in `Arc` so `JxlEncoderConfig`
    /// remains cheap to clone.
    #[derive(Clone, Debug)]
    pub struct GainMapData {
        /// Serialized jhgm box payload (version + metadata + color_encoding +
        /// alt_icc + gain map codestream).
        pub jhgm_payload: Vec<u8>,
    }

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
        /// Explicit butteraugli distance override. When set, bypasses the
        /// quality-to-distance calibration curve.
        distance_override: Option<f32>,
        effort: Option<i32>,
        lossless: bool,
        /// Enable noise synthesis in lossy mode.
        noise: bool,
        /// Optional gain map to embed as a jhgm box in the container.
        gain_map: Option<Arc<GainMapData>>,
    }

    impl JxlEncoderConfig {
        /// Create a default lossy encoder config (distance 1.0, effort 7).
        pub fn new() -> Self {
            Self {
                mode: JxlEncMode::Lossy(LossyConfig::new(1.0)),
                calibrated_quality: None,
                generic_quality: None,
                distance_override: None,
                effort: None,
                lossless: false,
                noise: false,
                gain_map: None,
            }
        }

        /// Attach a gain map for embedding in the output JXL container.
        ///
        /// The `jhgm_payload` is the serialized gain map bundle — the output
        /// of [`GainMapBundle::serialize()`]. When set, the encoder wraps
        /// the codestream in a JXL container and appends a `jhgm` box.
        ///
        /// # Example
        ///
        /// ```ignore
        /// use zenjxl::GainMapBundle;
        ///
        /// let bundle = GainMapBundle {
        ///     metadata: iso_metadata,
        ///     color_encoding: None,
        ///     alt_icc_compressed: None,
        ///     gain_map_codestream: gain_map_jxl,
        /// };
        /// let config = JxlEncoderConfig::new()
        ///     .with_gain_map(bundle.serialize());
        /// ```
        pub fn with_gain_map(mut self, jhgm_payload: Vec<u8>) -> Self {
            self.gain_map = Some(Arc::new(GainMapData { jhgm_payload }));
            self
        }

        /// Returns the gain map data if set.
        pub fn gain_map(&self) -> Option<&GainMapData> {
            self.gain_map.as_deref()
        }

        /// Set the butteraugli distance directly, bypassing calibration.
        ///
        /// This overrides any quality set via [`with_generic_quality`]. Valid
        /// range is 0.0 (mathematically lossless) to 25.0 (very low quality).
        /// A distance of 1.0 is visually lossless for most content.
        pub fn with_distance(mut self, distance: f32) -> Self {
            self.distance_override = Some(distance.clamp(0.0, 25.0));
            // Clear quality-based state since distance takes priority
            self.calibrated_quality = None;
            self.generic_quality = None;
            if !self.lossless {
                self.rebuild_lossy();
            }
            self
        }

        /// Enable or disable noise synthesis for lossy encoding.
        ///
        /// When enabled, the encoder synthesizes film-grain-like noise to
        /// mask compression artifacts at low bitrates.
        pub fn with_noise(mut self, enable: bool) -> Self {
            self.noise = enable;
            if !self.lossless {
                self.rebuild_lossy();
            }
            self
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

        /// Fail-fast validation of the configured encoder parameters.
        ///
        /// Encoder setters such as [`with_distance`](Self::with_distance) and
        /// [`with_generic_effort`](zencodec::encode::EncoderConfig::with_generic_effort)
        /// already clamp out-of-range values silently, which suits one-off
        /// encode calls but hides typos in batch jobs that fan a single
        /// config out across many encodes. `validate()` returns
        /// [`Err`](crate::ValidationError) for the same out-of-range inputs
        /// the setters silently accept, letting callers fail fast.
        ///
        /// Validates:
        /// - `generic_quality` in `0.0..=100.0` (and not NaN)
        /// - `distance_override` in `0.0..=25.0` (and not NaN)
        /// - `effort` in `1..=10`
        /// - `noise` is not combined with lossless mode (noise synthesis
        ///   is a lossy-VarDCT feature; under the modular path it would
        ///   be a silent no-op, so the combination is rejected rather
        ///   than remapped)
        ///
        /// The opaque `gain_map` payload is not parsed here — its validity
        /// surfaces during encode.
        pub fn validate(&self) -> Result<(), crate::ValidationError> {
            crate::validate::check_optional_f32_range(
                self.generic_quality,
                &crate::validate::GENERIC_QUALITY_RANGE,
                |value, valid| crate::ValidationError::GenericQualityOutOfRange { value, valid },
            )?;
            crate::validate::check_optional_f32_range(
                self.distance_override,
                &crate::validate::DISTANCE_RANGE,
                |value, valid| crate::ValidationError::DistanceOutOfRange { value, valid },
            )?;
            crate::validate::check_optional_i32_range(
                self.effort,
                &crate::validate::EFFORT_RANGE,
                |value, valid| crate::ValidationError::EffortOutOfRange { value, valid },
            )?;
            if self.lossless && self.noise {
                return Err(crate::ValidationError::NoiseInLosslessMode);
            }
            Ok(())
        }

        /// Rebuild the lossy mode from current quality/distance + effort + noise state.
        fn rebuild_lossy(&mut self) {
            let distance = self
                .distance_override
                .or_else(|| self.calibrated_quality.map(quality_to_distance))
                .unwrap_or(1.0);
            let mut cfg = LossyConfig::new(distance);
            if let Some(e) = self.effort {
                cfg = cfg.with_effort(e.clamp(1, 10) as u8);
            }
            if self.noise {
                cfg = cfg.with_noise(true);
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

        /// Resolve every knob to what the encoder will actually run —
        /// the introspection counterpart of an encode call.
        ///
        /// There is no second resolution implementation that could
        /// drift: the values are read off the **same**
        /// `LossyConfig`/`LosslessConfig` object the encode path
        /// consumes (the mode is rebuilt by every setter, so it already
        /// reflects quality→distance calibration, effort clamping, and
        /// upstream defaults). What this plan does NOT report is the
        /// effort→tool resolution inside jxl-encoder (which DCT classes,
        /// tree knobs, etc. an effort level enables) — that lives in
        /// jxl-encoder's `EffortProfile` and is deliberately not
        /// duplicated here; override it per-knob via the `__expert`
        /// internal-params types instead.
        ///
        /// Static plans report only what is statically knowable:
        /// content-dependent encoder decisions (auto EPF strength, auto
        /// resampling at high distance, noise modelling) are not
        /// guessed.
        #[cfg(feature = "__expert")]
        pub fn resolve_plan(&self) -> JxlEncodePlan {
            let container_forced = self.gain_map.is_some();
            match &self.mode {
                JxlEncMode::Lossy(c) => JxlEncodePlan::Lossy(LossyPlan {
                    distance: c.distance(),
                    distance_source: if self.distance_override.is_some() {
                        DistanceSource::Override
                    } else if self.calibrated_quality.is_some() {
                        DistanceSource::CalibratedQuality
                    } else {
                        DistanceSource::Default
                    },
                    generic_quality: self.generic_quality,
                    calibrated_native_quality: self.calibrated_quality,
                    effort: c.effort(),
                    noise: c.noise(),
                    container_forced,
                }),
                JxlEncMode::Lossless(c) => {
                    let mut inert_knobs = Vec::new();
                    if self.noise {
                        inert_knobs.push("noise");
                    }
                    if self.distance_override.is_some() {
                        inert_knobs.push("distance_override");
                    }
                    if self.generic_quality.is_some() {
                        inert_knobs.push("generic_quality");
                    }
                    JxlEncodePlan::Lossless(LosslessPlan {
                        effort: c.effort(),
                        container_forced,
                        inert_knobs,
                    })
                }
            }
        }
    }

    /// Where a [`LossyPlan`]'s resolved distance came from.
    #[cfg(feature = "__expert")]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum DistanceSource {
        /// [`JxlEncoderConfig::with_distance`] — calibration bypassed.
        Override,
        /// `with_generic_quality` through the
        /// `calibrated_jxl_quality` → `quality_to_distance` chain.
        CalibratedQuality,
        /// Neither set: the constructor default (distance 1.0).
        Default,
    }

    /// Resolved lossy (VarDCT) encode plan. Knobs that are dead on the
    /// lossy path do not exist here — mirrors the sweep module's
    /// variant discrimination.
    #[cfg(feature = "__expert")]
    #[derive(Clone, Debug)]
    pub struct LossyPlan {
        /// The butteraugli distance the encoder will run with (read off
        /// the stored `LossyConfig`).
        pub distance: f32,
        /// How `distance` was produced.
        pub distance_source: DistanceSource,
        /// The generic quality as given (0–100), when quality-driven.
        pub generic_quality: Option<f32>,
        /// The calibrated JXL-native quality (the intermediate value of
        /// the same chain the encoder ran), when quality-driven.
        pub calibrated_native_quality: Option<f32>,
        /// Resolved effort (upstream default 7 when unset).
        pub effort: u8,
        /// Noise synthesis (live on this path).
        pub noise: bool,
        /// A gain map forces JXL container framing around the
        /// codestream.
        pub container_forced: bool,
    }

    /// Resolved lossless (modular) encode plan.
    #[cfg(feature = "__expert")]
    #[derive(Clone, Debug)]
    pub struct LosslessPlan {
        /// Resolved effort (upstream default 7 when unset).
        pub effort: u8,
        /// A gain map forces JXL container framing around the
        /// codestream.
        pub container_forced: bool,
        /// Knobs set on the config that are **dead** in lossless mode
        /// (`"noise"`, `"distance_override"`, `"generic_quality"`).
        /// `validate()` rejects the noise case outright; the quality
        /// knobs are tolerated for generic zencodec pipelines that set
        /// quality before toggling lossless, and reported here instead.
        pub inert_knobs: Vec<&'static str>,
    }

    /// Mode-discriminated resolved encode plan — see
    /// [`JxlEncoderConfig::resolve_plan`].
    #[cfg(feature = "__expert")]
    #[derive(Clone, Debug)]
    pub enum JxlEncodePlan {
        /// Lossy VarDCT plan.
        Lossy(LossyPlan),
        /// Lossless modular plan.
        Lossless(LosslessPlan),
    }

    impl Default for JxlEncoderConfig {
        fn default() -> Self {
            Self::new()
        }
    }

    impl zencodec::encode::EncoderConfig for JxlEncoderConfig {
        // Envelope (Pattern B): a generic consumer recovers `ErrorCategory` +
        // codec name through `Dyn*` dispatch. None of this config's methods are
        // fallible, so only the associated type changes; `JxlError` (the detail
        // + category source) flows through the bridge when a fallible boundary
        // method on a downstream type errors.
        type Error = At<zencodec::CodecError>;
        type Job = JxlEncodeJob;

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

        /// Honor a [`Fidelity`](zencodec::encode::Fidelity) target as natively
        /// as JPEG XL allows.
        ///
        /// - `Lossless` → modular lossless (`with_lossless(true)`).
        /// - `Lossy(ApproxButteraugli(d))` → **native** VarDCT butteraugli
        ///   distance (`with_distance`, clamped to `0.0..=25.0`).
        /// - `Lossy(CodecSpecificQuality(q))` → the calibrated jxl quality dial.
        /// - `Lossy(ApproxSsim2(s))` → jxl has no native SSIM2 target, so the
        ///   score is mapped onto the quality dial; `resolved_target_fidelity`
        ///   reports it as `codec_quality`, honest that no SSIM2 convergence
        ///   happened.
        ///
        /// The quality arms clear any prior `with_distance` so the encode and
        /// the resolved report agree under chained calls.
        fn with_fidelity(self, fidelity: zencodec::encode::Fidelity) -> Self {
            use zencodec::encode::{Fidelity, LossyTarget};
            match fidelity {
                Fidelity::Lossless => self.with_lossless(true),
                Fidelity::Lossy(LossyTarget::ApproxButteraugli(distance)) => {
                    self.with_lossless(false).with_distance(distance)
                }
                Fidelity::Lossy(LossyTarget::CodecSpecificQuality(q)) => {
                    let mut s = self.with_lossless(false);
                    s.distance_override = None;
                    s.with_generic_quality(q)
                }
                Fidelity::Lossy(LossyTarget::ApproxSsim2(score)) => {
                    let mut s = self.with_lossless(false);
                    s.distance_override = None;
                    s.with_generic_quality(score)
                }
                // `Fidelity` / `LossyTarget` are `#[non_exhaustive]`.
                _ => self.with_lossless(false),
            }
        }

        /// Report what jxl resolved the fidelity to: lossless wins, then a
        /// native butteraugli distance, else the quality dial.
        fn resolved_target_fidelity(&self) -> Option<zencodec::encode::Fidelity> {
            use zencodec::encode::Fidelity;
            if self.lossless {
                return Some(Fidelity::Lossless);
            }
            if let Some(distance) = self.distance_override {
                return Some(Fidelity::butteraugli(distance));
            }
            self.generic_quality.map(Fidelity::codec_quality)
        }

        fn estimate_encode_resources(
            &self,
            image: &zencodec::estimate::ImageCharacteristics,
            compute: &zencodec::estimate::ComputeEnvironment,
        ) -> zencodec::estimate::ResourceEstimate {
            use zencodec::estimate::{ResourceEstimate, ThreadingInformation};
            // Read path + effort off the resolved mode — the same
            // `LossyConfig`/`LosslessConfig` the encode consumes (rebuilt by
            // every setter, so it already reflects quality→distance
            // calibration, effort clamping, and upstream defaults).
            let (is_lossless, effort) = match &self.mode {
                JxlEncMode::Lossy(c) => (false, c.effort()),
                JxlEncMode::Lossless(c) => (true, c.effort()),
            };
            let descriptor = image.descriptor();
            let input_bpp = descriptor.bytes_per_pixel() as u8;
            let has_alpha = descriptor.has_alpha();
            match jxl_encoder::heuristics::estimate_encode(
                image.width(),
                image.height(),
                input_bpp,
                has_alpha,
                is_lossless,
                effort,
            ) {
                Some(e) => {
                    let ti = jxl_encoder::heuristics::encode_threading_info(is_lossless, effort);
                    let threading = if ti.parallel {
                        ThreadingInformation::parallel(ti.max_useful_threads)
                    } else {
                        ThreadingInformation::SERIAL
                    };
                    ResourceEstimate::new(e.peak_memory_bytes, e.time_ms as u64)
                        .with_peak_max(e.peak_memory_bytes_max)
                        .with_threading(threading)
                        .at_cores(compute.cores())
                }
                None => ResourceEstimate::conservative(image).at_cores(compute.cores()),
            }
        }

        fn job(self) -> JxlEncodeJob {
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
    pub struct JxlEncodeJob {
        config: JxlEncoderConfig,
        stop: Option<zencodec::StopToken>,
        limits: Option<ResourceLimits>,
        metadata: Option<Metadata>,
        policy: EncodePolicy,
        loop_count: Option<u32>,
    }

    impl zencodec::encode::EncodeJob for JxlEncodeJob {
        type Error = At<zencodec::CodecError>;
        type Enc = JxlEncoder;
        type AnimationFrameEnc = JxlAnimationFrameEncoder;

        fn with_stop(mut self, stop: zencodec::StopToken) -> Self {
            self.stop = Some(stop);
            self
        }

        fn with_limits(mut self, limits: ResourceLimits) -> Self {
            self.limits = Some(limits);
            self
        }

        fn with_metadata(mut self, meta: Metadata) -> Self {
            self.metadata = Some(meta);
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

        fn encoder(self) -> Result<JxlEncoder, At<zencodec::CodecError>> {
            // Infallible: no `JxlError` is produced, so the `At<CodecError>`
            // return type is never instantiated here.
            let mode = apply_threads(&self.config.mode, &self.limits);
            Ok(JxlEncoder {
                mode,
                metadata: self.metadata,
                policy: self.policy,
                limits: self.limits,
                stop: self.stop,
                stream: StreamState::Empty,
                gain_map: self.config.gain_map.clone(),
            })
        }

        fn animation_frame_encoder(
            self,
        ) -> Result<JxlAnimationFrameEncoder, At<zencodec::CodecError>> {
            let mode = apply_threads(&self.config.mode, &self.limits);
            Ok(JxlAnimationFrameEncoder::from_job(
                mode,
                self.metadata.as_ref(),
                &self.policy,
                self.limits,
                self.loop_count,
                self.config.gain_map.clone(),
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
    pub struct JxlEncoder {
        mode: JxlEncMode,
        metadata: Option<Metadata>,
        policy: EncodePolicy,
        limits: Option<ResourceLimits>,
        stop: Option<zencodec::StopToken>,
        stream: StreamState,
        gain_map: Option<Arc<GainMapData>>,
    }

    impl JxlEncoder {
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
                _ => Err(whereat::at!(JxlError::UnsupportedOperation(
                    UnsupportedOperation::PixelFormat,
                ))),
            }
        }
    }

    impl JxlEncoder {
        /// Build jxl-encoder ImageMetadata from the zencodec Metadata.
        ///
        /// `embed_icc` is resolved by [`resolve_jxl_color`](Self::resolve_jxl_color)
        /// — it is `false` when the color is carried by the codestream enum
        /// encoding and the ICC was a redundant duplicate (the JXL Balanced
        /// default). EXIF/XMP still follow the [`EncodePolicy`].
        fn build_jxl_metadata(&self, embed_icc: bool) -> Option<jxl_encoder::ImageMetadata<'_>> {
            let meta = self.metadata.as_ref()?;
            let mut jxl_meta = jxl_encoder::ImageMetadata::new();
            let mut has_any = false;

            if embed_icc && let Some(ref icc) = meta.icc_profile {
                jxl_meta = jxl_meta.with_icc_profile(icc);
                has_any = true;
            }
            if self.policy.resolve_exif(true)
                && let Some(ref exif) = meta.exif
            {
                jxl_meta = jxl_meta.with_exif(exif);
                has_any = true;
            }
            if self.policy.resolve_xmp(true)
                && let Some(ref xmp) = meta.xmp
            {
                jxl_meta = jxl_meta.with_xmp(xmp);
                has_any = true;
            }

            // HDR: forward the content peak (MaxCLL) as JXL's
            // `intensity_target` — the symmetric inverse of the decode-side
            // `intensity_target → MaxCLL` mapping, which was otherwise dropped
            // on encode so HDR peak signaling did not round-trip. (The
            // `diffuse_white` anchor is the *linear* reference used by
            // `quantize_to`; JXL signals the *peak*, not SDR white, so the
            // peak is what flows here.)
            if let Some(cll) = meta.content_light_level
                && cll.max_content_light_level > 0
            {
                jxl_meta = jxl_meta.with_intensity_target(f32::from(cll.max_content_light_level));
                has_any = true;
            }

            has_any.then_some(jxl_meta)
        }

        /// Resolve JXL color emission: whether to drive the codestream enum
        /// color encoding from `Metadata::cicp` (or an ICC that maps to CICP),
        /// and whether the ICC is then a redundant duplicate to drop.
        ///
        /// Routes through [`zencodec::resolve_color_emit`] under the caller's
        /// color preset (`self.policy.resolve_color`, default
        /// [`Balanced`](zencodec::ColorEmitPolicy::Balanced)). JXL is
        /// `cicp_safe_sole_carrier`, so a representable color drops the ICC
        /// (matching libjxl's `want_icc=false`). Returns the enum encoding to
        /// apply (if any) and whether to embed the ICC.
        fn resolve_jxl_color(
            &self,
            layout: PixelLayout,
        ) -> (Option<jxl_encoder::ColorEncoding>, bool) {
            let policy_icc = self.policy.resolve_icc(true);
            let Some(meta) = self.metadata.as_ref() else {
                return (None, policy_icc);
            };
            let channel_count = if Self::layout_is_gray(layout) { 1 } else { 3 };
            let mut src = zencodec::SourceColor::default().with_channel_count(channel_count);
            if let Some(cicp) = meta.cicp {
                src = src.with_cicp(cicp);
            }
            if let Some(ref icc) = meta.icc_profile {
                src = src.with_icc_profile(icc.clone());
            }
            let plan = zencodec::resolve_color_emit(
                &src,
                &JXL_ENCODE_CAPS,
                self.policy
                    .resolve_color(zencodec::ColorEmitPolicy::Balanced),
            );
            let color_encoding = plan
                .cicp
                .as_ref()
                .and_then(Self::cicp_to_jxl_color_encoding);
            // Drop the ICC only when the enum color actually carries the color
            // *and* the plan judged it redundant.
            let embed_icc = policy_icc
                && !(color_encoding.is_some()
                    && matches!(plan.icc, zencodec::IccDisposition::Drop));
            (color_encoding, embed_icc)
        }

        /// Whether a pixel layout is grayscale (CICP is RGB-centric and must be
        /// suppressed for gray so [`resolve_jxl_color`](Self::resolve_jxl_color)
        /// keeps the ICC instead of emitting an RGB color description).
        fn layout_is_gray(layout: PixelLayout) -> bool {
            matches!(
                layout,
                PixelLayout::Gray8
                    | PixelLayout::Gray16
                    | PixelLayout::GrayLinearF32
                    | PixelLayout::GrayLinearF16
                    | PixelLayout::GrayAlpha8
                    | PixelLayout::GrayAlpha16
                    | PixelLayout::GrayAlphaLinearF32
                    | PixelLayout::GrayAlphaLinearF16
            )
        }

        /// Map a CICP color description to a jxl-encoder enum [`ColorEncoding`],
        /// when the primaries and transfer are both expressible in JXL's enums
        /// (else `None` → keep the ICC). Rendering intent defaults to Perceptual
        /// (CICP carries none); `want_icc = false` (the enum is authoritative).
        fn cicp_to_jxl_color_encoding(cicp: &zencodec::Cicp) -> Option<jxl_encoder::ColorEncoding> {
            use jxl_encoder::headers::color_encoding::{
                ColorEncoding, ColorSpace, Primaries, RenderingIntent, TransferFunction, WhitePoint,
            };
            let primaries = match cicp.color_primaries {
                1 => Primaries::Srgb,
                9 => Primaries::Bt2100,
                11 | 12 => Primaries::P3,
                _ => return None,
            };
            let transfer_function = match cicp.transfer_characteristics {
                1 => TransferFunction::Bt709,
                8 => TransferFunction::Linear,
                13 => TransferFunction::Srgb,
                16 => TransferFunction::Pq,
                17 => TransferFunction::Dci,
                18 => TransferFunction::Hlg,
                _ => return None,
            };
            // CICP primaries 11 = DCI-P3 (DCI white); 12 = Display-P3 (D65).
            let white_point = if cicp.color_primaries == 11 {
                WhitePoint::Dci
            } else {
                WhitePoint::D65
            };
            Some(ColorEncoding {
                color_space: ColorSpace::Rgb,
                white_point,
                custom_white_point: None,
                primaries,
                custom_primaries: None,
                transfer_function,
                rendering_intent: RenderingIntent::Perceptual,
                want_icc: false,
                gamma: None,
            })
        }

        /// Check ResourceLimits against the given dimensions and bytes-per-pixel.
        fn check_limits(&self, width: u32, height: u32, bpp: u32) -> Result<(), At<JxlError>> {
            if let Some(ref limits) = self.limits {
                limits
                    .check_dimensions(width, height)
                    .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                let estimated = width as u64 * height as u64 * bpp as u64;
                limits
                    .check_memory(estimated)
                    .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
            }
            Ok(())
        }

        /// Check encoded output size against `max_output_bytes`.
        fn check_encoded_output_size(&self, encoded: &[u8]) -> Result<(), At<JxlError>> {
            if let Some(ref limits) = self.limits {
                limits
                    .check_output_size(encoded.len() as u64)
                    .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
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
            let (color_encoding, embed_icc) = self.resolve_jxl_color(layout);
            let jxl_meta = self.build_jxl_metadata(embed_icc);

            // Thread the caller's memory budget into the encoder. Without this
            // the encoder applies its default ~2 GiB budget, which large HDR
            // frames (≥12 MP at 16-bit) blow through (`memory budget exceeded`).
            // `ResourceLimits::max_memory_bytes` (e.g. set high for a trusted
            // batch sweep) → `jxl_encoder::Limits`.
            let jxl_limits = self
                .limits
                .as_ref()
                .and_then(|l| l.max_memory_bytes)
                .map(|b| jxl_encoder::Limits::default().with_max_memory_bytes(b));

            let encode = |req: jxl_encoder::EncodeRequest<'_>| -> Result<Vec<u8>, At<JxlError>> {
                let req = if let Some(ref ce) = color_encoding {
                    req.with_color_encoding(ce.clone())
                } else {
                    req
                };
                let req = if let Some(ref meta) = jxl_meta {
                    req.with_metadata(meta)
                } else {
                    req
                };
                let req = if let Some(ref lim) = jxl_limits {
                    req.with_limits(lim)
                } else {
                    req
                };
                let req = if let Some(ref stop) = self.stop {
                    req.with_stop(stop)
                } else {
                    req
                };
                req.encode(data).map_err_at(JxlError::Encode)
            };

            match &self.mode {
                JxlEncMode::Lossy(cfg) => encode(cfg.encode_request(width, height, layout)),
                JxlEncMode::Lossless(cfg) => encode(cfg.encode_request(width, height, layout)),
            }
        }

        /// If a gain map is configured, wrap the encoded JXL with a `jhgm` box.
        ///
        /// Bare codestreams are wrapped in a container first. Container-format
        /// output gets the jhgm box appended.
        fn maybe_attach_gain_map(&self, encoded: Vec<u8>) -> Vec<u8> {
            match &self.gain_map {
                Some(gm) => jxl_encoder::container::append_gain_map_box(&encoded, &gm.jhgm_payload),
                None => encoded,
            }
        }
    }

    impl zencodec::encode::Encoder for JxlEncoder {
        type Error = At<zencodec::CodecError>;

        fn reject(op: UnsupportedOperation) -> At<zencodec::CodecError> {
            // Bridge a bare native value into the envelope (see
            // `From<JxlError> for At<CodecError>`).
            JxlError::from(op).into()
        }

        fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<zencodec::CodecError>> {
            // The fallible body stays `At<JxlError>` (internal `?` sites
            // untouched); convert once at the boundary, preserving the trace.
            (move || -> Result<EncodeOutput, At<JxlError>> {
                use zenpixels::PixelFormat;

                let width = pixels.width();
                let height = pixels.rows();

                // Rgbx8 / Bgrx8: byte 3 is undefined padding — strip to 3-channel RGB
                // so the encoder doesn't treat the padding as alpha.
                let pf = pixels.descriptor().pixel_format();
                if matches!(pf, PixelFormat::Rgbx8 | PixelFormat::Bgrx8) {
                    self.check_limits(width, height, 3)?;
                    let raw = pixels.contiguous_bytes();
                    let mut rgb =
                        alloc::vec::Vec::with_capacity((width as usize) * (height as usize) * 3);
                    if matches!(pf, PixelFormat::Rgbx8) {
                        for px in raw.chunks_exact(4) {
                            rgb.extend_from_slice(&[px[0], px[1], px[2]]);
                        }
                    } else {
                        // Bgrx8: swap B↔R while stripping.
                        for px in raw.chunks_exact(4) {
                            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
                        }
                    }
                    let encoded =
                        self.encode_with_metadata(&rgb, width, height, PixelLayout::Rgb8)?;
                    let encoded = self.maybe_attach_gain_map(encoded);
                    self.check_encoded_output_size(&encoded)?;
                    return Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                        .with_mime_type("image/jxl")
                        .with_extension("jxl"));
                }

                let layout = Self::descriptor_to_layout(pixels.descriptor())?;
                let bpp = pixels.descriptor().bytes_per_pixel() as u32;
                self.check_limits(width, height, bpp)?;

                let data = pixels.contiguous_bytes();
                let encoded = self.encode_with_metadata(&data, width, height, layout)?;
                let encoded = self.maybe_attach_gain_map(encoded);
                self.check_encoded_output_size(&encoded)?;

                Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                    .with_mime_type("image/jxl")
                    .with_extension("jxl"))
            })()
            .map_err(zencodec::CodecError::of)
        }

        fn encode_srgba8(
            self,
            data: &mut [u8],
            make_opaque: bool,
            width: u32,
            height: u32,
            stride_pixels: u32,
        ) -> Result<EncodeOutput, At<zencodec::CodecError>> {
            (move || -> Result<EncodeOutput, At<JxlError>> {
                let w = width as usize;
                let h = height as usize;
                let stride = stride_pixels as usize;

                if make_opaque {
                    // Encode as RGB — strip alpha entirely for smaller output.
                    self.check_limits(width, height, 3)?;
                    // check_limits already gates dimensions, but use checked_mul
                    // here so a degenerate width/height pair can't overflow the
                    // capacity computation on its way to a panic.
                    let rgb_capacity =
                        w.checked_mul(h)
                            .and_then(|v| v.checked_mul(3))
                            .ok_or_else(|| {
                                whereat::at!(JxlError::LimitExceeded(
                                    "RGB capacity overflow".into(),
                                ))
                            })?;
                    let mut rgb = Vec::with_capacity(rgb_capacity);
                    for y in 0..h {
                        let row_start = y * stride * 4;
                        let row = &data[row_start..row_start + w * 4];
                        for px in row.chunks_exact(4) {
                            rgb.push(px[0]);
                            rgb.push(px[1]);
                            rgb.push(px[2]);
                        }
                    }
                    let encoded =
                        self.encode_with_metadata(&rgb, width, height, PixelLayout::Rgb8)?;
                    let encoded = self.maybe_attach_gain_map(encoded);
                    self.check_encoded_output_size(&encoded)?;
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
                    let encoded = self.maybe_attach_gain_map(encoded);
                    self.check_encoded_output_size(&encoded)?;
                    Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                        .with_mime_type("image/jxl")
                        .with_extension("jxl"))
                }
            })()
            .map_err(zencodec::CodecError::of)
        }

        fn push_rows(&mut self, rows: PixelSlice<'_>) -> Result<(), At<zencodec::CodecError>> {
            (move || -> Result<(), At<JxlError>> {
                use zenpixels::PixelFormat;

                let desc = rows.descriptor();
                let width = rows.width();
                let num_rows = rows.rows();

                // Rgbx8 / Bgrx8: strip padding byte and coerce to Rgb8 in the
                // accumulated buffer, then treat the stream as Rgb8 internally.
                let pf = desc.pixel_format();
                let (layout, bytes_cow): (PixelLayout, alloc::borrow::Cow<'_, [u8]>) =
                    if matches!(pf, PixelFormat::Rgbx8 | PixelFormat::Bgrx8) {
                        let raw = rows.contiguous_bytes();
                        let mut rgb = alloc::vec::Vec::with_capacity(
                            (width as usize) * (num_rows as usize) * 3,
                        );
                        if matches!(pf, PixelFormat::Rgbx8) {
                            for px in raw.chunks_exact(4) {
                                rgb.extend_from_slice(&[px[0], px[1], px[2]]);
                            }
                        } else {
                            for px in raw.chunks_exact(4) {
                                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
                            }
                        }
                        (PixelLayout::Rgb8, alloc::borrow::Cow::Owned(rgb))
                    } else {
                        (Self::descriptor_to_layout(desc)?, rows.contiguous_bytes())
                    };
                let bytes: &[u8] = &bytes_cow;

                match &mut self.stream {
                    StreamState::Empty => {
                        let mut data = Vec::new();
                        data.extend_from_slice(bytes);
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
                            return Err(whereat::at!(JxlError::InvalidInput(
                                "push_rows: width or pixel format changed between calls".into(),
                            )));
                        }
                        data.extend_from_slice(bytes);
                        *rows_pushed += num_rows;
                    }
                }
                Ok(())
            })()
            .map_err(zencodec::CodecError::of)
        }

        fn finish(self) -> Result<EncodeOutput, At<zencodec::CodecError>> {
            (move || -> Result<EncodeOutput, At<JxlError>> {
                let StreamState::Accumulating {
                    width,
                    layout,
                    descriptor,
                    ref data,
                    rows_pushed,
                    ..
                } = self.stream
                else {
                    return Err(whereat::at!(JxlError::InvalidInput(
                        "finish: no rows were pushed".into(),
                    )));
                };

                let bpp = descriptor.bytes_per_pixel() as u32;
                self.check_limits(width, rows_pushed, bpp)?;

                let encoded = self.encode_with_metadata(data, width, rows_pushed, layout)?;
                let encoded = self.maybe_attach_gain_map(encoded);
                self.check_encoded_output_size(&encoded)?;

                Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                    .with_mime_type("image/jxl")
                    .with_extension("jxl"))
            })()
            .map_err(zencodec::CodecError::of)
        }
    }

    // ── JxlAnimationFrameEncoder ──────────────────────────────────────────────

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
    pub struct JxlAnimationFrameEncoder {
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
        gain_map: Option<Arc<GainMapData>>,
    }

    impl JxlAnimationFrameEncoder {
        /// Create from job state, copying metadata we need for container wrapping.
        fn from_job(
            mode: JxlEncMode,
            metadata: Option<&Metadata>,
            policy: &EncodePolicy,
            limits: Option<ResourceLimits>,
            loop_count: Option<u32>,
            gain_map: Option<Arc<GainMapData>>,
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
                gain_map,
            }
        }

        /// Wrap an encoded animation codestream with EXIF/XMP metadata boxes
        /// and an optional gain map box.
        fn wrap_with_metadata_and_gain_map(&self, codestream: Vec<u8>) -> Vec<u8> {
            let has_meta = self
                .anim_meta
                .as_ref()
                .is_some_and(|m| m.exif.is_some() || m.xmp.is_some());
            let has_gain_map = self.gain_map.is_some();

            if !has_meta && !has_gain_map {
                return codestream;
            }

            // Always need container format if we have metadata or gain map.
            let exif = self.anim_meta.as_ref().and_then(|m| m.exif.as_deref());
            let xmp = self.anim_meta.as_ref().and_then(|m| m.xmp.as_deref());

            let wrapped = if has_meta {
                jxl_encoder::container::wrap_in_container(&codestream, exif, xmp)
            } else {
                codestream
            };

            match &self.gain_map {
                Some(gm) => jxl_encoder::container::append_gain_map_box(&wrapped, &gm.jhgm_payload),
                None => wrapped,
            }
        }
    }

    impl zencodec::encode::AnimationFrameEncoder for JxlAnimationFrameEncoder {
        type Error = At<zencodec::CodecError>;

        fn reject(op: UnsupportedOperation) -> At<zencodec::CodecError> {
            JxlError::from(op).into()
        }

        fn push_frame(
            &mut self,
            pixels: PixelSlice<'_>,
            duration_ms: u32,
            stop: Option<&dyn Stop>,
        ) -> Result<(), At<zencodec::CodecError>> {
            (move || -> Result<(), At<JxlError>> {
                use zenpixels::PixelFormat;

                // Check cancellation before doing any work.
                if let Some(stop) = stop {
                    stop.check()
                        .map_err(|e| whereat::at!(JxlError::Cancelled(e)))?;
                }

                let desc = pixels.descriptor();
                let pf = desc.pixel_format();
                let strip_padding = matches!(pf, PixelFormat::Rgbx8 | PixelFormat::Bgrx8);
                let layout = if strip_padding {
                    PixelLayout::Rgb8
                } else {
                    JxlEncoder::descriptor_to_layout(desc)?
                };
                let w = pixels.width();
                let h = pixels.rows();

                if self.pixel_data.is_empty() {
                    // Validate dimensions against limits on first frame.
                    if let Some(ref limits) = self.limits {
                        limits
                            .check_dimensions(w, h)
                            .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                    }
                    self.width = w;
                    self.height = h;
                    self.layout = Some(layout);
                } else if w != self.width || h != self.height {
                    return Err(whereat::at!(JxlError::InvalidInput(
                        "animation frame dimensions must match first frame".into(),
                    )));
                }

                // Check max_frames limit.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_frames(self.pixel_data.len() as u32 + 1)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }

                let frame_data = if strip_padding {
                    let raw = pixels.contiguous_bytes();
                    let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
                    if matches!(pf, PixelFormat::Rgbx8) {
                        for px in raw.chunks_exact(4) {
                            rgb.extend_from_slice(&[px[0], px[1], px[2]]);
                        }
                    } else {
                        for px in raw.chunks_exact(4) {
                            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
                        }
                    }
                    rgb
                } else {
                    pixels.contiguous_bytes().into_owned()
                };
                let frame_bytes = frame_data.len() as u64;
                self.accumulated_bytes += frame_bytes;

                // Check accumulated memory across ALL frames, not just the first.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_memory(self.accumulated_bytes)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }

                self.pixel_data.push(frame_data);
                self.frames.push(duration_ms);
                Ok(())
            })()
            .map_err(zencodec::CodecError::of)
        }

        fn finish(self, stop: Option<&dyn Stop>) -> Result<EncodeOutput, At<zencodec::CodecError>> {
            (move || -> Result<EncodeOutput, At<JxlError>> {
                // Check cancellation before expensive encode.
                if let Some(stop) = stop {
                    stop.check()
                        .map_err(|e| whereat::at!(JxlError::Cancelled(e)))?;
                }

                let layout = self.layout.ok_or_else(|| {
                    whereat::at!(JxlError::InvalidInput("no frames pushed".into()))
                })?;

                let animation = AnimationParams {
                    tps_numerator: 1000,
                    tps_denominator: 1,
                    num_loops: self.loop_count.unwrap_or(0),
                    // jxl-encoder 0.3.2 (W12-4 a1-audit chunk-1) added
                    // associated/premultiplied-alpha signalling on the
                    // animation path. zenjxl's high-level wrapper takes
                    // straight-alpha pixels, matching the new default.
                    ..AnimationParams::default()
                };

                let anim_frames: Vec<AnimationFrame<'_>> = self
                    .pixel_data
                    .iter()
                    .zip(&self.frames)
                    // jxl-encoder 0.3.2 expanded AnimationFrame with
                    // blend_mode / blend_source / save_as_reference /
                    // reference_only / name / timecode for multi-layer
                    // animation API parity with libjxl frame headers
                    // (jxl-encoder commit d0e47838). zenjxl's wrapper
                    // produces single-pass replace-blend frames, so
                    // `AnimationFrame::new` is the right constructor —
                    // it leaves every optional field at the encoder
                    // default (= the prior `pixels + duration`-only
                    // behaviour).
                    .map(|(data, &duration)| AnimationFrame::new(data, duration))
                    .collect();

                let encoded = match &self.mode {
                    JxlEncMode::Lossy(cfg) => cfg
                        .encode_animation(self.width, self.height, layout, &animation, &anim_frames)
                        .map_err_at(JxlError::Encode)?,
                    JxlEncMode::Lossless(cfg) => cfg
                        .encode_animation(self.width, self.height, layout, &animation, &anim_frames)
                        .map_err_at(JxlError::Encode)?,
                };

                let encoded = self.wrap_with_metadata_and_gain_map(encoded);

                Ok(EncodeOutput::new(encoded, ImageFormat::Jxl)
                    .with_mime_type("image/jxl")
                    .with_extension("jxl"))
            })()
            .map_err(zencodec::CodecError::of)
        }
    }
}

// ── Decoding ────────────────────────────────────────────────────────────────

#[cfg(feature = "decode")]
mod decoding {
    use super::*;
    use alloc::borrow::Cow;
    use alloc::collections::VecDeque;
    use alloc::vec::Vec;

    use alloc::string::ToString;

    use jxl::api::{
        ExtraChannel, JxlDecoder as JxlRsDecoder, JxlDecoderOptions, JxlOutputBuffer,
        ProcessingResult,
    };
    use zencodec::Unsupported;
    use zencodec::decode::{DecodeCapabilities, DecodeOutput, DecodePolicy, OutputInfo, SinkError};
    use zencodec::{AnimationFrame, OwnedAnimationFrame};
    use zencodec::{
        ContentLightLevel, ImageInfo, Orientation, OrientationHint, ResourceLimits,
        UnsupportedOperation,
    };
    use zenpixels::{Cicp, ColorAuthority, PixelBuffer};

    use enough::Stop;

    use crate::decode::{
        JxlInfo, JxlLimits, build_pixel_data, choose_pixel_format, decode_with_options_oriented,
        extract_color_info, is_hdr_or_wide_gamut, map_err, probe, probe_with_orientation,
    };

    /// Determine the decoder `parallel` flag from limits.
    fn policy_to_parallel(limits: &Option<ResourceLimits>) -> Option<bool> {
        limits.as_ref().map(|l| l.threading().is_parallel())
    }

    /// Convert a jxl-rs [`GainMapBundle`] into a zencodec [`GainMapSource`].
    ///
    /// Parses the ISO 21496-1 binary metadata and probes the gain map
    /// codestream for dimensions. Falls back to default metadata / zero
    /// dimensions when parsing or probing fails — the raw codestream is
    /// still preserved for downstream decode.
    fn bundle_to_gain_map_source(
        bundle: jxl::api::GainMapBundle,
    ) -> zencodec::gainmap::GainMapSource {
        use zencodec::gainmap::{GainMapInfo, GainMapSource};

        // Parse ISO 21496-1 metadata; fall back to defaults on failure.
        // JXL jhgm bundles store raw GainMapMetadata (no version byte prefix).
        // The version byte is part of the AVIF ToneMapImage envelope, not the
        // metadata itself. `JxlJhgm` is the canonical name for these bytes
        // (same payload as the deprecated `JpegApp2` variant).
        let params = zencodec::gainmap::parse_iso21496_fmt(
            &bundle.metadata,
            zencodec::gainmap::Iso21496Format::JxlJhgm,
        )
        .unwrap_or_default();

        // Probe the bare JXL codestream to get gain map image dimensions.
        let (width, height, channels) = if let Ok(gm_info) = probe(&bundle.gain_map_codestream) {
            let ch = if gm_info.is_gray { 1u8 } else { 3u8 };
            (gm_info.width, gm_info.height, ch)
        } else {
            // Codestream too short or invalid — dimensions unknown.
            (0, 0, 1)
        };

        let metadata = GainMapInfo::new(params, width, height, channels);
        GainMapSource::new(bundle.gain_map_codestream, ImageFormat::Jxl, metadata)
    }

    /// Apply a jhgm gain map to the decoded base (`GainMapRender::ReconstructHdr`).
    ///
    /// jhgm bundles come in two directions (ISO 21496-1 headrooms decide):
    /// the JXL-typical HDR-base bundle (alternate is SDR) needs nothing — the
    /// decoded base already IS the HDR rendition with its own signaling — while
    /// an SDR-base bundle (Adobe-style) gets the gain map applied via
    /// ultrahdr-core into linear f32 (or f16 when preferred) RGBA.
    /// `None` target reconstructs at the gain map's encoded maximum
    /// (`alternate_hdr_headroom` is log2 of the alternate/SDR peak ratio).
    /// Malformed metadata or an unsupported gain-map form is an error — the
    /// caller asked for an HDR rendition, and silently returning SDR would
    /// misrepresent the image.
    #[cfg(feature = "reconstruct-hdr")]
    #[allow(clippy::too_many_arguments)]
    fn reconstruct_hdr_base(
        base: zenpixels::PixelBuffer,
        mut info: ImageInfo,
        bundle: &jxl::api::GainMapBundle,
        target_headroom: Option<f32>,
        preferred: &[PixelDescriptor],
        native_limits: Option<&JxlLimits>,
        parallel: Option<bool>,
        stop: &dyn Stop,
        alloc_pref: zencodec::AllocPreference,
    ) -> Result<(zenpixels::PixelBuffer, ImageInfo), At<JxlError>> {
        use ultrahdr_core::gainmap::{HdrOutputFormat, apply_gainmap};

        /// SDR reference white (cd/m²) — 1.0 in the linear output maps here.
        const SDR_WHITE_NITS: f32 = 203.0;

        let params = zencodec::gainmap::parse_iso21496_fmt(
            &bundle.metadata,
            zencodec::gainmap::Iso21496Format::JxlJhgm,
        )
        .map_err(|_| {
            whereat::at!(JxlError::InvalidInput(
                "ReconstructHdr: jhgm ISO 21496-1 metadata failed to parse".into()
            ))
        })?;

        if params.direction() == zencodec::gainmap::GainMapDirection::BaseIsHdr {
            // The base image IS the HDR rendition (alternate is SDR) —
            // nothing to apply; the base carries its own HDR signaling.
            return Ok((base, info));
        }

        // SDR-base bundle: decode the gain-map codestream (same resource
        // limits + allocation preference as the base decode) and apply it.
        // `adjust_orientation = true`, `reject_progressive = false` matches
        // `decode_with_options`.
        let gm_result = decode_with_options_oriented(
            &bundle.gain_map_codestream,
            native_limits,
            &[],
            parallel,
            None,
            true,
            false,
            alloc_pref,
        )?;
        let gm_pixels = gm_result.pixels;
        let channels = match gm_pixels.descriptor().pixel_format() {
            zenpixels::PixelFormat::Gray8 => 1u8,
            zenpixels::PixelFormat::Rgb8 => 3u8,
            _ => {
                return Err(whereat::at!(JxlError::InvalidInput(
                    "ReconstructHdr: gain-map codestream must decode to gray8 or rgb8".into()
                )));
            }
        };
        let gm = ultrahdr_core::GainMap {
            width: gm_pixels.width(),
            height: gm_pixels.height(),
            channels,
            data: gm_pixels.as_slice().contiguous_bytes().into_owned(),
        };

        // Output form: honor an f16 preference; default linear f32 RGBA.
        let wants_f16 = preferred
            .iter()
            .any(|d| d.channel_type() == zenpixels::ChannelType::F16);
        let format = if wants_f16 {
            HdrOutputFormat::LinearF16
        } else {
            HdrOutputFormat::LinearFloat
        };

        // `None` = full reconstruction at the gain map's encoded maximum.
        let capacity_max = params.linear_alternate_headroom() as f32;
        let display_boost = target_headroom.unwrap_or(capacity_max).max(1.0);

        let hdr = apply_gainmap(&base, &gm, &params, display_boost, format, stop).map_err(|e| {
            whereat::at!(JxlError::InvalidInput(alloc::format!(
                "ReconstructHdr: gain-map apply failed: {e}"
            )))
        })?;

        // Envelope: derived peak (capped at the reconstruction boost) +
        // mastering display matching the base image's primaries
        // (`apply_gainmap` preserves them).
        let peak_nits = SDR_WHITE_NITS * capacity_max.min(display_boost);
        let primaries = match info.source_color.cicp.map(|c| c.color_primaries) {
            Some(12) => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]], // Display P3
            Some(9) => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],  // BT.2020
            _ => [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]],        // BT.709/sRGB
        };
        info.source_color.content_light_level =
            Some(zencodec::ContentLightLevel::new(peak_nits as u16, 0));
        info.source_color.mastering_display = Some(zencodec::MasteringDisplay::new(
            primaries,
            [0.3127, 0.3290],
            peak_nits,
            0.005,
        ));
        Ok((hdr, info))
    }

    // ── Capabilities ────────────────────────────────────────────────────

    static JXL_DECODE_CAPS: DecodeCapabilities = {
        let caps = DecodeCapabilities::new()
            .with_icc(true)
            .with_cicp(true)
            .with_hdr(true)
            .with_exif(true)
            .with_xmp(true)
            .with_gain_map(true)
            .with_native_gray(true)
            .with_native_16bit(true)
            .with_native_f32(true)
            .with_native_alpha(true)
            .with_animation(true)
            .with_cheap_probe(true)
            .with_enforces_max_pixels(true)
            .with_enforces_max_memory(true)
            .with_enforces_max_input_bytes(true)
            .with_stop(true)
            .with_threads_supported_range(
                1,
                if cfg!(feature = "threads") {
                    u16::MAX
                } else {
                    1
                },
            );
        #[cfg(feature = "reconstruct-hdr")]
        let caps = caps.with_reconstructs_hdr(true);
        caps
    };

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

        /// Fail-fast validation of the configured decoder parameters.
        ///
        /// Currently a no-op: [`JxlDecoderConfig`] has no tunable fields
        /// — all decode policy lives on [`JxlDecodeJob`] and is set per-call.
        /// The method exists so callers can write the same
        /// `cfg.validate()?` pattern across encoder and decoder configs in
        /// generic batch code.
        pub fn validate(&self) -> Result<(), crate::ValidationError> {
            Ok(())
        }
    }

    impl zencodec::decode::DecoderConfig for JxlDecoderConfig {
        // Envelope (Pattern B) — see the `EncoderConfig` impl. No fallible
        // methods here; the bridge carries `JxlError` at the `DecodeJob`
        // boundaries.
        type Error = At<zencodec::CodecError>;
        type Job<'a> = JxlDecodeJob;

        fn formats() -> &'static [ImageFormat] {
            &[ImageFormat::Jxl]
        }

        fn supported_descriptors() -> &'static [PixelDescriptor] {
            JXL_DECODE_DESCRIPTORS
        }

        fn capabilities() -> &'static DecodeCapabilities {
            &JXL_DECODE_CAPS
        }

        fn estimate_decode_resources(
            &self,
            image: &zencodec::estimate::ImageCharacteristics,
            compute: &zencodec::estimate::ComputeEnvironment,
        ) -> zencodec::estimate::ResourceEstimate {
            use zencodec::estimate::{ResourceEstimate, ThreadingInformation};
            // jxl-encoder ships a calibrated *encode* heuristic; the
            // zenjxl-decoder dependency exposes no decode estimate, so model
            // it here (output buffer + VarDCT/modular working set + fixed
            // overhead). Reported SERIAL: jxl-rs can thread, but the wrapper
            // does not characterize a scaling knee, so the estimate does not
            // promise core scaling.
            let out_bpp = image.descriptor().bytes_per_pixel() as u8;
            match estimate_jxl_decode(image.width(), image.height(), out_bpp) {
                Some((peak, wall_ms)) => ResourceEstimate::new(peak, wall_ms)
                    .with_threading(ThreadingInformation::SERIAL)
                    .at_cores(compute.cores()),
                None => ResourceEstimate::conservative(image).at_cores(compute.cores()),
            }
        }

        fn job<'a>(self) -> Self::Job<'a> {
            JxlDecodeJob {
                limits: None,
                policy: DecodePolicy::none(),
                stop: None,
                start_frame_index: 0,
                extract_gain_map: false,
                gain_map_render: zencodec::GainMapRender::default(),
                orientation: OrientationHint::Preserve,
            }
        }
    }

    /// Estimate `(peak_memory_bytes, wall_ms)` for a JXL decode of a
    /// `width × height` image producing `out_bpp` bytes per output pixel.
    ///
    /// Returns `None` only on dimension overflow (a forged header), in which
    /// case the caller falls back to
    /// [`ResourceEstimate::conservative`](zencodec::estimate::ResourceEstimate::conservative).
    ///
    /// Model (a rough envelope, not a measured calibration — JXL decode peak
    /// is content-dependent on the VarDCT vs modular path):
    /// * **peak** = output buffer (`W·H·out_bpp`) + a VarDCT/modular working
    ///   set + a fixed entropy/context overhead. The working set is taken as
    ///   `WORKING_BYTES_PER_PX` per pixel — the decoder carries the image as
    ///   internal f32 planes (≈ 3·4 B/px for XYB/RGB) plus LF/HF coefficient
    ///   and upsampling buffers, the largest live set across the passes.
    /// * **wall_ms** = pixels / throughput, at a conservative
    ///   `DECODE_MPIX_PER_S` (JXL decode is markedly slower per pixel than
    ///   PNG/JPEG).
    #[must_use]
    fn estimate_jxl_decode(width: u32, height: u32, out_bpp: u8) -> Option<(u64, u64)> {
        // Internal working set beyond the output buffer: f32 color planes plus
        // coefficient / upsampling scratch — the dominant live allocation on
        // the VarDCT path.
        const WORKING_BYTES_PER_PX: u64 = 24;
        // Entropy tables, context model, frame header scratch — size-independent.
        const FIXED_OVERHEAD_BYTES: u64 = 16 * 1024 * 1024;
        // Conservative JXL decode throughput (megapixels / second).
        const DECODE_MPIX_PER_S: f64 = 60.0;

        let pixels = (width as u64).checked_mul(height as u64)?;
        let output_bytes = pixels.checked_mul(out_bpp as u64)?;
        let working = pixels.checked_mul(WORKING_BYTES_PER_PX)?;
        let peak = output_bytes
            .checked_add(working)?
            .checked_add(FIXED_OVERHEAD_BYTES)?;
        let wall_ms = (pixels as f64 / (DECODE_MPIX_PER_S * 1_000.0)).ceil() as u64;
        Some((peak, wall_ms))
    }

    // ── JxlDecodeJob ────────────────────────────────────────────────────

    /// Per-operation decode job for JPEG XL.
    pub struct JxlDecodeJob {
        limits: Option<ResourceLimits>,
        policy: DecodePolicy,
        stop: Option<zencodec::StopToken>,
        start_frame_index: u32,
        extract_gain_map: bool,
        /// Gain-map rendition intent (zencodec 0.1.21). `Components` (and
        /// `ReconstructHdr`, downgraded — zenjxl surfaces, it does not apply)
        /// additionally decodes the jhgm gain-map codestream into a
        /// [`zencodec::decode::DecodedGainMap`]. Default `BaseOnly`.
        gain_map_render: zencodec::GainMapRender,
        /// How to handle the image's stored EXIF/container orientation.
        ///
        /// Default [`OrientationHint::Preserve`] — the zencodec ecosystem
        /// default. Under `Preserve` the JXL decoder does **not** bake the
        /// orientation: pixels are emitted in their stored orientation and
        /// [`ImageInfo`] reports the coded dimensions plus the intrinsic EXIF
        /// [`Orientation`]. Under [`Correct`](OrientationHint::Correct) the
        /// decoder bakes the stored orientation natively (display dims,
        /// `Identity` residual). [`CorrectAndTransform`] and
        /// [`ExactTransform`] additionally apply the requested transform on top
        /// of the decoder's native bake.
        ///
        /// [`CorrectAndTransform`]: OrientationHint::CorrectAndTransform
        /// [`ExactTransform`]: OrientationHint::ExactTransform
        orientation: OrientationHint,
    }

    impl JxlDecodeJob {
        /// Enable extraction of the HDR gain map (ISO 21496-1 `jhgm` box).
        ///
        /// When `true`, a [`GainMapSource`](zencodec::gainmap::GainMapSource) is
        /// attached to the [`DecodeOutput`](zencodec::decode::DecodeOutput) as a
        /// typed extension (retrievable via `extras::<GainMapSource>()`).
        /// The source contains the raw JXL codestream and parsed ISO 21496-1
        /// metadata, ready for downstream decode.
        ///
        /// Defaults to `false` — gain map data is skipped even when present.
        /// [`GainMapPresence`](zencodec::GainMapPresence) on [`ImageInfo`] is
        /// always populated regardless of this flag.
        pub fn with_extract_gain_map(mut self, extract: bool) -> Self {
            self.extract_gain_map = extract;
            self
        }

        /// Gain-map rendition intent for jhgm bundles.
        ///
        /// With the `reconstruct-hdr` feature (`reconstructs_hdr()` is
        /// `true`), `ReconstructHdr` applies natively: an SDR-base bundle
        /// gets the gain map applied via ultrahdr-core into linear f32/f16
        /// RGBA with a content-light-level / mastering-display envelope; an
        /// HDR-base bundle (JXL-typical) returns the base, which already
        /// carries its own HDR signaling. Without the feature,
        /// `ReconstructHdr` downgrades to surfacing
        /// [`zencodec::GainMapRender::Components`] — the base stays honestly
        /// labeled and the caller applies one layer up.
        pub fn with_gain_map_render(mut self, render: zencodec::GainMapRender) -> Self {
            self.gain_map_render = render;
            self
        }

        /// Strip metadata fields from an `ImageInfo` according to the decode policy.
        fn apply_policy(info: ImageInfo, policy: &DecodePolicy) -> ImageInfo {
            let mut info = info;
            if !policy.resolve_icc(true) {
                info.source_color.icc_profile = None;
            }
            if !policy.resolve_exif(true) {
                info.embedded_metadata.exif = None;
            }
            if !policy.resolve_xmp(true) {
                info.embedded_metadata.xmp = None;
            }
            info
        }

        /// Convert native JxlInfo into zencodec ImageInfo.
        fn jxl_info_to_image_info(info: &JxlInfo) -> ImageInfo {
            let mut image_info = ImageInfo::new(info.width, info.height, ImageFormat::Jxl)
                .with_alpha(info.has_alpha)
                .with_bit_depth(info.bit_depth.unwrap_or(8))
                .with_channel_count(match (info.is_gray, info.has_alpha) {
                    (true, false) => 1,
                    (true, true) => 2,
                    (false, false) => 3,
                    (false, true) => 4,
                });

            if info.has_animation {
                image_info = image_info.with_sequence(zencodec::ImageSequence::Animation {
                    frame_count: None,
                    loop_count: None,
                    random_access: true,
                });
            }

            // `info.orientation` is the *residual* EXIF orientation of the
            // emitted pixels (mode-aware): Identity when the JXL decoder baked
            // the stored orientation (Correct path, `adjust_orientation = true`),
            // and the intrinsic stored orientation when it did not (Preserve
            // path). Reporting the residual — not the intrinsic tag — is what
            // avoids the double-rotation hazard: paired with `info.width`/
            // `info.height` (also the emitted geometry), a consumer that applies
            // the reported orientation lands upright exactly once. The extra
            // transform of `CorrectAndTransform`/`ExactTransform` is applied (and
            // the report rewritten to Identity) by `apply_orientation_to_output`
            // / `report_probe_for_hint` after this base conversion.
            image_info = image_info
                .with_orientation(Orientation::from_exif(info.orientation).unwrap_or_default());

            if let Some((cp, tc, mc, fr)) = info.cicp {
                image_info = image_info.with_cicp(Cicp::new(cp, tc, mc, fr));
            }

            if let Some(ref icc) = info.icc_profile {
                image_info = image_info.with_icc_profile(icc.clone());
            }

            // JXL color mode is exclusive:
            //   want_icc=true  → only ICC is set → authority stays Icc (default)
            //   want_icc=false → both CICP and a synthesized ICC are set → CICP is authoritative
            if info.cicp.is_some() {
                image_info = image_info.with_color_authority(ColorAuthority::Cicp);
            }

            if let Some(ref exif) = info.exif {
                image_info = image_info.with_exif(exif.clone());
            }

            if let Some(ref xmp) = info.xmp {
                image_info = image_info.with_xmp(xmp.clone());
            }

            // JXL ToneMapping.intensity_target = peak luminance the content was
            // mastered for. Default 255.0 = SDR; >255 indicates HDR. Surface as
            // MaxCLL so downstream HDR-aware code (tone-mapping policy, encode
            // negotiation) sees the peak. JXL has no MaxFALL signal — leave 0.
            // JXL also has no separate cLLi-style box; this is the closest
            // semantic match in the zencodec metadata model.
            if info.intensity_target > 255.0 {
                let max_cll = info.intensity_target.min(u16::MAX as f32) as u16;
                image_info =
                    image_info.with_content_light_level(ContentLightLevel::new(max_cll, 0));
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

            let base = match (info.is_gray, info.has_alpha, is_float, is_16) {
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
            };
            enrich_descriptor_from_cicp(base, info.cicp)
        }
    }

    /// Re-tag an output descriptor with the transfer function and color
    /// primaries from the codestream's CICP color encoding, when present.
    ///
    /// When the JXL signals an enum (CICP-expressible) color encoding, the
    /// decoder renders into exactly that encoding for every output depth —
    /// including f32, where a PQ or sRGB source yields PQ-/sRGB-coded floats,
    /// NOT linear (zenjxl-decoder only falls back to linear sRGB floats for
    /// XYB images whose embedded profile is ICC-only, i.e. `cicp == None`).
    /// So tagging from CICP both signals HDR (PQ/HLG) and corrects the
    /// blanket `_SRGB`/`_LINEAR` claims of the base descriptors. With no
    /// CICP the base claim stands (ICC rides along in the metadata).
    fn enrich_descriptor_from_cicp(
        mut desc: PixelDescriptor,
        cicp: Option<(u8, u8, u8, bool)>,
    ) -> PixelDescriptor {
        let Some((cp, tc, _, _)) = cicp else {
            return desc;
        };
        if let Some(tf) = zenpixels::TransferFunction::from_cicp(tc) {
            desc = desc.with_transfer(tf);
        }
        if let Some(p) = zenpixels::ColorPrimaries::from_cicp(cp) {
            desc = desc.with_primaries(p);
        }
        desc
    }

    // ── Orientation handling ─────────────────────────────────────────────
    //
    // The JXL decoder can bake the image's *intrinsic* (stored) orientation
    // natively via `JxlDecoderOptions::adjust_orientation`. The zencodec
    // `OrientationHint` is richer — it can also request an arbitrary extra
    // transform on top — so we split each hint into two parts:
    //
    //   1. the decoder's `adjust_orientation` flag (does the decoder bake the
    //      intrinsic orientation, or emit stored pixels?), and
    //   2. an *extra* post-decode [`Orientation`] applied to the decoder's
    //      output (Identity for the common cases; a real transform only for
    //      `ExactTransform`/`CorrectAndTransform`).
    //
    // This leans on the decoder's native bake for `Correct` (the common case)
    // — no pixel copy at all — and only falls back to an explicit buffer
    // transform when the caller asks for a transform the codestream can't
    // express on its own.
    impl JxlDecodeJob {
        /// Whether `hint` bakes the pixels at all (transform → report `Identity`)
        /// vs. leaves them in their stored orientation (`Preserve`).
        ///
        /// This is the local equivalent of `OrientationHint::bakes()` — inlined so
        /// the adapter does not require an unreleased zencodec (published 0.1.21
        /// lacks `bakes()`; it is committed for 0.1.22 at `6136ff6`). [`Preserve`]
        /// is the only hint that leaves pixels untouched; every other hint bakes.
        ///
        /// Unlike codecs that bake every transform themselves, zenjxl splits the
        /// work: the JXL decoder bakes the intrinsic orientation natively
        /// ([`decoder_adjust_orientation`](Self::decoder_adjust_orientation)) and we
        /// apply only the residual transform
        /// ([`extra_orientation`](Self::extra_orientation)). So those two precise
        /// predicates — not this coarse one — drive the decode path; `hint_bakes` is
        /// kept as the single source of truth for "is this the preserve path?"
        /// (used in debug assertions and as the documented gate).
        ///
        /// [`Preserve`]: OrientationHint::Preserve
        fn hint_bakes(hint: OrientationHint) -> bool {
            !matches!(hint, OrientationHint::Preserve)
        }

        /// The `adjust_orientation` flag to pass to the JXL decoder for `hint`.
        ///
        /// `true` makes the decoder bake the stored orientation natively (so for
        /// [`Correct`](OrientationHint::Correct) and
        /// [`CorrectAndTransform`](OrientationHint::CorrectAndTransform) the pixels
        /// come back already EXIF-corrected). `false` emits stored pixels
        /// untransformed — used by [`Preserve`](OrientationHint::Preserve) and by
        /// [`ExactTransform`](OrientationHint::ExactTransform), which ignores EXIF
        /// and applies its literal transform to the raw stored pixels.
        ///
        /// Invariant: on the preserve path ([`hint_bakes`](Self::hint_bakes) is
        /// `false`) the result is always `false` and
        /// [`extra_orientation`](Self::extra_orientation) is `Identity` — i.e. the
        /// pixels are emitted exactly as stored.
        fn decoder_adjust_orientation(hint: OrientationHint) -> bool {
            match hint {
                OrientationHint::Preserve => false,
                OrientationHint::Correct => true,
                OrientationHint::CorrectAndTransform(_) => true,
                OrientationHint::ExactTransform(_) => false,
                // `OrientationHint` is `#[non_exhaustive]`; a future variant is
                // treated as preserve (no native bake) — the extra-transform
                // resolver below also returns Identity for it, so the net effect is
                // "leave pixels stored", the safest default.
                _ => false,
            }
        }

        /// The extra [`Orientation`] to apply to the decoder's output for `hint`,
        /// on top of whatever the decoder did natively (per
        /// [`decoder_adjust_orientation`]).
        ///
        /// - [`Preserve`](OrientationHint::Preserve): nothing extra (decoder emitted
        ///   stored pixels; we surface them as-is) → [`Identity`](Orientation::Identity).
        /// - [`Correct`](OrientationHint::Correct): the decoder already baked the
        ///   intrinsic orientation → [`Identity`](Orientation::Identity).
        /// - [`CorrectAndTransform(t)`](OrientationHint::CorrectAndTransform): the
        ///   decoder baked the intrinsic correction; apply `t` on top.
        /// - [`ExactTransform(t)`](OrientationHint::ExactTransform): the decoder
        ///   emitted stored pixels (EXIF ignored); apply `t` literally.
        fn extra_orientation(hint: OrientationHint) -> Orientation {
            match hint {
                OrientationHint::Preserve | OrientationHint::Correct => Orientation::Identity,
                OrientationHint::CorrectAndTransform(t) | OrientationHint::ExactTransform(t) => t,
                // Future `#[non_exhaustive]` variant: no extra transform.
                _ => Orientation::Identity,
            }
        }

        /// The total [`Orientation`] the decode pipeline applies to the *stored*
        /// pixels for `hint` — the decoder's native bake composed with the extra
        /// transform. This is what [`OutputInfo::orientation_applied`] wants (the
        /// framework derives "remaining for the caller" from it).
        ///
        /// `info` must come from a probe in the matching orientation mode so its
        /// [`JxlInfo::intrinsic_orientation`] is populated.
        ///
        /// - [`Preserve`](OrientationHint::Preserve): nothing applied →
        ///   [`Identity`](Orientation::Identity); the caller still owns the intrinsic
        ///   orientation.
        /// - [`Correct`](OrientationHint::Correct): the decoder applied the intrinsic
        ///   correction.
        /// - [`ExactTransform(t)`](OrientationHint::ExactTransform): only `t` (EXIF
        ///   ignored).
        /// - [`CorrectAndTransform(t)`](OrientationHint::CorrectAndTransform): the
        ///   intrinsic correction then `t`.
        fn total_applied_orientation(info: &JxlInfo, hint: OrientationHint) -> Orientation {
            let intrinsic = Orientation::from_exif(info.intrinsic_orientation).unwrap_or_default();
            match hint {
                OrientationHint::Preserve => Orientation::Identity,
                OrientationHint::Correct => intrinsic,
                OrientationHint::ExactTransform(t) => t,
                OrientationHint::CorrectAndTransform(t) => intrinsic.then(t),
                // Future `#[non_exhaustive]` variant: nothing applied.
                _ => Orientation::Identity,
            }
        }

        /// Rewrite a probe [`ImageInfo`] so it agrees with what `decode` will emit
        /// for `hint`.
        ///
        /// The base `info` comes from a probe whose `adjust_orientation` already
        /// matched [`decoder_adjust_orientation`], so its dims + reported
        /// orientation are correct for the decoder-native part:
        ///
        /// - On the **preserve path** ([`hint_bakes`](Self::hint_bakes) is `false`)
        ///   the info is returned untouched — stored dims + the intrinsic tag the
        ///   caller must still apply.
        /// - On a **bake path** the pixels are declared final, so the reported
        ///   orientation becomes [`Identity`](Orientation::Identity) (even when the
        ///   net transform is Identity, e.g. `ExactTransform(Identity)` or `Correct`
        ///   on an upright image — a consumer must never re-apply a stale tag). The
        ///   *extra* transform's axis-swap is folded into the reported dims.
        fn report_probe_for_hint(mut info: ImageInfo, hint: OrientationHint) -> ImageInfo {
            if !Self::hint_bakes(hint) {
                return info;
            }
            let extra = Self::extra_orientation(hint);
            let (ow, oh) = extra.output_dimensions(info.width, info.height);
            info.width = ow;
            info.height = oh;
            info.with_orientation(Orientation::Identity)
        }

        /// Apply the extra orientation transform for `hint` to a decoded pixel
        /// buffer + its [`ImageInfo`], rewriting the reported geometry + orientation
        /// to match what the caller asked for.
        ///
        /// The decoder already handled the intrinsic-orientation bake (per
        /// [`decoder_adjust_orientation`]); this applies only the residual transform
        /// of `ExactTransform`/`CorrectAndTransform`:
        ///
        /// - On the **preserve path** the inputs are returned untouched (stored
        ///   pixels, intrinsic tag).
        /// - On a **bake path** the reported orientation becomes
        ///   [`Identity`](Orientation::Identity) (the pixels are final). The pixel
        ///   *copy* is performed only when the extra transform is non-Identity; when
        ///   it is Identity (e.g. `Correct`, or `ExactTransform(Identity)`) the
        ///   buffer is kept as-is and only the tag is normalized to Identity.
        ///
        /// Operating on `(PixelBuffer, ImageInfo)` rather than a built
        /// [`DecodeOutput`] keeps the source-encoding details and any gain-map
        /// extensions intact — those are reattached by the caller after this
        /// transform.
        fn apply_orientation_to_pixels(
            pixels: PixelBuffer,
            mut info: ImageInfo,
            hint: OrientationHint,
        ) -> (PixelBuffer, ImageInfo) {
            if !Self::hint_bakes(hint) {
                // Preserve: stored pixels, intrinsic tag — leave both untouched.
                return (pixels, info);
            }
            let extra = Self::extra_orientation(hint);
            if extra.is_identity() {
                // Bake path with no net transform (Correct, or
                // ExactTransform(Identity)): the decoder already produced the final
                // pixels; just declare them final so no stale tag is re-applied.
                info = info.with_orientation(Orientation::Identity);
                return (pixels, info);
            }
            // Pixel-exact orientation transform — no resampling, no rounding.
            let baked = zenpixels_convert::orient::apply_orientation(pixels.as_slice(), extra);
            // `ImageInfo` has no dimension setter; the fields are public. Report the
            // baked buffer's geometry + Identity (pixels are now final, so no
            // consumer can double-apply a stale tag).
            info.width = baked.width();
            info.height = baked.height();
            info = info.with_orientation(Orientation::Identity);
            (baked, info)
        }
    }

    impl<'a> zencodec::decode::DecodeJob<'a> for JxlDecodeJob {
        type Error = At<zencodec::CodecError>;
        type Dec = JxlDecoder<'a>;
        type StreamDec = Unsupported<At<zencodec::CodecError>>;
        type AnimationFrameDec = JxlAnimationFrameDecoder;

        fn with_stop(mut self, stop: zencodec::StopToken) -> Self {
            self.stop = Some(stop);
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

        fn with_gain_map_render(self, render: zencodec::GainMapRender) -> Self {
            // Delegate to the inherent builder (also reachable without the
            // trait import); keeps the dyn `set_gain_map_render` parity path
            // working instead of falling through to the provided no-op.
            JxlDecodeJob::with_gain_map_render(self, render)
        }

        fn with_policy(mut self, policy: DecodePolicy) -> Self {
            self.policy = policy;
            self
        }

        fn with_orientation(mut self, hint: OrientationHint) -> Self {
            self.orientation = hint;
            self
        }

        fn probe(&self, data: &[u8]) -> Result<ImageInfo, At<zencodec::CodecError>> {
            (move || -> Result<ImageInfo, At<JxlError>> {
                // Enforce input size limit.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_input_size(data.len() as u64)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }
                // Probe in the same orientation mode the decode will use, so the
                // reported dims + orientation match what `decode` emits.
                let info = probe_with_orientation(
                    data,
                    Self::decoder_adjust_orientation(self.orientation),
                )?;
                // Enforce dimension limits after probing the header. The post-decode
                // extra transform (ExactTransform/CorrectAndTransform) only reorders
                // pixels, so the buffer size to bound is the emitted geometry here.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_dimensions(info.width, info.height)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }
                let image_info =
                    Self::jxl_info_to_image_info(&info).with_source_encoding_details(info);
                // Fold in the extra transform's dims/Identity for the bake hints
                // that the decoder can't express natively.
                let image_info = Self::report_probe_for_hint(image_info, self.orientation);
                Ok(Self::apply_policy(image_info, &self.policy))
            })()
            .map_err(zencodec::CodecError::of)
        }

        fn output_info(&self, data: &[u8]) -> Result<OutputInfo, At<zencodec::CodecError>> {
            (move || -> Result<OutputInfo, At<JxlError>> {
                // Enforce input size limit.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_input_size(data.len() as u64)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }
                // Probe in the chosen orientation mode so `info` carries the emitted
                // (decoder-native) geometry; the extra transform is folded in below.
                let info = probe_with_orientation(
                    data,
                    Self::decoder_adjust_orientation(self.orientation),
                )?;
                // Bound the actual decode against the decoder-native geometry (the
                // extra transform only reorders pixels, never grows the buffer).
                if let Some(ref limits) = self.limits {
                    limits
                        .check_dimensions(info.width, info.height)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }
                let native_desc = Self::native_descriptor(&info);
                // Final emitted geometry = decoder-native geometry with the extra
                // transform's axis-swap folded in.
                let extra = Self::extra_orientation(self.orientation);
                let (ow, oh) = extra.output_dimensions(info.width, info.height);
                // Total orientation the pipeline applies to the *stored* pixels —
                // the decoder's native bake (intrinsic on the bake path, Identity on
                // preserve) composed with the extra transform. Reported so the
                // framework's "remaining for caller" arithmetic stays correct.
                let applied = Self::total_applied_orientation(&info, self.orientation);
                Ok(OutputInfo::full_decode(ow, oh, native_desc)
                    .with_alpha(info.has_alpha)
                    .with_orientation_applied(applied))
            })()
            .map_err(zencodec::CodecError::of)
        }

        fn decoder(
            self,
            data: Cow<'a, [u8]>,
            preferred: &[PixelDescriptor],
        ) -> Result<JxlDecoder<'a>, At<zencodec::CodecError>> {
            (move || -> Result<JxlDecoder<'a>, At<JxlError>> {
                // Enforce input size limit before decoding.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_input_size(data.len() as u64)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }
                Ok(JxlDecoder {
                    data,
                    limits: self.limits,
                    policy: self.policy,
                    stop: self.stop,
                    preferred: preferred.to_vec(),
                    extract_gain_map: self.extract_gain_map,
                    gain_map_render: self.gain_map_render,
                    orientation: self.orientation,
                })
            })()
            .map_err(zencodec::CodecError::of)
        }

        fn streaming_decoder(
            self,
            _data: Cow<'a, [u8]>,
            _preferred: &[PixelDescriptor],
        ) -> Result<Unsupported<At<zencodec::CodecError>>, At<zencodec::CodecError>> {
            Err(JxlError::from(UnsupportedOperation::RowLevelDecode).into())
        }

        fn push_decoder(
            self,
            data: Cow<'a, [u8]>,
            sink: &mut dyn zencodec::decode::DecodeRowSink,
            preferred: &[PixelDescriptor],
        ) -> Result<OutputInfo, At<zencodec::CodecError>> {
            // The helper drives the typed decode path and yields `Self::Error`
            // (now `At<CodecError>`); the sink-error closure bridges via `From`.
            zencodec::helpers::copy_decode_to_sink(self, data, sink, preferred, |e| {
                JxlError::Sink(e).into()
            })
        }

        fn animation_frame_decoder(
            self,
            data: Cow<'a, [u8]>,
            preferred: &[PixelDescriptor],
        ) -> Result<JxlAnimationFrameDecoder, At<zencodec::CodecError>> {
            (move || -> Result<JxlAnimationFrameDecoder, At<JxlError>> {
                if !self.policy.resolve_animation(true) {
                    return Err(whereat::at!(JxlError::UnsupportedOperation(
                        UnsupportedOperation::AnimationDecode,
                    )));
                }
                // Enforce input size limit before decoding.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_input_size(data.len() as u64)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }
                // Eagerly probe to populate image_info so info() never panics.
                let info = probe(&data)?;
                // Enforce dimension limits after probing the header.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_dimensions(info.width, info.height)
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }
                let image_info = Arc::new(Self::apply_policy(
                    Self::jxl_info_to_image_info(&info),
                    &self.policy,
                ));
                Ok(JxlAnimationFrameDecoder {
                    data: data.into_owned(),
                    limits: self.limits,
                    policy: self.policy,
                    stop: self.stop,
                    preferred: preferred.to_vec(),
                    frames: None,
                    image_info: Some(image_info),
                    current: None,
                    start_frame_index: self.start_frame_index,
                })
            })()
            .map_err(zencodec::CodecError::of)
        }
    }

    // ── JxlDecoder ──────────────────────────────────────────────────────

    /// Single-image JPEG XL decoder.
    pub struct JxlDecoder<'a> {
        data: Cow<'a, [u8]>,
        limits: Option<ResourceLimits>,
        policy: DecodePolicy,
        stop: Option<zencodec::StopToken>,
        preferred: Vec<PixelDescriptor>,
        extract_gain_map: bool,
        gain_map_render: zencodec::GainMapRender,
        /// Orientation handling inherited from the [`JxlDecodeJob`].
        orientation: OrientationHint,
    }

    impl zencodec::decode::Decode for JxlDecoder<'_> {
        type Error = At<zencodec::CodecError>;

        fn decode(self) -> Result<DecodeOutput, At<zencodec::CodecError>> {
            (move || -> Result<DecodeOutput, At<JxlError>> {
                let native_limits = JxlDecodeJob::to_native_limits(&self.limits);
                let parallel = policy_to_parallel(&self.limits);
                // Forward the caller's allocation-fallibility preference to the
                // wrapper's untrusted output-buffer allocation. The direct
                // (non-zencodec) decode API leaves this `CodecDefault`; here the
                // zencodec adapter maps `ResourceLimits::prefer_fallible_allocations`.
                let alloc_pref = self
                    .limits
                    .as_ref()
                    .map(|l| l.prefer_fallible_allocations)
                    .unwrap_or_default();
                let stop_arc: Option<Arc<dyn Stop>> =
                    self.stop.map(|s| Arc::new(s) as Arc<dyn Stop>);
                #[cfg(feature = "reconstruct-hdr")]
                let stop_for_apply = stop_arc.clone();
                // Gate progressive content during decode when the policy forbids it.
                // `resolve_progressive(true)` is true unless `allow_progressive`
                // is explicitly `Some(false)`, so we only reject when the caller
                // opted out. The decoder errors at the first progressive frame
                // header; the header-only probe is unaffected.
                let reject_progressive = !self.policy.resolve_progressive(true);
                // Decode in the orientation mode the hint selects: the JXL decoder
                // bakes the intrinsic orientation natively on the `Correct`/
                // `CorrectAndTransform` path (`adjust_orientation = true`) and emits
                // stored pixels on `Preserve`/`ExactTransform`. The extra transform
                // of `ExactTransform`/`CorrectAndTransform` is applied below.
                let result = decode_with_options_oriented(
                    &self.data,
                    native_limits.as_ref(),
                    &self.preferred,
                    parallel,
                    stop_arc,
                    JxlDecodeJob::decoder_adjust_orientation(self.orientation),
                    reject_progressive,
                    alloc_pref,
                )?;

                let info = JxlDecodeJob::apply_policy(
                    JxlDecodeJob::jxl_info_to_image_info(&result.info),
                    &self.policy,
                );
                // Re-tag color signaling from the codestream CICP (see
                // enrich_descriptor_from_cicp): the decoder rendered into the
                // signaled encoding, so PQ/HLG/wide-gamut sources keep their
                // code-value meaning in the output descriptor.
                let enriched =
                    enrich_descriptor_from_cicp(result.pixels.descriptor(), result.info.cicp);
                let pixels = result.pixels.with_descriptor(enriched);
                // Gain-map rendition intent: Components decodes the jhgm gain-map
                // codestream into a DecodedGainMap; ReconstructHdr applies it
                // natively when the `reconstruct-hdr` feature is on, and
                // downgrades to surfacing Components per the zencodec contract
                // when it is off (reconstructs_hdr() is false — the base stays
                // honestly labeled per its own descriptor). Unknown future modes
                // are refused, never mis-rendered.
                let (surface_components, reconstruct_target): (bool, Option<Option<f32>>) =
                    match self.gain_map_render {
                        zencodec::GainMapRender::BaseOnly => (false, None),
                        zencodec::GainMapRender::Components => (true, None),
                        #[cfg(feature = "reconstruct-hdr")]
                        zencodec::GainMapRender::ReconstructHdr { target_headroom } => {
                            (false, Some(target_headroom))
                        }
                        #[cfg(not(feature = "reconstruct-hdr"))]
                        zencodec::GainMapRender::ReconstructHdr { .. } => (true, None),
                        _ => {
                            return Err(whereat::at!(JxlError::InvalidInput(
                                "unrecognized GainMapRender mode".into()
                            )));
                        }
                    };
                #[cfg(not(feature = "reconstruct-hdr"))]
                let _ = reconstruct_target;

                // Native HDR reconstruction: apply the gain map to the decoded
                // base. A ReconstructHdr request on a plain (no-jhgm) image is a
                // normal decode — the base may itself be the HDR rendition.
                #[cfg(feature = "reconstruct-hdr")]
                let (pixels, info) = match (reconstruct_target, result.gain_map.as_ref()) {
                    (Some(target), Some(gm)) => {
                        let stop_ref: &dyn Stop = match &stop_for_apply {
                            Some(s) => &**s,
                            None => &enough::Unstoppable,
                        };
                        reconstruct_hdr_base(
                            pixels,
                            info,
                            gm,
                            target,
                            &self.preferred,
                            native_limits.as_ref(),
                            parallel,
                            stop_ref,
                            alloc_pref,
                        )?
                    }
                    _ => (pixels, info),
                };

                // Fold in the extra orientation transform for the bake hints the
                // decoder can't express natively (`ExactTransform`/
                // `CorrectAndTransform`). For `Preserve`/`Correct` this is Identity
                // and returns the pair untouched (no pixel copy). Applied after any
                // HDR gain-map reconstruction so the final composited image is
                // upright. Pixel-exact — orientation only reorders pixels.
                let (pixels, info) =
                    JxlDecodeJob::apply_orientation_to_pixels(pixels, info, self.orientation);

                let mut output =
                    DecodeOutput::new(pixels, info).with_source_encoding_details(result.info);
                if (self.extract_gain_map || surface_components)
                    && let Some(gm) = result.gain_map
                {
                    // Components: recursively decode the gain-map JXL codestream
                    // (same resource limits as the base decode). Errors only when
                    // a present gain map is malformed.
                    if surface_components {
                        // Same `alloc_pref` as the base decode so the gain-map
                        // sub-image's untrusted output buffer honors the caller's
                        // fallibility preference too. `adjust_orientation = true`,
                        // `reject_progressive = false` matches `decode_with_options`.
                        let gm_result = decode_with_options_oriented(
                            &gm.gain_map_codestream,
                            native_limits.as_ref(),
                            &[],
                            parallel,
                            None,
                            true,
                            false,
                            alloc_pref,
                        )?;
                        let source = bundle_to_gain_map_source(gm);
                        let gm_info = source.metadata.clone();
                        output = output.with_extras(zencodec::decode::DecodedGainMap::new(
                            gm_result.pixels,
                            gm_info,
                        ));
                        output = output.with_extras(source);
                    } else {
                        output = output.with_extras(bundle_to_gain_map_source(gm));
                    }
                }
                Ok(output)
            })()
            .map_err(zencodec::CodecError::of)
        }
    }

    // ── JxlAnimationFrameDecoder ──────────────────────────────────────────────

    /// Animation JPEG XL decoder (fully composited frames).
    ///
    /// Decodes all frames eagerly on first call to `render_next_frame()` — the
    /// jxl-rs decoder handles blending/disposal internally, producing
    /// fully composited frames.
    pub struct JxlAnimationFrameDecoder {
        data: Vec<u8>,
        limits: Option<ResourceLimits>,
        policy: DecodePolicy,
        stop: Option<zencodec::StopToken>,
        preferred: Vec<PixelDescriptor>,
        /// Pre-decoded frames (lazily populated on first render_next_frame call).
        frames: Option<DecodedFrames>,
        /// Image info, set after decoding.
        image_info: Option<Arc<ImageInfo>>,
        /// Current frame for borrowed access via `render_next_frame`.
        current: Option<OwnedAnimationFrame>,
        /// Number of displayed frames to skip from the front.
        start_frame_index: u32,
    }

    struct DecodedFrames {
        frames: VecDeque<OwnedAnimationFrame>,
        loop_count: Option<u32>,
    }

    impl JxlAnimationFrameDecoder {
        /// Decode all frames up front.
        fn decode_all_frames(&mut self) -> Result<(), At<JxlError>> {
            let mut options = JxlDecoderOptions::default();

            // Per-frame output buffers are sized from the (untrusted) header
            // dimensions, so they honor the caller's allocation-fallibility
            // preference with a *fallible* site default. The direct decode API
            // never reaches this path; only the zencodec adapter does, mapping
            // `ResourceLimits::prefer_fallible_allocations`.
            let alloc_pref = self
                .limits
                .as_ref()
                .map(|l| l.prefer_fallible_allocations)
                .unwrap_or_default();

            // Honor the same progressive gate as the single-image decode path:
            // an animation whose frames are progressive must also be rejected
            // when the policy forbids it. Header parse below stays unaffected
            // (the gate fires only at frame headers, of which there are none yet).
            options.reject_progressive = !self.policy.resolve_progressive(true);

            if let Some(p) = policy_to_parallel(&self.limits) {
                options.parallel = p;
            }

            if let Some(ref lim) = self.limits {
                if let Some(max_px) = lim.max_pixels {
                    // Saturate u64 → usize on 32-bit targets.
                    options.limits.max_pixels = Some(usize::try_from(max_px).unwrap_or(usize::MAX));
                }
                if let Some(max_mem) = lim.max_memory_bytes {
                    // jxl-rs takes max_memory_bytes as u64; pass through directly
                    // so the wrapper's memory cap is honored end-to-end.
                    options.limits.max_memory_bytes = Some(max_mem);
                }
            }

            // Forward stop token for cooperative cancellation.
            if let Some(ref stop) = self.stop {
                options.stop = Arc::new(stop.clone());
            }

            let decoder = JxlRsDecoder::new(options);

            // Parse header
            let mut input: &[u8] = &self.data;
            let result = decoder.process(&mut input).map_err_at(JxlError::from)?;
            let mut decoder = match result {
                ProcessingResult::Complete { result } => result,
                ProcessingResult::NeedsMoreInput { .. } => {
                    return Err(whereat::at!(JxlError::InvalidInput(
                        "JXL: insufficient data for header".into(),
                    )));
                }
            };

            let basic_info = decoder.basic_info();
            // Animation decodes use the default options (orientation adjusted):
            // `size` is the display geometry and `orientation` is the residual
            // (Identity). `coded_size` / `intrinsic_orientation` report the
            // stored values.
            let (width, height) = basic_info.size;
            let (coded_width, coded_height) = basic_info.coded_size;
            let has_alpha = basic_info
                .extra_channels
                .iter()
                .any(|ec| matches!(ec.ec_type, ExtraChannel::Alpha));
            let has_animation = basic_info.animation.is_some();
            let loop_count = basic_info.animation.as_ref().map(|a| a.num_loops);
            let bit_depth_u8 = basic_info.bit_depth.bits_per_sample() as u8;
            let orientation = basic_info.orientation as u8;
            let intrinsic_orientation = basic_info.intrinsic_orientation as u8;
            let is_gray = crate::decode::profile_is_grayscale(decoder.embedded_color_profile());
            let num_extra = basic_info.extra_channels.len();
            let xyb_encoded = !basic_info.uses_original_profile;
            let extra_channels = crate::decode::convert_extra_channels(&basic_info.extra_channels);
            let preview_size = basic_info.preview_size.map(|(w, h)| (w as u32, h as u32));
            let intrinsic_size = basic_info.intrinsic_size.map(|(w, h)| (w as u32, h as u32));
            let intensity_target = basic_info.tone_mapping.intensity_target;
            let min_nits = basic_info.tone_mapping.min_nits;
            let relative_to_max_display = basic_info.tone_mapping.relative_to_max_display;
            let linear_below = basic_info.tone_mapping.linear_below;

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

            let width_u32 =
                crate::decode::dim_to_u32(width, "width").map_err(|e| whereat::at!(e))?;
            let height_u32 =
                crate::decode::dim_to_u32(height, "height").map_err(|e| whereat::at!(e))?;
            let coded_width_u32 = crate::decode::dim_to_u32(coded_width, "coded width")
                .map_err(|e| whereat::at!(e))?;
            let coded_height_u32 = crate::decode::dim_to_u32(coded_height, "coded height")
                .map_err(|e| whereat::at!(e))?;

            let (bytes_per_row, frame_buf_bytes) =
                crate::decode::checked_buf_size(width, height, channels, bytes_per_sample)
                    .map_err(|e| whereat::at!(e))?;

            let is_f32 = matches!(
                chosen.pixel_format.color_data_format,
                Some(jxl::api::JxlDataFormat::F32 { .. })
            );
            // Only clamp when CICP explicitly tells us it's SDR.
            // When CICP is absent (ICC-only), we don't know the gamut.
            let clamp = is_f32 && cicp.is_some() && !is_hdr_or_wide_gamut(cicp);

            let mut frames = VecDeque::new();
            let mut frame_index = 0u32;
            // Bytes retained across frames (matches encoder-side accumulator) —
            // used to gate against ResourceLimits.max_memory_bytes so a long
            // animation cannot allocate without bound.
            let mut accumulated_bytes: u64 = 0;
            // Track the final decoder so we can extract EXIF/XMP after the loop.
            let mut final_decoder = None;

            loop {
                // Gate frame count BEFORE allocating the next frame's buffer.
                // `frame_index` is the count of frames seen so far, so the
                // (frame_index + 1)th frame must satisfy max_frames.
                if let Some(ref limits) = self.limits {
                    limits
                        .check_frames(frame_index.saturating_add(1))
                        .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                }

                // Advance to frame info. This is where `reject_progressive`
                // fires (at the frame header), so route through `map_err` to
                // surface the dedicated `JxlError::ProgressiveRejected`.
                let result = decoder
                    .process(&mut input)
                    .map_err(|e| e.map_error(map_err))?;
                let decoder_fi = match result {
                    ProcessingResult::Complete { result } => result,
                    ProcessingResult::NeedsMoreInput { .. } => break,
                };

                let frame_header = decoder_fi.frame_header();
                let duration_ms = frame_header.duration.map(|d| d as u32).unwrap_or(0);

                // Decode pixels. The per-frame output buffer is sized from the
                // (untrusted) header dimensions, so it honors `alloc_pref` with
                // a *fallible* site default.
                let mut buf = crate::alloc_util::alloc_zeroed(alloc_pref, true, frame_buf_bytes)?;
                let output = JxlOutputBuffer::new(&mut buf, height, bytes_per_row);

                let result = decoder_fi
                    .process(&mut input, &mut [output])
                    .map_err(|e| e.map_error(map_err))?;
                let next_decoder = match result {
                    ProcessingResult::Complete { result } => result,
                    ProcessingResult::NeedsMoreInput { .. } => {
                        return Err(whereat::at!(JxlError::InvalidInput(
                            "JXL: insufficient data for frame pixels".into(),
                        )));
                    }
                };

                // Skip frames before start_frame_index: decode them (jxl-rs
                // requires sequential decode) but drop immediately instead of
                // storing in the VecDeque.  This avoids holding all skipped
                // frames in memory at peak.
                if frame_index >= self.start_frame_index {
                    // Track retained-memory growth and gate against limits
                    // before pushing the new frame, mirroring the encoder
                    // path (ResourceLimits.check_memory).
                    accumulated_bytes = accumulated_bytes.saturating_add(frame_buf_bytes as u64);
                    if let Some(ref limits) = self.limits {
                        limits
                            .check_memory(accumulated_bytes)
                            .map_err(|e| whereat::at!(JxlError::LimitExceeded(e.to_string())))?;
                    }
                    if clamp {
                        crate::decode::clamp_f32_buf(&mut buf);
                    }
                    let pixel_buf = build_pixel_data(&buf, width, height, &chosen);
                    frames.push_back(OwnedAnimationFrame::new(
                        pixel_buf,
                        duration_ms,
                        frame_index,
                    ));
                }

                frame_index += 1;

                if !next_decoder.has_more_frames() {
                    final_decoder = Some(next_decoder);
                    break;
                }
                decoder = next_decoder;
            }

            // Extract EXIF and XMP metadata from container boxes.
            // These may appear after the codestream, so they're only
            // available after all frames have been decoded.
            let exif = final_decoder.as_mut().and_then(|d| d.take_exif());
            let xmp = final_decoder.as_mut().and_then(|d| d.take_xmp());

            let jxl_info = JxlInfo {
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
                min_nits,
                relative_to_max_display,
                linear_below,
                intrinsic_size,
            };
            let image_info = Arc::new(JxlDecodeJob::apply_policy(
                JxlDecodeJob::jxl_info_to_image_info(&jxl_info),
                &self.policy,
            ));

            self.image_info = Some(image_info);
            self.frames = Some(DecodedFrames { frames, loop_count });

            Ok(())
        }
    }

    impl zencodec::decode::AnimationFrameDecoder for JxlAnimationFrameDecoder {
        type Error = At<zencodec::CodecError>;

        fn wrap_sink_error(err: SinkError) -> At<zencodec::CodecError> {
            JxlError::Sink(err).into()
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
        ) -> Result<Option<AnimationFrame<'_>>, At<zencodec::CodecError>> {
            // Single fallible site (the eager all-frames decode): convert at the
            // boundary. `render_next_frame` returns a frame borrowing `self`, so
            // a closure-wrapped inner body cannot be used here (the borrow would
            // not outlive the closure).
            if self.frames.is_none() {
                self.decode_all_frames().map_err(zencodec::CodecError::of)?;
            }

            let decoded = self.frames.as_mut().unwrap();
            self.current = decoded.frames.pop_front();
            Ok(self.current.as_ref().map(|f| f.as_animation_frame()))
        }

        fn render_next_frame_to_sink(
            &mut self,
            stop: Option<&dyn Stop>,
            sink: &mut dyn zencodec::decode::DecodeRowSink,
        ) -> Result<Option<OutputInfo>, At<zencodec::CodecError>> {
            // The helper drives `render_next_frame_owned` + `wrap_sink_error`,
            // both `Self::Error` (now `At<CodecError>`).
            zencodec::helpers::copy_frame_to_sink(self, stop, sink)
        }

        fn render_next_frame_owned(
            &mut self,
            _stop: Option<&dyn Stop>,
        ) -> Result<Option<OwnedAnimationFrame>, At<zencodec::CodecError>> {
            if self.frames.is_none() {
                self.decode_all_frames().map_err(zencodec::CodecError::of)?;
            }

            let decoded = self.frames.as_mut().unwrap();
            Ok(decoded.frames.pop_front())
        }
    }
}

// ── Re-exports ──────────────────────────────────────────────────────────────

#[cfg(feature = "encode")]
pub use encoding::{
    GainMapData, JxlAnimationFrameEncoder, JxlEncodeJob, JxlEncoder, JxlEncoderConfig,
};

#[cfg(all(feature = "encode", feature = "__expert"))]
pub use encoding::{DistanceSource, JxlEncodePlan, LosslessPlan, LossyPlan};

#[cfg(feature = "decode")]
pub use decoding::{JxlAnimationFrameDecoder, JxlDecodeJob, JxlDecoder, JxlDecoderConfig};

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "encode", feature = "decode"))]
    use super::*;
    #[cfg(any(feature = "encode", feature = "decode"))]
    use alloc::borrow::Cow;
    #[cfg(feature = "encode")]
    use alloc::vec;
    #[cfg(any(feature = "encode", feature = "decode"))]
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
    fn fidelity_targets_roundtrip() {
        use zencodec::encode::{EncoderConfig, Fidelity};

        // Native modular lossless.
        let ll = JxlEncoderConfig::new().with_fidelity(Fidelity::Lossless);
        assert_eq!(ll.resolved_target_fidelity(), Some(Fidelity::Lossless));
        assert_eq!(ll.is_lossless(), Some(true));

        // Native VarDCT butteraugli distance round-trips as itself.
        let bt = JxlEncoderConfig::new().with_fidelity(Fidelity::butteraugli(2.0));
        assert_eq!(
            bt.resolved_target_fidelity(),
            Some(Fidelity::butteraugli(2.0))
        );
        assert_eq!(bt.is_lossless(), Some(false));
        assert!(bt.lossy_config().is_some());

        // Codec quality dial round-trips as itself.
        let cq = JxlEncoderConfig::new().with_fidelity(Fidelity::codec_quality(80.0));
        assert_eq!(
            cq.resolved_target_fidelity(),
            Some(Fidelity::codec_quality(80.0))
        );

        // No native SSIM2 in jxl: the score maps onto the quality dial and is
        // reported as codec_quality (honest that no SSIM2 convergence happened).
        let s2 = JxlEncoderConfig::new().with_fidelity(Fidelity::ssim2(90.0));
        assert_eq!(
            s2.resolved_target_fidelity(),
            Some(Fidelity::codec_quality(90.0))
        );
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

    #[cfg(feature = "encode")]
    #[test]
    fn supported_descriptors_includes_rgbx_and_bgrx() {
        use zencodec::encode::EncoderConfig;
        let desc = JxlEncoderConfig::supported_descriptors();
        assert!(
            desc.contains(&zenpixels::PixelDescriptor::RGBX8_SRGB),
            "RGBX8_SRGB must be in supported_descriptors"
        );
        assert!(
            desc.contains(&zenpixels::PixelDescriptor::BGRX8_SRGB),
            "BGRX8_SRGB must be in supported_descriptors"
        );
    }

    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn encode_rgbx8_roundtrip() {
        use zencodec::decode::Decode;
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{PixelDescriptor, PixelSlice};

        let w = 16u32;
        let h = 16u32;
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&[255, 128, 0, 0x13]);
        }
        let slice =
            PixelSlice::new(&buf, w, h, (w * 4) as usize, PixelDescriptor::RGBX8_SRGB).unwrap();

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.data().is_empty());

        // Verify decode back to RGB (no alpha channel expected).
        use zencodec::decode::{DecodeJob, DecoderConfig};
        let dec = JxlDecoderConfig::new();
        let decoder = dec
            .job()
            .decoder(Cow::Borrowed(output.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        assert_eq!(decoded.width(), w);
        assert_eq!(decoded.height(), h);
    }

    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn encode_bgrx8_roundtrip() {
        use zencodec::decode::Decode;
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{PixelDescriptor, PixelSlice};

        let w = 16u32;
        let h = 16u32;
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            // BGR order with padding: B=0, G=128, R=255, pad
            buf.extend_from_slice(&[0, 128, 255, 0x42]);
        }
        let slice =
            PixelSlice::new(&buf, w, h, (w * 4) as usize, PixelDescriptor::BGRX8_SRGB).unwrap();

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(slice.erase()).unwrap();
        assert!(!output.data().is_empty());

        use zencodec::decode::{DecodeJob, DecoderConfig};
        let dec = JxlDecoderConfig::new();
        let decoder = dec
            .job()
            .decoder(Cow::Borrowed(output.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        assert_eq!(decoded.width(), w);
        assert_eq!(decoded.height(), h);
    }

    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn encode_rgbx8_smaller_than_rgba() {
        // RGBX8 encodes as 3-channel RGB; an RGBA8 encode of the same data
        // with non-opaque alpha bytes stored in byte 3 would be larger or
        // different. Confirm RGBX output matches RGB output byte-for-byte.
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{PixelDescriptor, PixelSlice};

        let w = 16u32;
        let h = 16u32;
        let mut rgbx = Vec::with_capacity((w * h * 4) as usize);
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for i in 0..(w * h) {
            let r = (i & 0xff) as u8;
            let g = ((i >> 1) & 0xff) as u8;
            let b = ((i >> 2) & 0xff) as u8;
            rgbx.extend_from_slice(&[r, g, b, 0x55]);
            rgb.extend_from_slice(&[r, g, b]);
        }

        let rgbx_slice =
            PixelSlice::new(&rgbx, w, h, (w * 4) as usize, PixelDescriptor::RGBX8_SRGB).unwrap();
        let rgb_slice =
            PixelSlice::new(&rgb, w, h, (w * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();

        let rgbx_out = JxlEncoderConfig::new()
            .with_lossless(true)
            .job()
            .encoder()
            .unwrap()
            .encode(rgbx_slice.erase())
            .unwrap();
        let rgb_out = JxlEncoderConfig::new()
            .with_lossless(true)
            .job()
            .encoder()
            .unwrap()
            .encode(rgb_slice.erase())
            .unwrap();

        assert_eq!(
            rgbx_out.data(),
            rgb_out.data(),
            "RGBX8 must encode identically to RGB8 (padding byte stripped)"
        );
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

    #[cfg(all(feature = "encode", feature = "decode", feature = "zencodec"))]
    #[test]
    fn metadata_cicp_round_trips_via_enum_color() {
        use zencodec::decode::{DecodeJob, DecoderConfig};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zencodec::{Cicp, Metadata};

        let width = 16u32;
        let height = 16u32;
        let pixels: Vec<rgb::Rgb<u8>> = (0..width * height)
            .map(|i| {
                let v = (i % 256) as u8;
                rgb::Rgb {
                    r: v,
                    g: 0,
                    b: 255 - v,
                }
            })
            .collect();
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        // Encode with a Display-P3 CICP in the metadata (no ICC). It must drive
        // the codestream enum color encoding (the JXL CICP fix).
        let meta = Metadata::none().with_cicp(Cicp::DISPLAY_P3);
        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config
            .job()
            .with_metadata_policy(meta, zencodec::MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        // The P3 CICP must survive via the codestream enum color.
        let info = JxlDecoderConfig::new().job().probe(output.data()).unwrap();
        assert_eq!(
            info.source_color.cicp,
            Some(Cicp::DISPLAY_P3),
            "Display-P3 CICP should round-trip through the JXL codestream enum color"
        );
    }

    /// A BT.2100-PQ source re-tags the decode descriptor with PQ transfer +
    /// BT.2020 primaries (native HDR signaling) at both probe (`output_info`)
    /// and full-decode level. The decoder renders into the signaled encoding,
    /// so the descriptor must stop claiming sRGB for HDR streams.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn decode_descriptor_carries_cicp_pq_hdr() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zencodec::{Cicp, Metadata};
        use zenpixels::{ColorPrimaries, TransferFunction};

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

        let meta = Metadata::none().with_cicp(Cicp::BT2100_PQ);
        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config
            .job()
            .with_metadata_policy(meta, zencodec::MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        // Probe level: output_info's native format carries PQ + BT.2020.
        let oi = JxlDecoderConfig::new()
            .job()
            .output_info(output.data())
            .unwrap();
        assert_eq!(oi.native_format.transfer(), TransferFunction::Pq);
        assert_eq!(oi.native_format.primaries, ColorPrimaries::Bt2020);

        // Full decode: the buffer descriptor carries PQ + BT.2020 (the
        // decoder rendered into the signaled encoding).
        let decoded = JxlDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(output.data()), &[])
            .unwrap()
            .decode()
            .unwrap();
        let desc = decoded.into_buffer().descriptor();
        assert_eq!(desc.transfer(), TransferFunction::Pq);
        assert_eq!(desc.primaries, ColorPrimaries::Bt2020);
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

    /// Forcing test for the **envelope** error pattern (Pattern B): driving the
    /// decoder through `DynDecoderConfig` erases the error to `BoxedError`
    /// (`Box<dyn Error + Send + Sync>`), yet a generic consumer still recovers the
    /// codec-agnostic [`ErrorCategory`] *and* the originating codec name from the
    /// [`CodecError`] envelope. Under Pattern A (`type Error = At<JxlError>`) both
    /// recoveries would be `None` after erasure — there would be no `CodecError`
    /// to downcast to. This is the whole reason the trait impls return the
    /// envelope rather than the typed error.
    #[cfg(feature = "decode")]
    #[test]
    fn dyn_dispatch_preserves_category_and_codec_through_erasure() {
        use zencodec::decode::DynDecoderConfig;
        use zencodec::{CodecError, CodecErrorExt, ErrorCategory};

        // Bad JXL magic, long enough to clear any "insufficient data for header"
        // guard: the decoder reads the (invalid) signature and rejects the
        // bitstream as malformed rather than asking for more input.
        let malformed = [0xABu8; 256];

        let dyn_cfg: &dyn DynDecoderConfig = &JxlDecoderConfig::new();
        let erased = dyn_cfg
            .dyn_job()
            .probe(&malformed)
            .expect_err("malformed JXL must fail to probe");

        // The coarse category survives erasure to a plain `Box<dyn Error>`.
        assert_eq!(
            erased.error_category(),
            Some(ErrorCategory::MalformedImage),
            "bad JXL magic must categorize as MalformedImage through dyn dispatch"
        );
        // ...and so does the codec name, so a consumer can tell codecs apart.
        assert_eq!(
            erased.codec_error().and_then(CodecError::codec),
            Some("zenjxl"),
            "the originating codec must be recoverable through dyn dispatch"
        );
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
        assert!(caps.gain_map());
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
        let expected_max = if cfg!(feature = "threads") {
            u16::MAX
        } else {
            1
        };
        assert_eq!(caps.threads_supported_range(), (1, expected_max));
        assert!(caps.exif());
        assert!(caps.xmp());
        assert!(caps.gain_map());
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

        let limits = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
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

        let limits = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);

        // Encode with single thread
        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().with_limits(limits).encoder().unwrap();
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
    fn animation_frame_decoder_info_before_render() {
        use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};
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
            .animation_frame_decoder(Cow::Borrowed(output.data()), &[])
            .unwrap();

        // info() should return valid data without panicking.
        let info = ffd.info();
        assert_eq!(info.width, width);
        assert_eq!(info.height, height);
        assert_eq!(info.format, ImageFormat::Jxl);
    }

    /// H2 (audit): animation decode must gate accumulated retained-frame
    /// memory against ResourceLimits.max_memory_bytes.
    ///
    /// We encode a 4-frame animation, then ask the animation decoder to
    /// render under a memory cap that's too tight to hold even the first
    /// decoded frame. The first call to `render_next_frame` must error.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn animation_decode_respects_max_memory_bytes() {
        use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};
        use zencodec::encode::{AnimationFrameEncoder, EncodeJob, EncoderConfig};
        use zencodec::{ResourceLimits, ThreadingPolicy};
        use zenpixels::{PixelDescriptor, PixelSlice};

        // Encode a 4-frame 8x8 RGB animation with lossless to keep deps low.
        let width = 8u32;
        let height = 8u32;
        let stride = (width as usize) * 3;
        let frame_pixels = stride * height as usize;
        let make_frame = |seed: u8| -> Vec<u8> {
            (0..frame_pixels)
                .map(|i| seed.wrapping_add(i as u8))
                .collect()
        };
        let limits_for_encode = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
        let config = JxlEncoderConfig::new().with_lossless(true);
        let mut enc = config
            .job()
            .with_limits(limits_for_encode)
            .animation_frame_encoder()
            .unwrap();
        for seed in 0u8..4 {
            let frame = make_frame(seed * 17);
            let slice =
                PixelSlice::new(&frame, width, height, stride, PixelDescriptor::RGB8_SRGB).unwrap();
            enc.push_frame(slice, 100, None).unwrap();
        }
        let encoded = AnimationFrameEncoder::finish(enc, None).unwrap();

        // Decode with a max_memory_bytes cap below one frame's retained bytes
        // (8 * 8 * 4 = 256 for RGBA8 native output; cap at 128).
        let dec_limits = ResourceLimits::none()
            .with_threading(ThreadingPolicy::Sequential)
            .with_max_memory(128);
        let dec_config = JxlDecoderConfig::new();
        let mut ffd = dec_config
            .job()
            .with_limits(dec_limits)
            .animation_frame_decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap();

        // First render call triggers decode_all_frames and must fail with
        // a LimitExceeded error from the new accumulated_bytes gate.
        let result = ffd.render_next_frame(None);
        assert!(
            result.is_err(),
            "animation render under tight memory cap must error, got Ok"
        );
    }

    /// H2 (audit): animation decode must also gate frame count via
    /// ResourceLimits.max_frames. Encode 3 frames, cap to 2, expect error.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn animation_decode_respects_max_frames() {
        use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};
        use zencodec::encode::{AnimationFrameEncoder, EncodeJob, EncoderConfig};
        use zencodec::{ResourceLimits, ThreadingPolicy};
        use zenpixels::{PixelDescriptor, PixelSlice};

        let width = 4u32;
        let height = 4u32;
        let stride = (width as usize) * 3;
        let frame_pixels = stride * height as usize;
        let make_frame = |seed: u8| -> Vec<u8> {
            (0..frame_pixels)
                .map(|i| seed.wrapping_add(i as u8))
                .collect()
        };
        let enc_limits = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
        let config = JxlEncoderConfig::new().with_lossless(true);
        let mut enc = config
            .job()
            .with_limits(enc_limits)
            .animation_frame_encoder()
            .unwrap();
        for seed in 0u8..3 {
            let frame = make_frame(seed * 23);
            let slice =
                PixelSlice::new(&frame, width, height, stride, PixelDescriptor::RGB8_SRGB).unwrap();
            enc.push_frame(slice, 50, None).unwrap();
        }
        let encoded = AnimationFrameEncoder::finish(enc, None).unwrap();

        let dec_limits = ResourceLimits::none()
            .with_threading(ThreadingPolicy::Sequential)
            .with_max_frames(2);
        let dec_config = JxlDecoderConfig::new();
        let mut ffd = dec_config
            .job()
            .with_limits(dec_limits)
            .animation_frame_decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap();
        let result = ffd.render_next_frame(None);
        assert!(
            result.is_err(),
            "animation render with 3 frames under max_frames=2 must error"
        );
    }

    /// Encode with gain map via zencodec trait → decode → verify gain map roundtrips.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn gain_map_roundtrip_via_trait() {
        use crate::GainMapBundle;
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        // Build a small base image.
        let width = 8u32;
        let height = 8u32;
        let pixels: Vec<rgb::Rgb<u8>> = (0..width * height)
            .map(|i| {
                let v = (i % 256) as u8;
                rgb::Rgb { r: v, g: v, b: v }
            })
            .collect();
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        // Build a fake gain map bundle.
        // The gain map codestream doesn't need to be valid JXL for this test —
        // the decoder just stores the raw bytes without parsing them.
        let fake_metadata = vec![0x01, 0x02, 0x03, 0x04];
        let fake_gain_map_codestream = vec![0xFF, 0x0A, 0xDE, 0xAD, 0xBE, 0xEF];
        let bundle = GainMapBundle {
            metadata: fake_metadata.clone(),
            color_encoding: None,
            alt_icc_compressed: None,
            gain_map_codestream: fake_gain_map_codestream.clone(),
        };
        let jhgm_payload = bundle.serialize();

        // Encode with gain map attached.
        let config = JxlEncoderConfig::new()
            .with_lossless(true)
            .with_gain_map(jhgm_payload);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        // The output should be in container format (not bare codestream).
        assert!(
            jxl_encoder::container::is_container(output.data()),
            "output with gain map should be in container format"
        );

        // Decode with jxl-rs and verify gain map roundtripped.
        let decode_result = crate::decode::decode(output.data(), None, &[]).unwrap();
        let decoded_gm = decode_result
            .gain_map
            .expect("decoded output should contain a gain map");
        assert_eq!(
            decoded_gm.metadata, fake_metadata,
            "gain map metadata should roundtrip"
        );
        assert_eq!(
            decoded_gm.gain_map_codestream, fake_gain_map_codestream,
            "gain map codestream should roundtrip"
        );
        assert!(
            decoded_gm.color_encoding.is_none(),
            "color_encoding should be None"
        );
        assert!(
            decoded_gm.alt_icc_compressed.is_none(),
            "alt_icc should be None"
        );
    }

    /// Encode with gain map via native encode API → decode → verify gain map roundtrips.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn gain_map_roundtrip_native() {
        use crate::GainMapBundle;
        use imgref::Img;

        // Build a small base image.
        let width = 8u32;
        let height = 8u32;
        let pixels: Vec<rgb::Rgb<u8>> = (0..width * height)
            .map(|i| {
                let v = (i % 256) as u8;
                rgb::Rgb { r: v, g: v, b: v }
            })
            .collect();
        let img = Img::new(pixels, width as usize, height as usize);

        // Encode losslessly (produces bare codestream).
        let config = jxl_encoder::LosslessConfig::default();
        let encoded =
            jxl_encoder::convenience::encode_rgb8_lossless(img.as_ref(), &config).unwrap();

        // Build a gain map bundle and append it.
        let bundle = GainMapBundle {
            metadata: vec![0xAA, 0xBB],
            color_encoding: Some(vec![0xCC, 0xDD]),
            alt_icc_compressed: None,
            gain_map_codestream: vec![0xFF, 0x0A, 0x01, 0x02],
        };
        let with_gm = jxl_encoder::container::append_gain_map_box(&encoded, &bundle.serialize());

        // Should now be container format.
        assert!(jxl_encoder::container::is_container(&with_gm));

        // Decode and verify gain map.
        let decode_result = crate::decode::decode(&with_gm, None, &[]).unwrap();
        let decoded_gm = decode_result
            .gain_map
            .expect("decoded output should contain a gain map");
        assert_eq!(decoded_gm.metadata, vec![0xAA, 0xBB]);
        assert_eq!(decoded_gm.color_encoding, Some(vec![0xCC, 0xDD]));
        assert!(decoded_gm.alt_icc_compressed.is_none());
        assert_eq!(decoded_gm.gain_map_codestream, vec![0xFF, 0x0A, 0x01, 0x02]);
    }

    /// Encoding without gain map should not produce container format.
    #[cfg(feature = "encode")]
    #[test]
    fn no_gain_map_stays_bare_codestream() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 4u32;
        let height = 4u32;
        let pixels: Vec<rgb::Rgb<u8>> = vec![rgb::Rgb { r: 0, g: 0, b: 0 }; 16];
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        // No gain map, no metadata → bare codestream.
        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        assert!(
            !jxl_encoder::container::is_container(output.data()),
            "output without gain map should be a bare codestream"
        );
    }

    /// GainMapBundle serialize→parse roundtrip (independent of encode/decode).
    #[cfg(feature = "decode")]
    #[test]
    fn gain_map_bundle_serialize_parse_roundtrip() {
        use crate::GainMapBundle;

        let bundle = GainMapBundle {
            metadata: vec![0x10, 0x20, 0x30],
            color_encoding: Some(vec![0xAA, 0xBB]),
            alt_icc_compressed: Some(vec![0xCC; 64]),
            gain_map_codestream: vec![0xFF, 0x0A, 0x00, 0x01],
        };

        let serialized = bundle.serialize();
        let parsed = GainMapBundle::parse(&serialized).unwrap();
        assert_eq!(bundle, parsed);
    }

    // ── EXIF / XMP metadata roundtrip tests ────────────────────────────

    /// Encode with EXIF → decode → verify EXIF bytes roundtrip through JxlInfo.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn exif_roundtrip() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 8u32;
        let height = 8u32;
        let pixels: Vec<rgb::Rgb<u8>> = vec![rgb::Rgb { r: 0, g: 0, b: 0 }; 64];
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        // Minimal EXIF blob (starts with byte order marker, just enough to be recognizable)
        let exif_data: &[u8] = b"MM\x00\x2a\x00\x00\x00\x08\x00\x00";
        let meta = zencodec::Metadata::none().with_exif(exif_data);

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config
            .job()
            .with_metadata_policy(meta, zencodec::MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        // Output must be container format (EXIF is stored in a container box).
        assert!(
            jxl_encoder::container::is_container(output.data()),
            "output with EXIF should be in container format"
        );

        // Decode and verify EXIF roundtripped.
        let result = crate::decode::decode(output.data(), None, &[]).unwrap();
        let decoded_exif = result
            .info
            .exif
            .expect("decoded output should contain EXIF");
        assert_eq!(
            decoded_exif, exif_data,
            "EXIF data should roundtrip exactly"
        );
    }

    /// Encode with XMP → decode → verify XMP bytes roundtrip through JxlInfo.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn xmp_roundtrip() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 8u32;
        let height = 8u32;
        let pixels: Vec<rgb::Rgb<u8>> = vec![rgb::Rgb { r: 0, g: 0, b: 0 }; 64];
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        let xmp_data: &[u8] = b"<?xml version=\"1.0\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>";
        let meta = zencodec::Metadata::none().with_xmp(xmp_data);

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config
            .job()
            .with_metadata_policy(meta, zencodec::MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        assert!(
            jxl_encoder::container::is_container(output.data()),
            "output with XMP should be in container format"
        );

        let result = crate::decode::decode(output.data(), None, &[]).unwrap();
        let decoded_xmp = result.info.xmp.expect("decoded output should contain XMP");
        assert_eq!(
            decoded_xmp.as_slice(),
            xmp_data,
            "XMP data should roundtrip exactly"
        );
    }

    /// Encode with both EXIF and XMP → decode → verify both roundtrip.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn exif_and_xmp_roundtrip() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 4u32;
        let height = 4u32;
        let pixels: Vec<rgb::Rgb<u8>> = vec![rgb::Rgb { r: 0, g: 0, b: 0 }; 16];
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        let exif_data: &[u8] = b"MM\x00\x2a\x00\x00\x00\x08\x00\x01";
        let xmp_data: &[u8] = b"<xmp>test</xmp>";
        let meta = zencodec::Metadata::none()
            .with_exif(exif_data)
            .with_xmp(xmp_data);

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config
            .job()
            .with_metadata_policy(meta, zencodec::MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        let result = crate::decode::decode(output.data(), None, &[]).unwrap();
        assert_eq!(
            result.info.exif.as_deref(),
            Some(exif_data),
            "EXIF should roundtrip"
        );
        assert_eq!(
            result.info.xmp.as_deref(),
            Some(xmp_data),
            "XMP should roundtrip"
        );
    }

    /// Bare codestream (no container) should return None for EXIF/XMP.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn bare_codestream_no_metadata() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 4u32;
        let height = 4u32;
        let pixels: Vec<rgb::Rgb<u8>> = vec![rgb::Rgb { r: 0, g: 0, b: 0 }; 16];
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        // No metadata → bare codestream.
        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        // Confirm it's a bare codestream.
        assert!(!jxl_encoder::container::is_container(output.data()));

        let result = crate::decode::decode(output.data(), None, &[]).unwrap();
        assert!(
            result.info.exif.is_none(),
            "bare codestream should have no EXIF"
        );
        assert!(
            result.info.xmp.is_none(),
            "bare codestream should have no XMP"
        );
    }

    /// ICC profile roundtrip (encode with structured sRGB → decode → verify ICC present).
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn icc_from_structured_color() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 4u32;
        let height = 4u32;
        let pixels: Vec<rgb::Rgb<u8>> = vec![rgb::Rgb { r: 0, g: 0, b: 0 }; 16];
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        let result = crate::decode::decode(output.data(), None, &[]).unwrap();
        // JXL with structured sRGB color should synthesize an ICC profile.
        assert!(
            result.info.icc_profile.is_some(),
            "sRGB image should have a synthesized ICC profile"
        );
    }

    /// EXIF/XMP wired through zencodec ImageInfo in the trait-based decode path.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn exif_xmp_in_image_info() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

        let width = 8u32;
        let height = 8u32;
        let pixels: Vec<rgb::Rgb<u8>> = vec![rgb::Rgb { r: 0, g: 0, b: 0 }; 64];
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, width, height).unwrap();

        let exif_data: &[u8] = b"MM\x00\x2a\x00\x00\x00\x08\x00\x00";
        let xmp_data: &[u8] = b"<xmp>hi</xmp>";
        let meta = zencodec::Metadata::none()
            .with_exif(exif_data)
            .with_xmp(xmp_data);

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config
            .job()
            .with_metadata_policy(meta, zencodec::MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap();
        let output = encoder.encode(buf.as_slice().into()).unwrap();

        // Decode via zencodec trait path.
        let dec_config = JxlDecoderConfig::new();
        let decoder = dec_config
            .job()
            .decoder(Cow::Borrowed(output.data()), &[])
            .unwrap();
        let decoded = decoder.decode().unwrap();
        let info = decoded.info();
        assert_eq!(
            info.embedded_metadata.exif.as_deref(),
            Some(exif_data),
            "EXIF should be accessible via ImageInfo"
        );
        assert_eq!(
            info.embedded_metadata.xmp.as_deref(),
            Some(xmp_data),
            "XMP should be accessible via ImageInfo"
        );
    }

    /// The `AllocPreference` override must not change decoded bytes: `Fallible`,
    /// `Infallible`, and the default (`CodecDefault`) decode of the same JXL
    /// produce byte-identical output. Only the allocation *strategy* differs.
    #[cfg(all(feature = "encode", feature = "decode"))]
    #[test]
    fn fallible_alloc_decode_matches_default() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zencodec::{AllocPreference, ResourceLimits};

        // Lossless RGBA8 so the decoded bytes are deterministic.
        let width = 48u32;
        let height = 40u32;
        let pixels: Vec<rgb::Rgba<u8>> = (0..width * height)
            .map(|i| rgb::Rgba {
                r: (i % 256) as u8,
                g: ((i / 3) % 256) as u8,
                b: ((i / 7) % 256) as u8,
                a: 255,
            })
            .collect();
        let buf =
            zenpixels::PixelBuffer::<rgb::Rgba<u8>>::from_pixels(pixels, width, height).unwrap();

        let config = JxlEncoderConfig::new().with_lossless(true);
        let encoder = config.job().encoder().unwrap();
        let encoded = encoder.encode(buf.as_slice().into()).unwrap();

        let decode_with = |pref: Option<AllocPreference>| -> Vec<u8> {
            let dec_config = JxlDecoderConfig::new();
            let mut job = dec_config.job();
            if let Some(p) = pref {
                job = job.with_limits(ResourceLimits::none().with_prefer_fallible_allocations(p));
            }
            let decoder = job.decoder(Cow::Borrowed(encoded.data()), &[]).unwrap();
            let decoded = decoder.decode().unwrap();
            decoded.pixels().contiguous_bytes().into_owned()
        };

        let default_bytes = decode_with(None);
        let codec_default_bytes = decode_with(Some(AllocPreference::CodecDefault));
        let fallible_bytes = decode_with(Some(AllocPreference::Fallible));
        let infallible_bytes = decode_with(Some(AllocPreference::Infallible));

        assert!(!default_bytes.is_empty());
        assert_eq!(
            default_bytes, codec_default_bytes,
            "CodecDefault must match no-limits default"
        );
        assert_eq!(
            default_bytes, fallible_bytes,
            "Fallible must match default bytes"
        );
        assert_eq!(
            default_bytes, infallible_bytes,
            "Infallible must match default bytes"
        );
    }

    /// `estimate_decode_resources` reports a non-trivial peak (output buffer +
    /// working set + overhead), a serial threading model, and a peak that
    /// grows with image area.
    #[cfg(feature = "decode")]
    #[test]
    fn estimate_decode_resources_is_sane() {
        use zencodec::decode::DecoderConfig;
        use zencodec::estimate::{ComputeEnvironment, ImageCharacteristics};

        let cfg = JxlDecoderConfig::new();
        let compute = ComputeEnvironment::new();
        let small = ImageCharacteristics::new(256, 256, zenpixels::PixelDescriptor::RGBA8_SRGB);
        let large = ImageCharacteristics::new(2048, 2048, zenpixels::PixelDescriptor::RGBA8_SRGB);

        let es = cfg.estimate_decode_resources(&small, &compute);
        let el = cfg.estimate_decode_resources(&large, &compute);

        // Peak must exceed the bare output buffer (W*H*4) — there is a working
        // set + fixed overhead on top.
        let small_output = 256u64 * 256 * 4;
        assert!(
            es.peak_memory_bytes_est().unwrap() > small_output,
            "peak must exceed the bare output buffer"
        );
        // Larger image → larger peak and at-least-as-large wall time.
        assert!(
            el.peak_memory_bytes_est().unwrap() > es.peak_memory_bytes_est().unwrap(),
            "peak must grow with area"
        );
        assert!(el.wall_ms().unwrap() >= es.wall_ms().unwrap());
    }
}
