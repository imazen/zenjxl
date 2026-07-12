//! Integration tests for `zenjxl::lossless_verify` — proves the self-verified
//! lossless encode round-trips exactly (zero tolerance for pixel corruption)
//! for both the single-encode (typical content) and multi-candidate
//! (low-color / near-gray content) branches, and that the multi-candidate
//! branch never produces a LARGER result than its own lean member (the lean
//! cell is a member of the rich candidate set, so this is a strict
//! keep-the-smallest guarantee, not a probabilistic one).
//!
//! Run: cargo test -p zenjxl --features encode,decode,__expert --test lossless_verify
#![cfg(all(feature = "encode", feature = "decode", feature = "__expert"))]

use imgref::{Img, ImgRef};
use rgb::Rgb;
use zenjxl::lossless_verify::encode_rgb8_lossless_verified;
use zenjxl::sweep::{BuiltConfig, variant_from_cell_id};

fn decode_rgb8(cs: &[u8]) -> (Vec<Rgb<u8>>, u32, u32) {
    let out = zenjxl::decode(cs, None, &[zenpixels::PixelDescriptor::RGB8])
        .expect("decode verified-lossless output");
    let raw = out.pixels.into_vec();
    let pixels: Vec<Rgb<u8>> = raw
        .chunks_exact(3)
        .map(|c| Rgb {
            r: c[0],
            g: c[1],
            b: c[2],
        })
        .collect();
    (pixels, out.info.width, out.info.height)
}

fn assert_lossless_roundtrip(img: ImgRef<'_, Rgb<u8>>) -> Vec<u8> {
    let encoded = encode_rgb8_lossless_verified(img).expect("encode_rgb8_lossless_verified");
    let (decoded, w, h) = decode_rgb8(&encoded);
    assert_eq!(w as usize, img.width(), "width must round-trip exactly");
    assert_eq!(h as usize, img.height(), "height must round-trip exactly");
    let original: Vec<Rgb<u8>> = img.pixels().collect();
    assert_eq!(
        decoded, original,
        "lossless encode must round-trip pixels EXACTLY -- zero tolerance for corruption"
    );
    encoded
}

/// Typical content (a chromatic gradient over a large palette, far from
/// gray): takes the single lean-encode branch and must round-trip exactly.
#[test]
fn typical_content_roundtrips_losslessly() {
    let (w, h) = (64usize, 64usize);
    let pixels: Vec<Rgb<u8>> = (0..w * h)
        .map(|i| {
            let x = (i % w) as u8;
            let y = (i / w) as u8;
            Rgb {
                r: x.wrapping_mul(7),
                g: y.wrapping_mul(11),
                b: x.wrapping_add(y).wrapping_mul(3),
            }
        })
        .collect();
    let img = Img::new(pixels, w, h);
    assert_lossless_roundtrip(img.as_ref());
}

/// Low color count (a 3-color flat-region image, well under the 256 cap):
/// takes the multi-candidate branch and must round-trip exactly.
#[test]
fn low_color_count_roundtrips_losslessly() {
    let (w, h) = (64usize, 64usize);
    let colors = [
        Rgb {
            r: 10,
            g: 10,
            b: 10,
        },
        Rgb {
            r: 200,
            g: 50,
            b: 50,
        },
        Rgb {
            r: 50,
            g: 200,
            b: 50,
        },
    ];
    let pixels: Vec<Rgb<u8>> = (0..w * h).map(|i| colors[i % colors.len()]).collect();
    let img = Img::new(pixels, w, h);
    assert_lossless_roundtrip(img.as_ref());
}

/// Near-grayscale content with MANY distinct colors (gradient of grays with
/// tiny per-channel jitter): the >256-distinct-colors path alone would miss
/// it; the grayscale side of the gate must route it to the multi-candidate
/// branch (this is exactly the mid-size-rescaled-plot family the 2026-07-12
/// sweep found slipping through a colors-only gate). Round-trip proof is the
/// same either way; this test exists so a regression in the gray gate shows
/// up as a branch change in coverage, not silently.
#[test]
fn near_gray_many_colors_roundtrips_losslessly() {
    let (w, h) = (96usize, 96usize);
    let pixels: Vec<Rgb<u8>> = (0..w * h)
        .map(|i| {
            let v = (i % 251) as u8;
            // spread <= 2, well inside the gray tolerance, but thousands of
            // distinct (r,g,b) triples.
            Rgb {
                r: v,
                g: v.saturating_add((i % 3) as u8),
                b: v.saturating_add((i % 2) as u8),
            }
        })
        .collect();
    let img = Img::new(pixels, w, h);
    assert_lossless_roundtrip(img.as_ref());
}

/// The multi-candidate branch must never produce a result LARGER than its
/// own lean member encoded alone -- the lean cell is a member of the rich
/// candidate set and the branch keeps the byte-smallest candidate.
#[test]
fn gated_branch_never_worse_than_lean_member() {
    let (w, h) = (128usize, 128usize);
    // 2-color checkerboard: low color count -> multi-candidate branch, with
    // the fine spatial structure the module docs identify as the jagged-RD
    // regime.
    let pixels: Vec<Rgb<u8>> = (0..w * h)
        .map(|i| {
            let x = i % w;
            let y = i / w;
            if (x + y) % 2 == 0 {
                Rgb { r: 0, g: 0, b: 0 }
            } else {
                Rgb {
                    r: 255,
                    g: 255,
                    b: 255,
                }
            }
        })
        .collect();
    let img = Img::new(pixels, w, h);

    let verified = assert_lossless_roundtrip(img.as_ref());

    let variant = variant_from_cell_id("mod-e9_lloyd-pal0").expect("lean cell id parses");
    let BuiltConfig::Lossless(cfg) = variant.build() else {
        panic!("lean cell must be lossless");
    };
    let lean = jxl_encoder::convenience::encode_rgb8_lossless(img.as_ref(), &cfg)
        .expect("lean member encode");

    assert!(
        verified.len() <= lean.len(),
        "verified branch ({} bytes) must be <= its lean member alone ({} bytes)",
        verified.len(),
        lean.len()
    );
}
