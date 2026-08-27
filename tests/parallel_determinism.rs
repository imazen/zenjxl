// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! Parallel-build determinism: the same config must produce
//! byte-identical output at `threads = 1` and `threads = N` under the
//! `parallel` feature (which also enables jxl-encoder's
//! `parallel-tree-learning`).
//!
//! Why this lives here and not only upstream: the sweep fingerprint
//! contract (`docs/VARIANT_GENERATION.md` §4) deliberately leaves the
//! thread count OUT of the fingerprint — "threading knobs must be
//! byte-neutral" — and `examples/sweep_validate.rs` proves its axes on a
//! non-`parallel` build with threads pinned to 1. This is the
//! zenjxl-side check that the `parallel` build honours that assumption,
//! so a sweep run on a `parallel` build produces the bytes the harness
//! validated. Tracked in imazen/zenjxl#8 (item 4).
//!
//! **What the maiden run (2026-08-27, jxl-encoder path dep) measured on
//! the 512×384 image below** — every cell deterministic across repeats:
//!
//! | mode (`LosslessConfig::with_sectioned_trees`) | e5 | e7 | e9 |
//! |---|---|---|---|
//! | `Off` / `On` / `Hybrid` | invariant | invariant | invariant |
//! | `Auto` (default) | **threads=1 ≠ threads≥2 (and ≠ 0)** | **same** | invariant |
//! | lossy d=1.0 (any) | — | invariant | invariant |
//!
//! The parallel machinery is byte-deterministic; the ONE divergence is
//! upstream's `SectionedTrees::Auto` policy (jxl-encoder `api.rs`,
//! 2026-08-19: "sectioned at effort <= 7 with more than one worker"),
//! which makes the DEFAULT lossless config at e≤7 resolve to `Off`
//! bytes at `threads=1` and `On` bytes at `threads>1` (e7: 265,500 B vs
//! 279,016 B here). That is a mode-selection policy consulting the
//! thread count, not a race — and `sectioned_trees` is not part of the
//! sweep fingerprint. How zenjxl's sweep should treat it (pin a
//! thread-invariant mode in `LosslessVariant::build`, or add
//! `sectioned_trees` as an axis + hash it, or ask upstream to drop the
//! thread arm from `Auto`) is an open policy decision on #8; until it is
//! made, the sweep contract holds only at `threads = 1` for lossless
//! e≤7, and the default-`Auto` e≤7 case is deliberately NOT asserted
//! here (it would fail, and a test asserting the divergence would
//! enshrine it).
//!
//! Sizing: 512×384 RGB8 (196,608 px) with a noise band so the modular
//! tree learner sees far more than the e≤7 `parallel_root_threshold`
//! (8192 samples; 4096 at e≥8) and actually takes the root-split fork
//! (confirmed with `JXL_DBG_PARALLEL_TREE=1`: `gate=true` + root splits
//! on every lossless cell), and so the VarDCT path has more than one
//! 256×256 group to fan out.

#![cfg(all(feature = "parallel", feature = "encode"))]

use jxl_encoder::api::SectionedTrees;
use zenjxl::{LosslessConfig, LossyConfig, PixelLayout};

const W: u32 = 512;
const H: u32 = 384;

/// Mixed-content RGB8: smooth gradient (top third), LCG noise band
/// (middle third — keeps the post-dedup sample count high), bars +
/// speckle (bottom third — small-DCT / IDENTITY friendly).
fn synthetic_rgb8() -> Vec<u8> {
    let mut out = Vec::with_capacity((W * H * 3) as usize);
    let mut state: u32 = 0x2468_ACE1;
    for y in 0..H {
        for x in 0..W {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 24) as u8;
            let (r, g, b) = if y < H / 3 {
                let v = ((x + y) * 255 / (W + H / 3 - 2)) as u8;
                (v, v.wrapping_add(20), v.wrapping_sub(20))
            } else if y < 2 * H / 3 {
                (noise, noise.rotate_left(3), noise ^ 0xA5)
            } else {
                let bars_g: u8 = if (x / 4) % 2 == 0 { 30 } else { 220 };
                let speckle = noise & 0x3F;
                (
                    (x as u8) ^ 0x55,
                    bars_g,
                    ((x as u8) ^ 0x55).wrapping_add(speckle) | 0x10,
                )
            };
            out.extend_from_slice(&[r, g, b]);
        }
    }
    out
}

fn encode_lossless(effort: u8, mode: SectionedTrees, threads: usize, px: &[u8]) -> Vec<u8> {
    LosslessConfig::new()
        .with_effort(effort)
        .with_sectioned_trees(mode)
        .with_threads(threads)
        .encode(px, W, H, PixelLayout::Rgb8)
        .unwrap_or_else(|e| panic!("lossless e{effort} {mode:?} threads={threads}: {e:?}"))
}

fn encode_lossy(effort: u8, threads: usize, px: &[u8]) -> Vec<u8> {
    LossyConfig::new(1.0)
        .with_effort(effort)
        .with_threads(threads)
        .encode(px, W, H, PixelLayout::Rgb8)
        .unwrap_or_else(|e| panic!("lossy e{effort} threads={threads}: {e:?}"))
}

/// `0` = ambient rayon pool, `1` = forced sequential, `N >= 2` = a
/// dedicated N-thread pool (per `with_threads` upstream docs). Every one
/// of them must match the sequential bytes exactly.
const THREAD_COUNTS: [usize; 3] = [2, 4, 0];

fn assert_thread_invariant(label: &str, encode: impl Fn(usize) -> Vec<u8>) {
    let sequential = encode(1);
    assert_eq!(
        &sequential[..2],
        &[0xFF, 0x0A],
        "{label}: not a JXL codestream"
    );
    for &n in &THREAD_COUNTS {
        let parallel = encode(n);
        assert!(
            parallel == sequential,
            "{label}: threads={n} output ({} B) differs from threads=1 ({} B) — \
             the parallel build is not byte-deterministic, which breaks the sweep \
             fingerprint contract (thread count is excluded from the fingerprint)",
            parallel.len(),
            sequential.len()
        );
    }
}

/// Every explicit sectioned-trees mode must be thread-invariant at the
/// efforts where `Auto` is thread-dependent (e≤7) and above (e9). This
/// is the proof that the divergence documented in the module docs is
/// the `Auto` policy alone, not the parallel encode machinery.
#[test]
fn lossless_explicit_sectioned_modes_threads_byte_identical() {
    let px = synthetic_rgb8();
    for mode in [
        SectionedTrees::Off,
        SectionedTrees::On,
        SectionedTrees::Hybrid,
    ] {
        for effort in [5u8, 7, 9] {
            assert_thread_invariant(&format!("lossless e{effort} {mode:?}"), |t| {
                encode_lossless(effort, mode, t, &px)
            });
        }
    }
}

/// The default (`Auto`) lossless config is thread-invariant at e9,
/// where upstream's thread arm (`effort <= 7`) cannot fire.
#[test]
fn lossless_e9_default_threads_byte_identical() {
    let px = synthetic_rgb8();
    assert_thread_invariant("lossless e9 Auto", |t| {
        encode_lossless(9, SectionedTrees::Auto, t, &px)
    });
}

#[test]
fn lossy_e7_threads_byte_identical() {
    let px = synthetic_rgb8();
    assert_thread_invariant("lossy e7 d1.0", |t| encode_lossy(7, t, &px));
}

#[test]
fn lossy_e9_threads_byte_identical() {
    let px = synthetic_rgb8();
    assert_thread_invariant("lossy e9 d1.0", |t| encode_lossy(9, t, &px));
}
