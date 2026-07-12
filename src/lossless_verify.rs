// Copyright (c) Imazen LLC and the JPEG XL Project Authors.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing

//! Self-verified lossless encode tuned for image-proxy deployment: fast on
//! typical content, multi-candidate verification exactly where a single
//! fixed config is measurably unreliable.
//!
//! Derived from a 6,841-rendition / 1,000-origin local sweep (2026-07-12,
//! stratified across all 21 imazen-26 content classes, 4–11 sizes per
//! origin, 32 configs — every curated `__expert` internal-params probe at
//! its non-default-aliasing efforts — with per-cell local encode timing;
//! zero byte drift vs the prior fleet sweep across 53,964 overlapping
//! cells). Policies were selected on the origin-parity train split, tuned
//! on validation, and the shipped policy was evaluated ONCE on the held-out
//! test split. Record: `benchmarks/proxy_policy_2026-07-12/` in this repo.
//!
//! The shipped policy (test-split numbers, vs the previous
//! palette-gated {e10_def±pal0, e6_def±pal0} policy):
//! - **34% faster** on average (4.06s vs 6.18s per image on the corpus mix,
//!   single-threaded 7950X) — the lean branch runs ONE encode at effort 9,
//!   which the sweep showed is both cheaper than effort 10 AND more often
//!   optimal (the e9/e10 byte ladder is non-monotonic).
//! - **1.9× lower mean byte overhead** vs the all-config oracle
//!   (0.79% vs 1.48%), p99 8.6% vs 40.1%, half as many >20% outliers.
//! - Both policies share one residual pathological family (fine-grid
//!   synthetic plots; worst single image ~80% vs ~76% — a wash).
//!
//! Lean cell (everything not gated): `mod-e9_lloyd-pal0` — effort 9,
//! `lloyd_max_buckets` on (won 63% of head-to-heads vs default in probe
//! sampling, net-positive, bounded downside), palette detection OFF
//! (palette hurts unpredictably on exactly the content where it engages;
//! disabling it is the better single default, and palette-ON cells are
//! reachable via the rich branch).
//!
//! Rich branch (low-color-count OR near-grayscale content, ~20–28% of a
//! mixed corpus — the regime with a measurably jagged effort × params RD
//! landscape): additionally `mod-e10_lloyd-pal0`, `mod-e9_seeds2`, and
//! `mod-e10_maxsamples8192` (a knob that LOSES on average but is the
//! oracle on this regime's worst family — +30–86% rescued), keep the
//! smallest. The gate is computed locally in one cheap pixel pass
//! (early-capped distinct-color count ≤ 256, mirroring zenanalyze's
//! `palette_fits_in_256`, OR a ≥99% near-gray pixel fraction — a
//! conservative in-codec analog of zenanalyze's `grayscale_score ≥ 0.99`,
//! which catches the same family at sizes where rescaling pushes the
//! distinct-color count past 256).
//!
//! Candidates are constructed via [`crate::sweep::variant_from_cell_id`],
//! reusing the sweep infrastructure's tested construction path, per policy
//! that MLP/research-derived work routes through `__expert`.
//!
//! Requires the `encode` and `__expert` features.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use imgref::ImgRef;
use jxl_encoder::convenience::encode_rgb8_lossless;
use rgb::Rgb;
use whereat::ResultAtExt;

use crate::error::JxlError;
use crate::sweep::{BuiltConfig, variant_from_cell_id};

type At<E> = whereat::At<E>;

/// Distinct-color cap for the "low color count" regime. Matches
/// zenanalyze's `palette_fits_in_256` feature threshold (an 8-bit
/// palette can represent at most this many distinct colors).
const LOW_COLOR_COUNT_CAP: usize = 256;

/// Per-channel spread (max−min over r,g,b) at or below which a pixel
/// counts as "near-gray" for the grayscale gate.
const GRAY_TOLERANCE: u8 = 8;

/// Minimum near-gray pixel fraction for the grayscale gate (analog of
/// zenanalyze `grayscale_score >= 0.99`).
const GRAY_FRACTION: f64 = 0.99;

/// The single lean candidate for typical content: effort 9 +
/// `lloyd_max_buckets` + palette detection disabled. See module docs for
/// why each of the three choices beats the old `mod-e10_def` default.
const LEAN_CANDIDATE: &str = "mod-e9_lloyd-pal0";

/// The verified candidate set for gated (low-color / near-gray) content.
/// Ordered cheapest-useful-first; the smallest output wins regardless.
const RICH_CANDIDATES: [&str; 4] = [
    "mod-e9_lloyd-pal0",
    "mod-e9_seeds2",
    "mod-e10_lloyd-pal0",
    "mod-e10_maxsamples8192",
];

/// One cheap pass: does `img` fall in the verify regime — at most
/// [`LOW_COLOR_COUNT_CAP`] distinct colors OR at least [`GRAY_FRACTION`]
/// near-gray pixels? Distinct-color tracking stops inserting once the cap
/// is exceeded (bounded memory); the near-gray count needs the full pass
/// anyway, which is O(n) compares — trivial next to seconds of encode.
fn needs_verification(img: ImgRef<'_, Rgb<u8>>) -> bool {
    let mut seen = BTreeSet::new();
    let mut over_cap = false;
    let mut gray = 0usize;
    let mut total = 0usize;
    for row in img.rows() {
        for px in row {
            total += 1;
            let (mx, mn) = (px.r.max(px.g).max(px.b), px.r.min(px.g).min(px.b));
            if mx - mn <= GRAY_TOLERANCE {
                gray += 1;
            }
            if !over_cap {
                seen.insert((px.r, px.g, px.b));
                if seen.len() > LOW_COLOR_COUNT_CAP {
                    over_cap = true;
                    seen.clear();
                }
            }
        }
    }
    !over_cap || (gray as f64) >= GRAY_FRACTION * (total as f64)
}

/// Encode `img` with the config parsed from sweep cell-id `id`. Panics on
/// an unparseable id or a non-lossless variant — both are programmer
/// errors (the candidate list above is a fixed, validated constant), not
/// runtime conditions callers need to handle.
fn encode_cell(img: ImgRef<'_, Rgb<u8>>, id: &str) -> Result<Vec<u8>, At<JxlError>> {
    let variant = variant_from_cell_id(id)
        .unwrap_or_else(|e| panic!("lossless_verify: fixed candidate id {id:?} must parse: {e}"));
    let BuiltConfig::Lossless(cfg) = variant.build() else {
        panic!("lossless_verify: fixed candidate id {id:?} must be a lossless variant");
    };
    encode_rgb8_lossless(img, &cfg).map_err_at(JxlError::Encode)
}

/// Lossless-encode `img` under the measured time+RD-optimal proxy policy
/// (see the module docs for the sweep evidence and test-split numbers).
///
/// - Typical content (most images): ONE encode with [`LEAN_CANDIDATE`].
/// - Low-color-count or near-grayscale content (detected locally in one
///   cheap pixel pass; ~20–28% of a mixed corpus): encodes the
///   [`RICH_CANDIDATES`] set and returns the smallest.
pub fn encode_rgb8_lossless_verified(img: ImgRef<'_, Rgb<u8>>) -> Result<Vec<u8>, At<JxlError>> {
    if !needs_verification(img) {
        return encode_cell(img, LEAN_CANDIDATE);
    }

    let mut best: Option<Vec<u8>> = None;
    for id in RICH_CANDIDATES {
        let candidate = encode_cell(img, id)?;
        best = Some(match best {
            Some(b) if b.len() <= candidate.len() => b,
            _ => candidate,
        });
    }
    // Loop always runs >=1 iteration over a non-empty fixed candidate list.
    Ok(best.expect("candidate list is non-empty"))
}
