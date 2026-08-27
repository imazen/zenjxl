// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! The `lean` sweep stratum (`EncoderStrategy::LeanFaster`) must not be
//! inert: on screenshot-class content it has to produce different bytes
//! from the `Zenjxl` default at the harness's own cells.
//!
//! Background (imazen/zenjxl#8 item 3): `examples/sweep_validate.rs`
//! used to soft-exempt the `lean` label from its inert-step hard check
//! because the LeanFaster bundle only diverges from Zenjxl on content
//! that trips Zenjxl's per-image gates (screenshot-/smooth-photo-class),
//! which the small CID22+synthetic corpus might not contain. This test
//! is the "confirmed gate-tripping image": it builds a synthetic
//! screenshot, PROVES it is screenshot-class by upstream's own
//! discriminator (`flat_color_block_ratio >= 0.35` over 8×8 blocks with
//! per-channel range <= 4, at >= 65,536 px — jxl-encoder
//! `api/content_detect.rs::classify_from_proxies`), then encodes the
//! harness's exact `vd-e7_zen_def_q*` vs `vd-e7_lean_def_q*` cells on it
//! and requires at least one quality point to differ. With that
//! guaranteed, the harness's exemption is retired (inert `lean` is a
//! hard failure like every other axis step).

#![cfg(all(feature = "encode", feature = "__expert"))]

use zenjxl::PixelLayout;
use zenjxl::sweep::{BuiltConfig, variant_from_cell_id};

const W: usize = 512;
const H: usize = 512;
/// Mirrors `Q_GRID` in `examples/sweep_validate.rs`.
const Q_GRID: [u32; 6] = [10, 30, 50, 70, 85, 95];
// Upstream skips classification below CONTENT_CLASS_MIN_PIXELS (65,536).
const _: () = assert!(W * H >= 65_536, "below CONTENT_CLASS_MIN_PIXELS");

/// Deterministic synthetic screenshot: flat UI chrome (title bar,
/// sidebar, content background), text-like glyph runs on a 24-px line
/// pitch (8-px "font", aligned so the 8-row blocks between lines stay
/// flat), a bordered flat button, and one embedded photo-like
/// thumbnail (gradient + speckle) so the image is not trivially flat.
/// Kept in sync with `generate_screenshot` in
/// `examples/sweep_validate.rs` — the harness encodes the same picture.
fn synthetic_screenshot_rgb8() -> Vec<u8> {
    let mut px = vec![0u8; W * H * 3];
    let mut put = |x: usize, y: usize, c: [u8; 3]| {
        let i = (y * W + x) * 3;
        px[i..i + 3].copy_from_slice(&c);
    };
    let bg = [240, 240, 240];
    let bar = [40, 44, 52];
    let side = [250, 250, 250];
    let ink = [20, 20, 20];
    let accent = [30, 110, 220];
    for y in 0..H {
        for x in 0..W {
            let c = if y < 40 {
                bar
            } else if x < 128 {
                side
            } else {
                bg
            };
            put(x, y, c);
        }
    }
    // Sidebar "menu items": flat accent rectangles with 1-px borders.
    for item in 0..6 {
        let y0 = 64 + item * 48;
        for y in y0..y0 + 28 {
            for x in 12..116 {
                let edge = y == y0 || y == y0 + 27 || x == 12 || x == 115;
                put(x, y, if edge { ink } else { accent });
            }
        }
    }
    // Text lines in the content area: 8-px glyph rows on a 24-px pitch,
    // LCG-driven runs of 1–3 dark pixels with gaps, like antialias-free
    // bitmap text. Each run's colour toggles between ink and accent.
    let mut state: u32 = 0x5EED_1234;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state >> 16
    };
    let mut line = 0;
    while 56 + line * 24 + 16 <= H - 8 {
        let y0 = 56 + line * 24 + 8;
        let x_end = 144 + (next() as usize % 300) + 40;
        let mut x = 144;
        while x < x_end.min(W - 8) {
            let run = 1 + (next() as usize % 3);
            let gap = 1 + (next() as usize % 4);
            let colour = if next() % 7 == 0 { accent } else { ink };
            for dx in 0..run {
                for dy in 0..8 {
                    // Glyph-ish: skip a pseudo-random subset of pixels.
                    if (next() % 5) != 0 {
                        put(x + dx, y0 + dy, colour);
                    }
                }
            }
            x += run + gap;
        }
        line += 1;
    }
    // Embedded photo-like thumbnail (gradient + speckle), 160×160.
    for y in 300..460 {
        for x in 320..480 {
            let gx = ((x - 320) * 255 / 159) as u8;
            let gy = ((y - 300) * 255 / 159) as u8;
            let n = (next() & 0x1F) as u8;
            put(
                x,
                y,
                [
                    gx.wrapping_add(n),
                    gy.wrapping_add(n / 2),
                    (255 - gx / 2).wrapping_sub(n),
                ],
            );
        }
    }
    px
}

/// Upstream's W44-164 discriminator, re-implemented verbatim: fraction
/// of 8×8 blocks whose per-channel (max - min) is <= 4 on every channel.
fn flat_color_block_ratio(px: &[u8]) -> f32 {
    let (bx, by) = (W / 8, H / 8);
    let mut flat = 0usize;
    for j in 0..by {
        for i in 0..bx {
            let mut lo = [255u8; 3];
            let mut hi = [0u8; 3];
            for dy in 0..8 {
                for dx in 0..8 {
                    let p = ((j * 8 + dy) * W + i * 8 + dx) * 3;
                    for c in 0..3 {
                        lo[c] = lo[c].min(px[p + c]);
                        hi[c] = hi[c].max(px[p + c]);
                    }
                }
            }
            if (0..3).all(|c| hi[c] - lo[c] <= 4) {
                flat += 1;
            }
        }
    }
    flat as f32 / (bx * by) as f32
}

fn encode_cell(id: &str, px: &[u8]) -> Vec<u8> {
    let variant = variant_from_cell_id(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    match variant.build() {
        // threads pinned to 1 exactly like the harness's `encode()`.
        BuiltConfig::Lossy(c) => {
            c.with_threads(1)
                .encode(px, W as u32, H as u32, PixelLayout::Rgb8)
        }
        BuiltConfig::Lossless(_) => panic!("{id}: expected a lossy cell"),
    }
    .unwrap_or_else(|e| panic!("{id}: encode failed: {e:?}"))
}

#[test]
fn synthetic_screenshot_is_screenshot_class_by_upstream_predicate() {
    let px = synthetic_screenshot_rgb8();
    let fcbr = flat_color_block_ratio(&px);
    let fnv = px.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    eprintln!("synthetic screenshot flat_color_block_ratio = {fcbr:.3}, pixel fnv64 = {fnv:016x}");
    // 0.35 is W44_164_FCBR_SCREENSHOT_MIN; the calibrated screenshot band
    // is 0.360–0.907 (10 GB82-SC screenshots), photos top out at 0.098.
    assert!(
        (0.36..=0.907).contains(&fcbr),
        "flat_color_block_ratio = {fcbr:.3}: not inside the calibrated screenshot band"
    );
}

#[test]
fn lean_diverges_from_zenjxl_default_on_screenshot_class() {
    let px = synthetic_screenshot_rgb8();
    let mut differing = Vec::new();
    for q in Q_GRID {
        let zen = encode_cell(&format!("vd-e7_zen_def_q{q}"), &px);
        let lean = encode_cell(&format!("vd-e7_lean_def_q{q}"), &px);
        assert_eq!(&zen[..2], &[0xFF, 0x0A]);
        assert_eq!(&lean[..2], &[0xFF, 0x0A]);
        eprintln!(
            "q{q}: zen {} B, lean {} B, {}",
            zen.len(),
            lean.len(),
            if zen == lean { "IDENTICAL" } else { "differ" }
        );
        if zen != lean {
            differing.push(q);
        }
    }
    assert!(
        !differing.is_empty(),
        "LeanFaster is byte-identical to Zenjxl on a confirmed screenshot-class image at \
         every harness quality point {Q_GRID:?} — the `lean` stratum would be inert and \
         the harness's retired soft-exemption must not be reinstated; investigate the \
         per-image gates instead"
    );
}
