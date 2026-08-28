// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! The class-conditional alpha axes (imazen/zenjxl#8 item 5, playbook
//! pattern 10 "the class-conditional knob") — the two-sided check on a
//! CI-sized alpha-class image:
//!
//! 1. **On-class**: every alpha step of `SweepAxes::modes_full_alpha`
//!    (`ad2`, `ad10`, `asq2`, `asq10`, `keepinv`; lossless `zeroinv`)
//!    changes output bytes vs its mode's default stratum on an RGBA
//!    sprite with a transparent (alpha = 0) background carrying noisy RGB
//!    and soft (non-constant) alpha edges.
//! 2. **Off-class**: the SAME pixels fed as `Rgb8` produce bytes
//!    identical to the default at every step — the alpha knobs must not
//!    couple into the colour path.
//! 3. **Lossless exactness, alpha-aware**: the default lossless cell
//!    round-trips RGBA exactly; `zeroinv` round-trips alpha exactly and
//!    RGB exactly wherever alpha > 0 (it deliberately discards RGB under
//!    alpha = 0 — the harness gate masks the same way).
//! 4. **Neutral spelling pinned by encode**: `Quantized(1.0)` (q = 1 at
//!    8-bit alpha) is byte-identical to the default, which is why no
//!    preset curates it.
//!
//! The byte-for-byte twin of the sprite generator lives in
//! `examples/sweep_validate.rs` (`generate_sprite_rgba`), whose alpha
//! leg runs the same checks over the corpus; keep the two in sync.

#![cfg(all(feature = "encode", feature = "decode", feature = "__expert"))]

use zenjxl::PixelLayout;
use zenjxl::sweep::{BuiltConfig, QualityGrid, SweepAxes, SweepBuilder, variant_from_cell_id};

const W: usize = 512;
const H: usize = 512;
/// Subset of the harness `Q_GRID` (runtime: this is a debug-build CI test).
const Q: [u32; 2] = [10, 85];

/// Deterministic RGBA sprite: fully transparent background whose RGB
/// samples are LCG noise (so `keep_invisible` / `zero_invisible` have
/// something to preserve or discard), three shaded discs with 24-px
/// soft alpha edges (a non-constant alpha plane, so the squeeze path
/// engages), and a fully-opaque interior.
fn sprite_rgba8() -> Vec<u8> {
    let mut px = vec![0u8; W * H * 4];
    let mut state: u32 = 0xA1FA_5EED;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state >> 16
    };
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            let mut rgb = [
                (next() & 0xFF) as u8,
                (next() & 0xFF) as u8,
                (next() & 0xFF) as u8,
            ];
            let mut a = 0u8;
            for (cx, cy, r, col) in [
                (160.0f32, 160.0f32, 110.0f32, [220u8, 60, 40]),
                (350.0, 200.0, 90.0, [40, 180, 90]),
                (260.0, 380.0, 120.0, [50, 90, 230]),
            ] {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if d < r + 12.0 {
                    let cov = ((r + 12.0 - d) / 24.0).clamp(0.0, 1.0);
                    let shade = (255.0 - d / r * 90.0).clamp(0.0, 255.0) as u8;
                    rgb = [
                        (u16::from(col[0]) * u16::from(shade) / 255) as u8,
                        (u16::from(col[1]) * u16::from(shade) / 255) as u8,
                        (u16::from(col[2]) * u16::from(shade) / 255) as u8,
                    ];
                    a = a.max((cov * 255.0) as u8);
                }
            }
            px[i..i + 3].copy_from_slice(&rgb);
            px[i + 3] = a;
        }
    }
    px
}

fn rgb_of(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

fn encode_cell(id: &str, px: &[u8], layout: PixelLayout) -> Vec<u8> {
    let variant = variant_from_cell_id(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    match variant.build() {
        // threads pinned to 1 exactly like the harness's `encode()`.
        BuiltConfig::Lossy(c) => c.with_threads(1).encode(px, W as u32, H as u32, layout),
        BuiltConfig::Lossless(c) => c.with_threads(1).encode(px, W as u32, H as u32, layout),
    }
    .unwrap_or_else(|e| panic!("{id}: encode failed: {e:?}"))
}

fn decode_rgba8(jxl: &[u8]) -> Vec<u8> {
    let out = zenjxl::decode(jxl, None, &[zenpixels::PixelDescriptor::RGBA8]).expect("decode");
    assert_eq!((out.info.width, out.info.height), (W as u32, H as u32));
    let px = out.pixels.into_vec();
    assert_eq!(
        px.len(),
        W * H * 4,
        "decoder did not hand back packed RGBA8"
    );
    px
}

/// The alpha steps the preset curates (dev-1 strata at the default effort
/// / strategy), split by mode. Derived from the plan so the test tracks
/// the preset rather than a hand-copied list.
fn curated_alpha_steps() -> (Vec<String>, Vec<String>) {
    let plan = SweepBuilder::new(
        SweepAxes::modes_full_alpha(),
        QualityGrid::ExplicitQuality(vec![85.0]),
    )
    .plan();
    let mut lossy = Vec::new();
    let mut lossless = Vec::new();
    for c in plan.cells.iter().filter(|c| c.deviations == 1) {
        if let Some(base) = c.id.strip_suffix("_q85") {
            if base.starts_with("vd-e7_zen_def-") {
                lossy.push(base.to_string());
            }
        } else if c.id.starts_with("mod-e7_def-") {
            lossless.push(c.id.clone());
        }
    }
    (lossy, lossless)
}

#[test]
fn alpha_steps_are_live_on_class_and_inert_off_class() {
    let rgba = sprite_rgba8();
    let rgb = rgb_of(&rgba);
    let (mut lossy_steps, lossless_steps) = curated_alpha_steps();
    lossy_steps.sort();
    assert_eq!(
        lossy_steps,
        [
            "vd-e7_zen_def-ad10",
            "vd-e7_zen_def-ad2",
            "vd-e7_zen_def-asq10",
            "vd-e7_zen_def-asq2",
            "vd-e7_zen_def-keepinv",
        ],
        "modes_full_alpha lossy step set drifted"
    );
    assert_eq!(lossless_steps, ["mod-e7_def-zeroinv"]);

    // Lossy: two-sided per step over Q.
    let mut def_rgba = Vec::new();
    let mut def_rgb = Vec::new();
    for q in Q {
        def_rgba.push(encode_cell(
            &format!("vd-e7_zen_def_q{q}"),
            &rgba,
            PixelLayout::Rgba8,
        ));
        def_rgb.push(encode_cell(
            &format!("vd-e7_zen_def_q{q}"),
            &rgb,
            PixelLayout::Rgb8,
        ));
    }
    for step in &lossy_steps {
        let mut differing = Vec::new();
        for (qi, q) in Q.iter().enumerate() {
            let on = encode_cell(&format!("{step}_q{q}"), &rgba, PixelLayout::Rgba8);
            let off = encode_cell(&format!("{step}_q{q}"), &rgb, PixelLayout::Rgb8);
            eprintln!(
                "{step} q{q}: rgba {} B vs default {} B ({}); rgb {} B ({})",
                on.len(),
                def_rgba[qi].len(),
                if on == def_rgba[qi] {
                    "IDENTICAL"
                } else {
                    "differ"
                },
                off.len(),
                if off == def_rgb[qi] {
                    "IDENTICAL"
                } else {
                    "differ"
                },
            );
            assert_eq!(
                off, def_rgb[qi],
                "CLASS COUPLING: {step} changed Rgb8 (no-alpha) output at q{q}"
            );
            if on != def_rgba[qi] {
                differing.push(*q);
            }
        }
        assert!(
            !differing.is_empty(),
            "INERT ALPHA STEP: {step} is byte-identical to the default on the alpha-class \
             sprite at every quality in {Q:?}"
        );
    }

    // Neutral spelling: d = 1.0 → q = 1 ≡ lossless alpha, so it must NOT
    // be a curated step (it would be an inert step by construction).
    let neutral = encode_cell("vd-e7_zen_def-ad1_q85", &rgba, PixelLayout::Rgba8);
    assert_eq!(
        neutral, def_rgba[1],
        "alpha_distance 1.0 is no longer the neutral spelling — re-curate the alpha axis"
    );
    assert!(
        !lossy_steps
            .iter()
            .any(|s| s.ends_with("-ad1") || s.ends_with("-asq1")),
        "preset curates the neutral spelling"
    );

    // Lossless: two-sided plus alpha-aware exactness.
    let def_on = encode_cell("mod-e7_def", &rgba, PixelLayout::Rgba8);
    let def_off = encode_cell("mod-e7_def", &rgb, PixelLayout::Rgb8);
    let decoded = decode_rgba8(&def_on);
    assert_eq!(
        decoded, rgba,
        "default lossless RGBA cell must round-trip exactly"
    );
    for step in &lossless_steps {
        let on = encode_cell(step, &rgba, PixelLayout::Rgba8);
        let off = encode_cell(step, &rgb, PixelLayout::Rgb8);
        eprintln!(
            "{step}: rgba {} B vs default {} B ({}); rgb {} B ({})",
            on.len(),
            def_on.len(),
            if on == def_on { "IDENTICAL" } else { "differ" },
            off.len(),
            if off == def_off {
                "IDENTICAL"
            } else {
                "differ"
            },
        );
        assert_eq!(off, def_off, "CLASS COUPLING: {step} changed Rgb8 output");
        assert_ne!(
            on, def_on,
            "INERT ALPHA STEP: {step} on the alpha-class sprite"
        );
        let dec = decode_rgba8(&on);
        let mut visible_mismatch = 0usize;
        let mut alpha_mismatch = 0usize;
        for (s, d) in rgba.as_chunks::<4>().0.iter().zip(dec.as_chunks::<4>().0) {
            if s[3] != d[3] {
                alpha_mismatch += 1;
            }
            if s[3] != 0 && s[..3] != d[..3] {
                visible_mismatch += 1;
            }
        }
        assert_eq!(alpha_mismatch, 0, "{step}: alpha plane not exact");
        assert_eq!(
            visible_mismatch, 0,
            "{step}: visible (alpha > 0) RGB not exact"
        );
    }
}
