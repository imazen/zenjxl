//! zennode node definitions for JPEG XL encoding.
//!
//! Defines [`EncodeJxl`] with RIAPI-compatible querystring keys
//! for JPEG XL encoding parameters.

use zennode::*;

/// JPEG XL encoding with quality, effort, distance, and lossless options.
///
/// Supports both perceptual quality (0-100 scale) and direct butteraugli
/// distance control. When `lossless` is true, both `quality` and `distance`
/// are ignored.
///
/// JSON API: `{ "quality": 75, "effort": 7, "distance": 1.0, "lossless": false }`
/// RIAPI: `?jxl.quality=75&jxl.effort=7&jxl.distance=1.0&jxl.lossless=false`
#[derive(Node, Clone, Debug)]
#[node(id = "zenjxl.encode", group = Encode, role = Encode)]
#[node(tags("codec", "jxl", "lossy", "lossless", "encode", "hdr"))]
pub struct EncodeJxl {
    /// Generic quality 0-100 (mapped via with_generic_quality at execution time).
    ///
    /// When set (>= 0), this value is passed through zencodec's
    /// `with_generic_quality()` which maps it to the codec's native
    /// quality scale. Use this for uniform quality across all codecs.
    #[param(range(0..=100), default = -1, step = 1)]
    #[param(unit = "", section = "Main", label = "Quality")]
    #[kv("quality")]
    pub quality: i32,

    /// Codec-specific JXL perceptual quality (0 = lowest, 100 = highest).
    ///
    /// Mapped internally to butteraugli distance. Higher values
    /// produce larger files with better visual quality. Ignored
    /// when `lossless` is true.
    /// When set (>= 0), takes precedence over the generic `quality` field.
    #[param(range(0.0..=100.0), default = -1.0, identity = 75.0, step = 1.0)]
    #[param(unit = "", section = "Main", label = "JXL Quality")]
    #[kv("jxl.quality")]
    pub jxl_quality: f32,

    /// Encoder effort (1 = fastest, 9 = slowest/best compression).
    ///
    /// Higher values use more CPU time for better compression ratios.
    /// Effort 7 is a good default balancing speed and compression.
    #[param(range(1..=9), default = 7, step = 1)]
    #[param(unit = "", section = "Main", label = "Effort")]
    #[kv("jxl.effort")]
    pub effort: i32,

    /// Butteraugli distance (0 = mathematically lossless, 25 = very lossy).
    ///
    /// Direct control over the perceptual distortion target.
    /// Lower values produce larger files with better quality.
    /// Distance 1.0 is visually lossless for most content.
    /// Ignored when `lossless` is true.
    #[param(range(0.0..=25.0), default = 1.0, identity = 1.0, step = 0.1)]
    #[param(unit = "", section = "Advanced", label = "Distance")]
    #[kv("jxl.distance")]
    pub distance: f32,

    /// Use mathematically lossless encoding.
    ///
    /// When true, the output is a bit-exact reconstruction of the
    /// input. Both `quality` and `distance` are ignored in this mode.
    #[param(default = false)]
    #[param(section = "Main")]
    #[kv("jxl.lossless")]
    pub lossless: bool,
}

impl Default for EncodeJxl {
    fn default() -> Self {
        Self {
            quality: -1,
            jxl_quality: -1.0,
            effort: 7,
            distance: 1.0,
            lossless: false,
        }
    }
}

/// Register all JPEG XL zennode definitions with a registry.
pub fn register(registry: &mut NodeRegistry) {
    registry.register(&ENCODE_JXL_NODE);
}

/// All JPEG XL zennode definitions.
pub static ALL: &[&dyn NodeDef] = &[&ENCODE_JXL_NODE];

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn schema_metadata() {
        let schema = ENCODE_JXL_NODE.schema();
        assert_eq!(schema.id, "zenjxl.encode");
        assert_eq!(schema.group, NodeGroup::Encode);
        assert_eq!(schema.role, NodeRole::Encode);
        assert!(schema.tags.contains(&"jxl"));
        assert!(schema.tags.contains(&"lossy"));
        assert!(schema.tags.contains(&"lossless"));
        assert!(schema.tags.contains(&"codec"));
        assert!(schema.tags.contains(&"encode"));
        assert!(schema.tags.contains(&"hdr"));
    }

    #[test]
    fn param_count_and_names() {
        let schema = ENCODE_JXL_NODE.schema();
        let names: Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(names.contains(&"quality"));
        assert!(names.contains(&"jxl_quality"));
        assert!(names.contains(&"effort"));
        assert!(names.contains(&"distance"));
        assert!(names.contains(&"lossless"));
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn defaults() {
        let node = ENCODE_JXL_NODE.create_default().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(-1)));
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::F32(-1.0)));
        assert_eq!(node.get_param("effort"), Some(ParamValue::I32(7)));
        assert_eq!(node.get_param("distance"), Some(ParamValue::F32(1.0)));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(false)));
    }

    #[test]
    fn from_kv_jxl_quality() {
        let mut kv = KvPairs::from_querystring("jxl.quality=90&jxl.lossless=false");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::F32(90.0)));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(false)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn from_kv_generic_quality() {
        let mut kv = KvPairs::from_querystring("quality=80");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        // jxl_quality remains unset
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::F32(-1.0)));
    }

    #[test]
    fn from_kv_both_qualities() {
        let mut kv = KvPairs::from_querystring("quality=80&jxl.quality=90");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::F32(90.0)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn from_kv_effort() {
        let mut kv = KvPairs::from_querystring("jxl.effort=9");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("effort"), Some(ParamValue::I32(9)));
    }

    #[test]
    fn from_kv_distance() {
        let mut kv = KvPairs::from_querystring("jxl.distance=0.5");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("distance"), Some(ParamValue::F32(0.5)));
    }

    #[test]
    fn from_kv_lossless() {
        let mut kv = KvPairs::from_querystring("jxl.lossless=true");
        let node = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(true)));
    }

    #[test]
    fn from_kv_no_match() {
        let mut kv = KvPairs::from_querystring("w=800&h=600");
        let result = ENCODE_JXL_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn json_round_trip() {
        let mut params = ParamMap::new();
        params.insert("quality".into(), ParamValue::I32(80));
        params.insert("jxl_quality".into(), ParamValue::F32(92.0));
        params.insert("effort".into(), ParamValue::I32(5));
        params.insert("distance".into(), ParamValue::F32(0.3));
        params.insert("lossless".into(), ParamValue::Bool(true));

        let node = ENCODE_JXL_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node.get_param("jxl_quality"), Some(ParamValue::F32(92.0)));
        assert_eq!(node.get_param("effort"), Some(ParamValue::I32(5)));
        assert_eq!(node.get_param("distance"), Some(ParamValue::F32(0.3)));
        assert_eq!(node.get_param("lossless"), Some(ParamValue::Bool(true)));

        // Round-trip
        let exported = node.to_params();
        let node2 = ENCODE_JXL_NODE.create(&exported).unwrap();
        assert_eq!(node2.get_param("quality"), Some(ParamValue::I32(80)));
        assert_eq!(node2.get_param("jxl_quality"), Some(ParamValue::F32(92.0)));
        assert_eq!(node2.get_param("effort"), Some(ParamValue::I32(5)));
        assert_eq!(node2.get_param("distance"), Some(ParamValue::F32(0.3)));
        assert_eq!(node2.get_param("lossless"), Some(ParamValue::Bool(true)));
    }

    #[test]
    fn downcast_to_concrete() {
        let node = ENCODE_JXL_NODE.create_default().unwrap();
        let enc = node.as_any().downcast_ref::<EncodeJxl>().unwrap();
        assert_eq!(enc.quality, -1);
        assert!((enc.jxl_quality - (-1.0)).abs() < f32::EPSILON);
        assert_eq!(enc.effort, 7);
        assert!((enc.distance - 1.0).abs() < f32::EPSILON);
        assert!(!enc.lossless);
    }

    #[test]
    fn registry_integration() {
        let mut registry = NodeRegistry::new();
        register(&mut registry);
        assert!(registry.get("zenjxl.encode").is_some());

        // jxl.quality triggers codec-specific path
        let result = registry.from_querystring("jxl.quality=80&jxl.effort=5");
        assert_eq!(result.instances.len(), 1);
        assert_eq!(result.instances[0].schema().id, "zenjxl.encode");

        // generic quality also triggers the node
        let result2 = registry.from_querystring("quality=80");
        assert_eq!(result2.instances.len(), 1);
        assert_eq!(result2.instances[0].schema().id, "zenjxl.encode");
    }
}
