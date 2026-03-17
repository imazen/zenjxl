//! JXL container format utilities for gain map embedding.
//!
//! Provides functions to detect container-format JXL, and assemble containers
//! with `jhgm` boxes for ISO 21496-1 gain maps.
//!
//! The JXL container format uses ISOBMFF-style boxes:
//! ```text
//! [JXL signature (12 bytes)] [ftyp (20 bytes)] [jxlc | jxlp...] [Exif?] [xml?] [jhgm?]
//! ```

use alloc::vec::Vec;

/// JXL container signature: 12-byte box with type `JXL ` and magic `0x0D0A870A`.
const JXL_CONTAINER_SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, // box size = 12
    b'J', b'X', b'L', b' ', // box type
    0x0D, 0x0A, 0x87, 0x0A, // JXL container magic
];

/// ftyp box: brand = "jxl ", minor_version = 0, compatible = "jxl ".
const FTYP_BOX: [u8; 20] = [
    0x00, 0x00, 0x00, 0x14, // box size = 20
    b'f', b't', b'y', b'p', // box type
    b'j', b'x', b'l', b' ', // major brand
    0x00, 0x00, 0x00, 0x00, // minor version
    b'j', b'x', b'l', b' ', // compatible brand
];

/// Returns `true` if `data` begins with the JXL container signature.
///
/// JXL files may be either bare codestreams (starting with `0xFF0A`) or
/// container-format files (starting with the 12-byte ISOBMFF signature box).
pub fn is_container(data: &[u8]) -> bool {
    data.len() >= 12 && data[..12] == JXL_CONTAINER_SIGNATURE
}

/// Returns `true` if `data` begins with the bare JXL codestream signature (`0xFF0A`).
pub fn is_bare_codestream(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && data[1] == 0x0A
}

/// Write an ISOBMFF box: `[u32 BE size][4-byte type][payload]`.
///
/// For payloads > ~4GB, uses extended 64-bit box header (size field = 1,
/// followed by 8-byte extended size).
fn write_box(out: &mut Vec<u8>, box_type: &[u8; 4], payload: &[u8]) {
    let total_size = 8u64 + payload.len() as u64;
    if total_size <= u32::MAX as u64 {
        out.extend_from_slice(&(total_size as u32).to_be_bytes());
        out.extend_from_slice(box_type);
    } else {
        let extended_size = 16u64 + payload.len() as u64;
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(box_type);
        out.extend_from_slice(&extended_size.to_be_bytes());
    }
    out.extend_from_slice(payload);
}

/// Append a `jhgm` box to JXL data (container or bare codestream).
///
/// `jhgm_payload` is the output of [`GainMapBundle::serialize()`][crate::GainMapBundle::serialize].
///
/// If `jxl_data` is already a container (starts with JXL signature), the
/// `jhgm` box is appended at the end. If `jxl_data` is a bare codestream,
/// it is first wrapped in a container (signature + ftyp + jxlc), then the
/// jhgm box is appended.
pub fn append_gain_map_box(jxl_data: &[u8], jhgm_payload: &[u8]) -> Vec<u8> {
    if is_container(jxl_data) {
        // Already a container — append jhgm box at the end.
        let jhgm_box_size = 8 + jhgm_payload.len();
        let mut out = Vec::with_capacity(jxl_data.len() + jhgm_box_size);
        out.extend_from_slice(jxl_data);
        write_box(&mut out, b"jhgm", jhgm_payload);
        out
    } else {
        // Bare codestream — wrap in container first, then append jhgm.
        let header_size = JXL_CONTAINER_SIGNATURE.len() + FTYP_BOX.len();
        let jxlc_size = 8 + jxl_data.len();
        let jhgm_box_size = 8 + jhgm_payload.len();
        let total = header_size + jxlc_size + jhgm_box_size;

        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&JXL_CONTAINER_SIGNATURE);
        out.extend_from_slice(&FTYP_BOX);
        write_box(&mut out, b"jxlc", jxl_data);
        write_box(&mut out, b"jhgm", jhgm_payload);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_container() {
        assert!(is_container(&JXL_CONTAINER_SIGNATURE));
        assert!(!is_container(&[0xFF, 0x0A, 0x00]));
        assert!(!is_container(&[0x00; 4]));
    }

    #[test]
    fn test_is_bare_codestream() {
        assert!(is_bare_codestream(&[0xFF, 0x0A]));
        assert!(is_bare_codestream(&[0xFF, 0x0A, 0x00, 0x01]));
        assert!(!is_bare_codestream(&[0xFF, 0x0B]));
        assert!(!is_bare_codestream(&[0x00]));
    }

    #[test]
    fn test_append_gain_map_to_bare_codestream() {
        let codestream = b"\xFF\x0A\x00\x01\x02\x03";
        let jhgm_payload = b"\x00\x00\x03\x01\x02\x03\x00\x00\x00\x00\x00\xFF\x0A";

        let result = append_gain_map_box(codestream, jhgm_payload);

        // Should start with container signature
        assert!(is_container(&result));
        assert_eq!(&result[..12], &JXL_CONTAINER_SIGNATURE);
        assert_eq!(&result[12..32], &FTYP_BOX);

        // jxlc box at offset 32
        let jxlc_size = u32::from_be_bytes([result[32], result[33], result[34], result[35]]);
        assert_eq!(jxlc_size as usize, 8 + codestream.len());
        assert_eq!(&result[36..40], b"jxlc");
        assert_eq!(&result[40..40 + codestream.len()], codestream);

        // jhgm box follows jxlc
        let jhgm_offset = 40 + codestream.len();
        let jhgm_size = u32::from_be_bytes([
            result[jhgm_offset],
            result[jhgm_offset + 1],
            result[jhgm_offset + 2],
            result[jhgm_offset + 3],
        ]);
        assert_eq!(jhgm_size as usize, 8 + jhgm_payload.len());
        assert_eq!(&result[jhgm_offset + 4..jhgm_offset + 8], b"jhgm");
        assert_eq!(&result[jhgm_offset + 8..], jhgm_payload);
    }

    #[test]
    fn test_append_gain_map_to_existing_container() {
        // Build a minimal container
        let codestream = b"\xFF\x0A\x00";
        let mut container = Vec::new();
        container.extend_from_slice(&JXL_CONTAINER_SIGNATURE);
        container.extend_from_slice(&FTYP_BOX);
        write_box(&mut container, b"jxlc", codestream);

        let jhgm_payload = b"\x00\x00\x01\xAA\x00\x00\x00\x00\x00\xFF\x0A";
        let result = append_gain_map_box(&container, jhgm_payload);

        // Original container bytes preserved
        assert_eq!(&result[..container.len()], container.as_slice());

        // jhgm box appended at the end
        let jhgm_offset = container.len();
        assert_eq!(&result[jhgm_offset + 4..jhgm_offset + 8], b"jhgm");
        assert_eq!(&result[jhgm_offset + 8..], jhgm_payload);
    }

    #[test]
    fn test_write_box_small() {
        let mut out = Vec::new();
        write_box(&mut out, b"test", b"hello");
        assert_eq!(out.len(), 8 + 5);
        let size = u32::from_be_bytes([out[0], out[1], out[2], out[3]]);
        assert_eq!(size, 13);
        assert_eq!(&out[4..8], b"test");
        assert_eq!(&out[8..], b"hello");
    }

    #[test]
    fn test_write_box_empty_payload() {
        let mut out = Vec::new();
        write_box(&mut out, b"emty", b"");
        assert_eq!(out.len(), 8);
        let size = u32::from_be_bytes([out[0], out[1], out[2], out[3]]);
        assert_eq!(size, 8);
        assert_eq!(&out[4..8], b"emty");
    }
}
