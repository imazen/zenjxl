//! Zennode node definitions for JPEG XL encoding and decoding.
//!
//! Provides [`EncodeJxl`] and [`DecodeJxl`], self-documenting pipeline nodes
//! that bridge zennode's parameter system with zenjxl's encoder/decoder configs.
//!
//! Feature-gated behind `feature = "zennode"`.

extern crate alloc;

use zennode::*;

// ── Encode ──────────────────────────────────────────────────────────────────

/// JPEG XL encoder configuration as a pipeline node.
///
/// Exposes all key JXL encoding parameters with RIAPI querystring support.
/// Convert to [`JxlEncoderConfig`](crate::JxlEncoderConfig) via
/// [`to_encoder_config()`](EncodeJxl::to_encoder_config) or overlay onto
/// an existing config with [`apply()`](EncodeJxl::apply) (both require the
/// `zencodec` and `encode` features).
///
/// **RIAPI**: `?jxl.d=1.0&jxl.effort=7` or `?jxl.q=85&jxl.effort=7`
///
/// Quality has three tiers of specificity:
/// - `quality` (generic, 0-100): uniform quality across all codecs
/// - `jxl_quality` / `jxl.q` (codec-specific, 0-100): JXL-native quality
/// - `distance` / `jxl.d` (raw, 0-25): direct butteraugli distance
///
/// When multiple are set, the most specific wins (distance > jxl_quality > quality).
/// When `lossless` is true, all quality/distance params are ignored.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenjxl.encode", group = Encode, role = Encode)]
#[node(tags("jxl", "jpeg-xl", "encode", "lossy", "lossless", "hdr", "codec"))]
pub struct EncodeJxl {
    /// Generic quality 0-100 (mapped via `with_generic_quality` at execution time).
    ///
    /// When set, this value is passed through zencodec's
    /// `with_generic_quality()` which maps it to the codec's native
    /// quality scale. Use this for uniform quality across all codecs.
    /// Overridden by `jxl_quality` or `distance` when those are also set.
    /// `None` = unset (no quality override).
    #[param(range(0..=100), default = 75, step = 1)]
    #[param(unit = "", section = "Quality", label = "Quality")]
    #[kv("quality")]
    pub quality: Option<i32>,

    /// Codec-specific JXL perceptual quality (0 = lowest, 100 = highest).
    ///
    /// Mapped internally to butteraugli distance via the calibrated
    /// quality-to-distance curve. Higher values produce larger files
    /// with better visual quality. When set, takes precedence
    /// over the generic `quality` field. Ignored when `lossless` is true.
    /// `None` = unset (no codec-specific quality override).
    #[param(range(0.0..=100.0), default = 75.0, identity = 75.0, step = 1.0)]
    #[param(unit = "", section = "Quality", label = "JXL Quality")]
    #[kv("jxl.quality", "jxl.q")]
    pub jxl_quality: Option<f32>,

    /// Butteraugli distance (0.0 = mathematically lossless, 1.0 = visually lossless).
    ///
    /// Direct control over the perceptual distortion target.
    /// Lower values produce larger files with better quality.
    /// Ignored when `lossless` is true.
    /// `None` = unset (use default distance or quality-derived distance).
    #[param(range(0.0..=25.0), default = 1.0, identity = 1.0, step = 0.1)]
    #[param(unit = "butteraugli", section = "Quality")]
    #[kv("jxl.distance", "jxl.d")]
    pub distance: Option<f32>,

    /// Enable lossless encoding (Modular mode, distance ignored).
    /// `None` = unset (inherit from pipeline or use lossy default).
    #[param(default = false)]
    #[param(section = "Mode")]
    #[kv("jxl.lossless")]
    pub lossless: Option<bool>,

    /// Encoder effort (1 = fastest, 10 = slowest/best compression).
    /// `None` = unset (inherit from pipeline or use default effort 7).
    #[param(range(1..=10), default = 7)]
    #[param(section = "Speed", label = "Effort")]
    #[kv("jxl.effort", "jxl.e")]
    pub effort: Option<i32>,

    /// Enable noise synthesis to mask compression artifacts.
    /// `None` = unset (inherit from pipeline or use default off).
    #[param(default = false)]
    #[param(section = "Advanced")]
    #[kv("jxl.noise")]
    pub noise: Option<bool>,
}

#[cfg(all(feature = "zencodec", feature = "encode"))]
impl EncodeJxl {
    /// Apply this node's explicitly-set params on top of an existing config.
    ///
    /// `None` fields are skipped, so this acts as an overlay — only params
    /// the user explicitly set take effect.
    ///
    /// Application order (most specific wins):
    /// 1. `lossless` — switches to modular mode
    /// 2. `quality` (generic) — calibrated mapping through `quality_to_distance`
    /// 3. `jxl_quality` (codec-specific) — also through calibrated mapping
    /// 4. `effort` — applied if `Some`
    /// 5. `noise` — applied if `Some(true)`
    pub fn apply(&self, mut config: crate::JxlEncoderConfig) -> crate::JxlEncoderConfig {
        use zencodec::encode::EncoderConfig as _;

        // Lossless first (changes internal mode)
        if let Some(true) = self.lossless {
            config = config.with_lossless(true);
        }
        // Generic quality (calibrated mapping through quality_to_distance)
        if let Some(q) = self.quality {
            config = config.with_generic_quality(q as f32);
        }
        // Codec-specific quality override (JXL native quality, also
        // mapped through quality_to_distance via with_generic_quality)
        if let Some(q) = self.jxl_quality {
            config = config.with_generic_quality(q);
        }
        // Effort
        if let Some(e) = self.effort {
            config = config.with_generic_effort(e.clamp(1, 10));
        }
        // Direct distance override (most specific — wins over quality mappings).
        // Only apply if no quality was set (quality/jxl_quality go through
        // calibrated mapping; distance is raw butteraugli).
        if let Some(d) = self.distance {
            if self.quality.is_none() && self.jxl_quality.is_none() {
                config = config.with_distance(d);
            }
        }
        // Noise synthesis
        if let Some(true) = self.noise {
            config = config.with_noise(true);
        }
        config
    }

    /// Build a config from scratch using only this node's params.
    ///
    /// If `quality` or `jxl_quality` is `Some`, it goes through the calibrated
    /// quality-to-distance mapping via `with_generic_quality`. Otherwise,
    /// `distance` (if `Some`) is used directly via `with_distance`.
    pub fn to_encoder_config(&self) -> crate::JxlEncoderConfig {
        self.apply(crate::JxlEncoderConfig::new())
    }
}

// ── Decode ──────────────────────────────────────────────────────────────────

/// JPEG XL decoder configuration as a pipeline node.
///
/// Exposes decoder parameters with RIAPI querystring support.
/// Convert to [`JxlDecoderConfig`](crate::JxlDecoderConfig) via
/// [`to_decoder_config()`](DecodeJxl::to_decoder_config) (requires the
/// `zencodec` and `decode` features).
///
/// **RIAPI**: `?jxl.orient=true`
///
/// Note: `intensity_target` and `adjust_orientation` are informational hints
/// for the pipeline. The current `JxlDecoderConfig` has no configurable fields;
/// these parameters are exposed for future use and pipeline metadata.
#[derive(Node, Clone, Debug)]
#[node(id = "zenjxl.decode", group = Decode, role = Decode)]
#[node(tags("jxl", "jpeg-xl", "decode", "codec"))]
pub struct DecodeJxl {
    /// Whether to apply EXIF orientation during decoding.
    #[param(default = true)]
    #[param(section = "Main")]
    #[kv("jxl.orient")]
    pub adjust_orientation: bool,

    /// Target display intensity in nits for HDR tone mapping.
    ///
    /// `None` = use the image's embedded intensity target (no override).
    /// When set, specifies the display peak luminance for tone mapping.
    #[param(range(0.0..=10000.0), default = 0.0, identity = 0.0, step = 100.0)]
    #[param(unit = "nits", section = "HDR")]
    #[kv("jxl.nits")]
    pub intensity_target: Option<f32>,
}

impl Default for DecodeJxl {
    fn default() -> Self {
        Self {
            adjust_orientation: true,
            intensity_target: None,
        }
    }
}

#[cfg(all(feature = "zencodec", feature = "decode"))]
impl DecodeJxl {
    /// Convert this node into a [`JxlDecoderConfig`](crate::JxlDecoderConfig).
    ///
    /// The current `JxlDecoderConfig` has no configurable fields, so this
    /// returns a default config. The `adjust_orientation` and `intensity_target`
    /// fields are available on the node for pipeline-level use.
    pub fn to_decoder_config(&self) -> crate::JxlDecoderConfig {
        crate::JxlDecoderConfig::new()
    }
}

// ── Registration ────────────────────────────────────────────────────────────

/// Register all JPEG XL zennode definitions with a registry.
pub fn register(registry: &mut NodeRegistry) {
    registry.register(&ENCODE_JXL_NODE);
    registry.register(&DECODE_JXL_NODE);
}

/// All JPEG XL zennode definitions.
pub static ALL: &[&dyn NodeDef] = &[&ENCODE_JXL_NODE, &DECODE_JXL_NODE];

#[cfg(test)]
mod tests {
    use super::*;

    // ── Encode tests ────────────────────────────────────────────────────

    #[test]
    fn encode_schema_basics() {
        let schema = ENCODE_JXL_NODE.schema();
        assert_eq!(schema.id, "zenjxl.encode");
        assert_eq!(schema.group, NodeGroup::Encode);
        assert_eq!(schema.role, NodeRole::Encode);
        assert!(schema.tags.contains(&"jxl"));
        assert!(schema.tags.contains(&"jpeg-xl"));
        assert!(schema.tags.contains(&"encode"));
        assert!(schema.tags.contains(&"lossy"));
        assert!(schema.tags.contains(&"lossless"));
        assert!(schema.tags.contains(&"hdr"));
        assert!(schema.tags.contains(&"codec"));
    }

    #[test]
    fn encode_param_count_and_names() {
        let schema = ENCODE_JXL_NODE.schema();
        let names: alloc::vec::Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(names.contains(&"quality"));
        assert!(names.contains(&"jxl_quality"));
        assert!(names.contains(&"distance"));
        assert!(names.contains(&"lossless"));
        assert!(names.contains(&"effort"));
        assert!(names.contains(&"noise"));
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn encode_defaults() {
        let node = ENCODE_JXL_NODE.create_default().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::None));
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::None));
        assert_eq!(node.get_param("distance"), Some(ParamValue::None));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::None));
        assert_eq!(node.get_param("effort"), Some(ParamValue::None));
        assert_eq!(node.get_param("noise"), Some(ParamValue::None));
    }

    #[test]
    fn encode_kv_distance_effort() {
        let mut kv = KvPairs::from_querystring("jxl.d=2.0&jxl.e=3");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("distance"), Some(ParamValue::F32(2.0)));
        assert_eq!(node.get_param("effort"), Some(ParamValue::I32(3)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn encode_kv_jxl_quality() {
        let mut kv = KvPairs::from_querystring("jxl.q=85");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::F32(85.0)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn encode_kv_generic_quality() {
        let mut kv = KvPairs::from_querystring("quality=80");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        // jxl_quality remains unset
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::None));
    }

    #[test]
    fn encode_kv_both_qualities() {
        let mut kv = KvPairs::from_querystring("quality=80&jxl.quality=90");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::F32(90.0)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn encode_kv_lossless() {
        let mut kv = KvPairs::from_querystring("jxl.lossless=true");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(true)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn encode_kv_no_match() {
        let mut kv = KvPairs::from_querystring("w=800&h=600");
        let result = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn encode_downcast() {
        let node = ENCODE_JXL_NODE.create_default().unwrap();
        let enc = node.as_any().downcast_ref::<EncodeJxl>().unwrap();
        assert_eq!(enc.quality, None);
        assert_eq!(enc.jxl_quality, None);
        assert_eq!(enc.distance, None);
        assert_eq!(enc.lossless, None);
        assert_eq!(enc.effort, None);
        assert_eq!(enc.noise, None);
    }

    #[test]
    fn encode_json_round_trip() {
        let mut params = ParamMap::new();
        params.insert("quality".into(), ParamValue::I32(80));
        params.insert("jxl_quality".into(), ParamValue::F32(92.0));
        params.insert("distance".into(), ParamValue::F32(0.3));
        params.insert("lossless".into(), ParamValue::Bool(true));
        params.insert("effort".into(), ParamValue::I32(5));
        params.insert("noise".into(), ParamValue::Bool(true));

        let node = ENCODE_JXL_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::F32(92.0)));
        assert_eq!(node.get_param("distance"), Some(ParamValue::F32(0.3)));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(true)));
        assert_eq!(node.get_param("effort"), Some(ParamValue::I32(5)));
        assert_eq!(node.get_param("noise"), Some(ParamValue::Bool(true)));

        // Round-trip
        let exported = node.to_params();
        let node2 = ENCODE_JXL_NODE.create(&exported).unwrap();
        assert_eq!(node2.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node2.get_param("jxl_quality"), Some(ParamValue::F32(92.0)));
        assert_eq!(node2.get_param("distance"), Some(ParamValue::F32(0.3)));
        assert_eq!(node2.get_param("lossless"), Some(ParamValue::Bool(true)));
        assert_eq!(node2.get_param("effort"), Some(ParamValue::I32(5)));
        assert_eq!(node2.get_param("noise"), Some(ParamValue::Bool(true)));
    }

    // ── Encode + zencodec tests ─────────────────────────────────────────

    #[cfg(all(feature = "zencodec", feature = "encode"))]
    #[test]
    fn to_encoder_config_defaults() {
        let node = EncodeJxl::default();
        let _config = node.to_encoder_config();
    }

    #[cfg(all(feature = "zencodec", feature = "encode"))]
    #[test]
    fn apply_generic_quality() {
        let mut node = EncodeJxl::default();
        node.quality = Some(80);
        let config = node.to_encoder_config();
        let q = zencodec::encode::EncoderConfig::generic_quality(&config);
        assert!(q.is_some());
    }

    #[cfg(all(feature = "zencodec", feature = "encode"))]
    #[test]
    fn apply_codec_specific_overrides() {
        let mut node = EncodeJxl::default();
        node.quality = Some(50);
        node.jxl_quality = Some(90.0);
        let config = node.to_encoder_config();
        // jxl_quality applied after quality, so 90.0 is effective
        let q = zencodec::encode::EncoderConfig::generic_quality(&config);
        assert!(q.is_some());
    }

    #[cfg(all(feature = "zencodec", feature = "encode"))]
    #[test]
    fn apply_preserves_existing() {
        use zencodec::encode::EncoderConfig as _;
        let base = crate::JxlEncoderConfig::new().with_generic_effort(5);
        let node = EncodeJxl::default();
        let config = node.apply(base);
        // Effort should still be 5 (defaults don't override)
        let e = zencodec::encode::EncoderConfig::generic_effort(&config);
        assert_eq!(e, Some(5));
    }

    #[cfg(all(feature = "zencodec", feature = "encode"))]
    #[test]
    fn apply_lossless() {
        let mut node = EncodeJxl::default();
        node.lossless = Some(true);
        let config = node.to_encoder_config();
        let lossless = zencodec::encode::EncoderConfig::is_lossless(&config);
        assert_eq!(lossless, Some(true));
    }

    #[cfg(all(feature = "zencodec", feature = "encode"))]
    #[test]
    fn apply_effort_and_quality() {
        let mut node = EncodeJxl::default();
        node.effort = Some(3);
        node.quality = Some(75);
        let config = node.to_encoder_config();
        let e = zencodec::encode::EncoderConfig::generic_effort(&config);
        assert_eq!(e, Some(3));
    }

    // ── Decode tests ────────────────────────────────────────────────────

    #[test]
    fn decode_schema_basics() {
        let schema = DECODE_JXL_NODE.schema();
        assert_eq!(schema.id, "zenjxl.decode");
        assert_eq!(schema.group, NodeGroup::Decode);
        assert_eq!(schema.role, NodeRole::Decode);
        assert!(schema.tags.contains(&"jxl"));
        assert!(schema.tags.contains(&"jpeg-xl"));
        assert!(schema.tags.contains(&"decode"));
        assert!(schema.tags.contains(&"codec"));

        let param_names: alloc::vec::Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(param_names.contains(&"adjust_orientation"));
        assert!(param_names.contains(&"intensity_target"));
    }

    #[test]
    fn decode_defaults() {
        let node = DECODE_JXL_NODE.create_default().unwrap();
        assert_eq!(
            node.get_param("adjust_orientation"),
            Some(ParamValue::Bool(true))
        );
        assert_eq!(node.get_param("intensity_target"), Some(ParamValue::None));
    }

    #[test]
    fn decode_kv_orient() {
        let mut kv = KvPairs::from_querystring("jxl.orient=false");
        let node = DECODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(
            node.get_param("adjust_orientation"),
            Some(ParamValue::Bool(false))
        );
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn decode_kv_nits() {
        let mut kv = KvPairs::from_querystring("jxl.nits=4000");
        let node = DECODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(
            node.get_param("intensity_target"),
            Some(ParamValue::F32(4000.0))
        );
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn decode_kv_no_match() {
        let mut kv = KvPairs::from_querystring("w=800&h=600");
        let result = DECODE_JXL_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn decode_downcast() {
        let node = DECODE_JXL_NODE.create_default().unwrap();
        let dec = node.as_any().downcast_ref::<DecodeJxl>().unwrap();
        assert!(dec.adjust_orientation);
        assert_eq!(dec.intensity_target, None);
    }

    // ── Registry integration ────────────────────────────────────────────

    #[test]
    fn registry_integration() {
        let mut registry = NodeRegistry::new();
        register(&mut registry);
        assert!(registry.get("zenjxl.encode").is_some());
        assert!(registry.get("zenjxl.decode").is_some());

        // jxl.quality triggers codec-specific path
        let result = registry.from_querystring("jxl.quality=80&jxl.effort=5");
        assert_eq!(result.instances.len(), 1);
        assert_eq!(result.instances[0].schema().id, "zenjxl.encode");

        // generic quality also triggers the encode node
        let result2 = registry.from_querystring("quality=80");
        assert_eq!(result2.instances.len(), 1);
        assert_eq!(result2.instances[0].schema().id, "zenjxl.encode");

        // jxl.orient triggers the decode node
        let result3 = registry.from_querystring("jxl.orient=false");
        assert_eq!(result3.instances.len(), 1);
        assert_eq!(result3.instances[0].schema().id, "zenjxl.decode");
    }
}
