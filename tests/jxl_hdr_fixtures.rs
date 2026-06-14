//! HDR decode validated against real cjxl (libjxl) fixtures.
//!
//! These JXL files come from the reference encoder (libjxl `cjxl` v0.11.2), so
//! they check that zenjxl reads the **standard** codestream HDR signaling —
//! `intensity_target` and the CICP color encoding — from real-world output, not
//! from its own encoder. libjxl writes no mastering-display / content-light box
//! (`cjxl -v -v --help` exposes only `--intensity_target`), and JXL defines no
//! standard carrier for the SMPTE ST 2086 mastering volume, so we don't
//! fabricate one.
//!
//! Regenerate the fixtures:
//! ```text
//! convert -size 16x16 gradient:red-blue -type TrueColor input.ppm
//! cjxl input.ppm hdr_pq_4000.jxl  --intensity_target=4000 -x color_space=RGB_D65_202_Rel_PeQ -d 0
//! cjxl input.ppm hdr_hlg_1000.jxl --intensity_target=1000 -x color_space=RGB_D65_202_Rel_HLG -d 0
//! ```

// `probe` (and the decode path it exercises) is gated behind `decode`; without
// this the no-decode CI feature builds fail to resolve `zenjxl::probe`.
#![cfg(feature = "decode")]

use std::path::PathBuf;
use zenjxl::probe;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("missing committed fixture {}: {e}", path.display()))
}

/// A real `cjxl`-produced BT.2100-PQ file: the probe reports the encoded
/// `intensity_target` (4000 nits) and a BT.2020 + PQ CICP.
#[test]
fn cjxl_pq_fixture_intensity_target_and_cicp() {
    let info = probe(&fixture("hdr_pq_4000.jxl")).expect("probe cjxl PQ fixture");
    assert_eq!(
        info.intensity_target, 4000.0,
        "cjxl --intensity_target=4000 must round-trip through the codestream"
    );
    let (cp, tc, _mc, _full) = info.cicp.expect("PQ fixture must carry CICP");
    assert_eq!(cp, 9, "BT.2020 primaries (CICP color_primaries 9)");
    assert_eq!(tc, 16, "PQ transfer (CICP transfer_characteristics 16)");
}

/// A real `cjxl`-produced BT.2100-HLG file: peak 1000 nits, HLG transfer,
/// BT.2020 primaries.
#[test]
fn cjxl_hlg_fixture_intensity_target_and_cicp() {
    let info = probe(&fixture("hdr_hlg_1000.jxl")).expect("probe cjxl HLG fixture");
    assert_eq!(info.intensity_target, 1000.0);
    let (cp, tc, _mc, _full) = info.cicp.expect("HLG fixture must carry CICP");
    assert_eq!(cp, 9, "BT.2020 primaries");
    assert_eq!(tc, 18, "HLG transfer (CICP transfer_characteristics 18)");
}
