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
/// [`to_encoder_config()`](EncodeJxl::to_encoder_config) (requires the
/// `zencodec` and `encode` features).
///
/// **RIAPI**: `?jxl.d=1.0&jxl.e=7` or `?jxl.q=85&jxl.e=7`
///
/// When `quality` is >= 0, it takes priority over `distance` and goes through
/// the calibrated quality-to-distance mapping. When `quality` is negative
/// (the default -1), `distance` is used directly.
#[derive(Node, Clone, Debug)]
#[node(id = "zenjxl.encode", group = Encode, role = Encode)]
#[node(tags("jxl", "jpeg-xl", "encode", "lossy", "lossless", "hdr"))]
pub struct EncodeJxl {
    /// Butteraugli distance (0.0 = mathematically lossless, 1.0 = visually lossless).
    #[param(range(0.0..=25.0), default = 1.0, step = 0.1)]
    #[param(unit = "butteraugli", section = "Quality")]
    #[kv("jxl.distance", "jxl.d")]
    pub distance: f32,

    /// Calibrated quality on a 0-100 scale (like libjpeg-turbo).
    ///
    /// When >= 0, overrides `distance` and goes through the calibrated
    /// quality-to-distance curve. Set to -1 to use `distance` directly.
    #[param(range(-1.0..=100.0), default = -1.0, identity = -1.0, step = 1.0)]
    #[param(section = "Quality", label = "Quality (calibrated)")]
    #[kv("jxl.quality", "jxl.q")]
    pub quality: f32,

    /// Enable lossless encoding (Modular mode, distance ignored).
    #[param(default = false)]
    #[param(section = "Mode")]
    #[kv("jxl.lossless")]
    pub lossless: bool,

    /// Encoder effort (1 = fastest, 10 = slowest/best compression).
    #[param(range(1..=10), default = 7)]
    #[param(section = "Speed", label = "Effort")]
    #[kv("jxl.effort", "jxl.e")]
    pub effort: i32,

    /// Enable noise synthesis to mask compression artifacts.
    #[param(default = false)]
    #[param(section = "Advanced")]
    #[kv("jxl.noise")]
    pub noise: bool,
}

impl Default for EncodeJxl {
    fn default() -> Self {
        Self {
            distance: 1.0,
            quality: -1.0,
            lossless: false,
            effort: 7,
            noise: false,
        }
    }
}

#[cfg(all(feature = "zencodec", feature = "encode"))]
impl EncodeJxl {
    /// Convert this node into a [`JxlEncoderConfig`](crate::JxlEncoderConfig).
    ///
    /// If `quality` >= 0, it takes priority and goes through the calibrated
    /// quality-to-distance mapping via `with_generic_quality`. Otherwise,
    /// `distance` is used directly via `with_distance`.
    pub fn to_encoder_config(&self) -> crate::JxlEncoderConfig {
        use zencodec::encode::EncoderConfig;

        let mut config = crate::JxlEncoderConfig::new();

        if self.lossless {
            config = config.with_lossless(true);
        } else if self.quality >= 0.0 {
            config = config.with_generic_quality(self.quality);
        } else {
            config = config.with_distance(self.distance);
        }

        config = config.with_generic_effort(self.effort);

        if self.noise {
            config = config.with_noise(true);
        }

        config
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
#[node(tags("jxl", "jpeg-xl", "decode"))]
pub struct DecodeJxl {
    /// Whether to apply EXIF orientation during decoding.
    #[param(default = true)]
    #[param(section = "Main")]
    #[kv("jxl.orient")]
    pub adjust_orientation: bool,

    /// Target display intensity in nits for HDR tone mapping.
    ///
    /// 0 = use the image's embedded intensity target (no override).
    /// Values > 0 specify the display peak luminance for tone mapping.
    #[param(range(0.0..=10000.0), default = 0.0, identity = 0.0, step = 100.0)]
    #[param(unit = "nits", section = "HDR")]
    #[kv("jxl.nits")]
    pub intensity_target: f32,
}

impl Default for DecodeJxl {
    fn default() -> Self {
        Self {
            adjust_orientation: true,
            intensity_target: 0.0,
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

#[cfg(test)]
mod tests {
    use super::*;

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

        let param_names: alloc::vec::Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(param_names.contains(&"distance"));
        assert!(param_names.contains(&"quality"));
        assert!(param_names.contains(&"lossless"));
        assert!(param_names.contains(&"effort"));
        assert!(param_names.contains(&"noise"));
    }

    #[test]
    fn encode_defaults() {
        let node = ENCODE_JXL_NODE.create_default().unwrap();
        assert_eq!(node.get_param("distance"), Some(ParamValue::F32(1.0)));
        assert_eq!(node.get_param("quality"), Some(ParamValue::F32(-1.0)));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(false)));
        assert_eq!(node.get_param("effort"), Some(ParamValue::I32(7)));
        assert_eq!(node.get_param("noise"), Some(ParamValue::Bool(false)));
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
    fn encode_kv_quality() {
        let mut kv = KvPairs::from_querystring("jxl.q=85");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::F32(85.0)));
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
        assert_eq!(enc.distance, 1.0);
        assert_eq!(enc.quality, -1.0);
        assert!(!enc.lossless);
        assert_eq!(enc.effort, 7);
        assert!(!enc.noise);
    }

    #[test]
    fn decode_schema_basics() {
        let schema = DECODE_JXL_NODE.schema();
        assert_eq!(schema.id, "zenjxl.decode");
        assert_eq!(schema.group, NodeGroup::Decode);
        assert_eq!(schema.role, NodeRole::Decode);
        assert!(schema.tags.contains(&"jxl"));
        assert!(schema.tags.contains(&"jpeg-xl"));
        assert!(schema.tags.contains(&"decode"));

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
        assert_eq!(
            node.get_param("intensity_target"),
            Some(ParamValue::F32(0.0))
        );
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
        assert_eq!(dec.intensity_target, 0.0);
    }
}
