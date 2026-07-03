//! Integration tests for `zenjxl::lossless_verify` — proves the self-verified
//! lossless encode round-trips exactly (zero tolerance for pixel corruption)
//! for both the single-encode (high color count) and multi-candidate
//! (low color count) paths, and that the multi-candidate path never produces
//! a LARGER result than the single default choice it's meant to improve on.
//!
//! Run: cargo test -p zenjxl --features encode --test lossless_verify
#![cfg(all(feature = "encode", feature = "decode"))]

use imgref::{Img, ImgRef};
use rgb::Rgb;
use zenjxl::LosslessConfig;
use zenjxl::lossless_verify::encode_rgb8_lossless_verified;

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

/// High color count (a smooth gradient over a large palette): must take the
/// single-encode path and still round-trip exactly.
#[test]
fn high_color_count_roundtrips_losslessly() {
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
/// must take the multi-candidate path and still round-trip exactly.
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

/// The multi-candidate path (low color count) must never produce a result
/// LARGER than a naive single fixed-effort encode -- it's a strict "keep the
/// smallest of several candidates" guarantee, not a probabilistic one.
#[test]
fn low_color_count_never_worse_than_naive_single_encode() {
    let (w, h) = (128usize, 128usize);
    // A pattern specifically chosen to be low-color-count (2 colors) but with
    // fine spatial structure (checkerboard), the kind of content the module
    // docs identify as having a jagged effort x predictor RD landscape.
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

    let naive_cfg = LosslessConfig::new().with_effort(10);
    let naive = jxl_encoder::convenience::encode_rgb8_lossless(img.as_ref(), &naive_cfg)
        .expect("naive single-config encode");

    assert!(
        verified.len() <= naive.len(),
        "verified path ({} bytes) must be <= naive single-config encode ({} bytes)",
        verified.len(),
        naive.len()
    );
}
