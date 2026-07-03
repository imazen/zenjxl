// Copyright (c) Imazen LLC and the JPEG XL Project Authors.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing

//! Self-verified lossless encode for low-color-count content.
//!
//! A single fixed lossless config (effort + base predictor + palette
//! setting) is close to optimal for most content, but measurably
//! unreliable for a specific, cheaply-detectable minority: images with at
//! most 256 distinct colors (mostly near-grayscale / low-complexity
//! content) have a far more jagged effort × predictor RD landscape than
//! typical photographic content, where the single best combination
//! doesn't generalize from generic descriptive signal. Measured via a
//! 4497-image corpus sweep (see `zenanalyze`'s
//! `zentrain/examples/zenjxl_modular_picker_config.py`, 2026-07-03
//! root-cause + mitigation write-up): that minority (23.3% of the
//! corpus) carries 3.4× higher mean RD-overhead risk and is 2.5× more
//! likely to exceed 10% overhead than the rest, with absolute misses up
//! to several hundred KB on individual images.
//!
//! [`encode_rgb8_lossless_verified`] closes that gap without any
//! external feature-extraction or model dependency: it counts distinct
//! colors locally (cheap — bails out the instant the count exceeds the
//! threshold, so typical high-color content pays only that early-exit
//! scan, not a full pass), and for the flagged minority encodes a small
//! fixed candidate set (effort 10 and effort 6, palette on/off) instead
//! of trusting a single choice, keeping whichever is actually smallest.
//! Verified against ground-truth Pareto data on held-out validation AND
//! test splits independently: mean overhead in the flagged bucket drops
//! to ~0.5%, with zero images exceeding 20% overhead (down from a worst
//! case of 224% under a single fixed choice). The two extra candidates
//! are inexpensive: the flagged content type measurably encodes 18–26%
//! faster than average, so the added encode cost is proportionally
//! cheaper than the raw candidate count suggests.
//!
//! This is a narrow, validated heuristic — not the full picker vision
//! (a trained model choosing among all 120 effort × predictor × pred6 ×
//! palette combinations). No zen codec has runtime picker/model
//! integration today; that remains a separate, larger undertaking
//! (bridging zenanalyze feature extraction + zenpredict inference into
//! a codec's encode path). This module is deployable now, self-
//! contained, and closes the specific worst-case failure mode measured
//! so far.
//!
//! Requires the `encode` feature.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use imgref::ImgRef;
use jxl_encoder::LosslessConfig;
use jxl_encoder::convenience::encode_rgb8_lossless;
use rgb::Rgb;
use whereat::ResultAtExt;

use crate::error::JxlError;

type At<E> = whereat::At<E>;

/// Distinct-color cap for the "low color count" regime. Matches
/// zenanalyze's `palette_fits_in_256` feature threshold (an 8-bit
/// palette can represent at most this many distinct colors).
const LOW_COLOR_COUNT_CAP: usize = 256;

/// Count distinct colors in `img`, stopping as soon as the count exceeds
/// `cap`. Returns `true` if the image has AT MOST `cap` distinct colors,
/// `false` otherwise (the exact count above `cap` is never needed, so
/// this bails out the instant it's exceeded — typical high-color content
/// exits after scanning a small fraction of pixels).
fn has_at_most_distinct_colors(img: ImgRef<'_, Rgb<u8>>, cap: usize) -> bool {
    let mut seen = BTreeSet::new();
    for row in img.rows() {
        for px in row {
            seen.insert((px.r, px.g, px.b));
            if seen.len() > cap {
                return false;
            }
        }
    }
    true
}

/// Lossless-encode `img`, using a self-verified small candidate set for
/// low-color-count content instead of a single fixed choice. See the
/// module docs for the measured RD justification.
///
/// - At most [`LOW_COLOR_COUNT_CAP`] distinct colors (checked locally,
///   ~23% of a typical mixed corpus): encodes 4 fixed candidates
///   (effort 10 and effort 6, each with default and disabled palette
///   detection) and returns the smallest.
/// - Otherwise (the majority): a single encode at effort 10, default
///   settings — higher effort wins predictably and reliably for this
///   regime, so extra candidates aren't warranted.
///
/// Cost: 1 encode for typical/high-color content, up to 4 for
/// low-color-count content (itself measured to encode faster than
/// average, so the extra cost is proportionally cheaper than the raw
/// candidate count suggests).
pub fn encode_rgb8_lossless_verified(img: ImgRef<'_, Rgb<u8>>) -> Result<Vec<u8>, At<JxlError>> {
    if !has_at_most_distinct_colors(img, LOW_COLOR_COUNT_CAP) {
        let cfg = LosslessConfig::new().with_effort(10);
        return encode_rgb8_lossless(img, &cfg).map_err_at(JxlError::Encode);
    }

    let mut best: Option<Vec<u8>> = None;
    for (effort, palette_colors) in [(10u8, None), (10, Some(0)), (6, None), (6, Some(0))] {
        let mut cfg = LosslessConfig::new().with_effort(effort);
        if let Some(n) = palette_colors {
            cfg = cfg.with_modular_palette_colors(Some(n));
        }
        let candidate = encode_rgb8_lossless(img, &cfg).map_err_at(JxlError::Encode)?;
        best = Some(match best {
            Some(b) if b.len() <= candidate.len() => b,
            _ => candidate,
        });
    }
    // Loop always runs >=1 iteration over a non-empty fixed candidate list.
    Ok(best.expect("candidate list is non-empty"))
}
