// Copyright (c) Imazen LLC and the JPEG XL Project Authors.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing

//! Lossy JPEG → JXL recompression to a perceptual-quality **target**.
//!
//! Two paths, one closed loop:
//! - **Coarsen** ([`JpegRecompressMethod::Coarsen`]) — the PreserveJxl
//!   coefficient-domain path (from `jxl-encoder`): re-quantize the JPEG's own
//!   DCT coefficients to a coarser same-family scale. Wins at gentle /
//!   near-lossless targets (keeps coefficients, no re-encode overhead).
//! - **Reencode** ([`JpegRecompressMethod::Reencode`]) — decode the source and
//!   re-encode with the full VarDCT encoder (XYB, adaptive quant, big
//!   transforms, CfL). Wins at medium / aggressive targets.
//! - **Auto** ([`JpegRecompressMethod::Auto`]) — the **router**: run both to the
//!   target and keep the smaller. Beats either single path (the crossover is
//!   content- and target-dependent; see jxl-encoder
//!   `docs/JPEG_LOSSY_RECOMPRESSION.md`).
//!
//! `zenjxl` is the natural home because it deps both the encoder *and* the
//! decoder, so the whole loop (encode → decode → score → bisect) runs
//! in-process. The loop is **metric-agnostic**: the caller supplies a
//! [`RelativeScorer`] over decoded RGB8, so the same driver hits a
//! zensim-A / cvvdp / butteraugli (or any) target.
//!
//! Targets here are **relative** (generation loss vs the source's own decoded
//! pixels): the reference is the lossless transcode (scale 1.0), which is also
//! the **input** to the Reencode path — so both paths score against the *same*
//! reference and the comparison is apples-to-apples. Inferred
//! (absolute-vs-original) targeting layers on top via the source-quality floor
//! calibration.
//!
//! Requires the `jpeg-lossy` cargo feature.

use alloc::vec::Vec;

use jxl_encoder::jpeg::encode_jpeg_recompress_auto_codestream;
use zenpixels::PixelDescriptor;

use crate::decode::decode;
use crate::error::JxlError;

type At<E> = whereat::At<E>;

/// Which recompression path to use. See the module docs for the crossover.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum JpegRecompressMethod {
    /// Coefficient-domain coarsening (PreserveJxl). Best at gentle targets.
    Coarsen,
    /// Pixel re-encode (VarDCT). Best at medium / aggressive targets.
    Reencode,
    /// Run both to the target and keep the smaller (the router). The default.
    #[default]
    Auto,
}

/// A scorer over `(reference_rgb8, distorted_rgb8, width, height) -> score`.
///
/// Both slices are tightly-packed RGB8 (3 bytes/pixel, `width*height*3` bytes),
/// the reference being the source JPEG's own decoded pixels (lossless
/// transcode). The score's *direction* is given by `higher_is_better` on the
/// recompress entry points (zensim-A / cvvdp: higher better; butteraugli: lower
/// better).
pub type RelativeScorer<'a> = dyn Fn(&[u8], &[u8], u32, u32) -> f32 + 'a;

/// Recompress a JPEG to the smallest output meeting a **relative** quality
/// target via the chosen [`JpegRecompressMethod`], returning the bare JXL
/// codestream.
///
/// `target` is the score level in `scorer`'s units; `higher_is_better` gives the
/// metric direction. Each path bisects its quality knob (Coarsen: coarsening
/// scale; Reencode: VarDCT distance) for the coarsest setting still meeting the
/// target, scored against the lossless-transcode reference. `Auto` runs both and
/// keeps the smaller. If the target is unreachable (too strict for a path), that
/// path returns the lossless transcode (the floor — never larger, never worse).
///
/// Cost: ~10–14 encode+decode+score probes per path (so ~2× for `Auto`).
/// Requires the `jpeg-lossy` feature.
pub fn recompress_jpeg_lossy(
    jpeg_bytes: &[u8],
    method: JpegRecompressMethod,
    target: f32,
    higher_is_better: bool,
    scorer: &RelativeScorer<'_>,
    effort: u8,
) -> Result<Vec<u8>, At<JxlError>> {
    // Reference = the source's own decoded pixels (lossless transcode @ 1.0).
    // This is also the Reencode path's input, so both paths share the reference.
    let lossless = encode_coarsen(jpeg_bytes, 1.0, effort)?;
    let (ref_px, w, h) = decode_rgb8(&lossless)?;

    let meets = |score: f32| {
        if higher_is_better {
            score >= target
        } else {
            score <= target
        }
    };

    // Decode a candidate and score it against the reference.
    let score_of = |cs: &[u8]| -> Result<f32, At<JxlError>> {
        let (dist_px, dw, dh) = decode_rgb8(cs)?;
        Ok(if dw == w && dh == h && dist_px.len() == ref_px.len() {
            scorer(&ref_px, &dist_px, w, h)
        } else if higher_is_better {
            f32::MIN
        } else {
            f32::MAX
        })
    };

    let coarsen = || {
        run_loop(
            |scale| {
                let cs = encode_coarsen(jpeg_bytes, scale, effort)?;
                let sc = score_of(&cs)?;
                Ok((cs, sc))
            },
            1.0,
            6.0,
            &meets,
            &lossless,
        )
    };
    let reencode = || {
        run_loop(
            |dist| {
                let cs = encode_pixel(&ref_px, w, h, dist, effort)?;
                let sc = score_of(&cs)?;
                Ok((cs, sc))
            },
            0.3,
            4.0,
            &meets,
            &lossless,
        )
    };

    match method {
        JpegRecompressMethod::Coarsen => coarsen(),
        JpegRecompressMethod::Reencode => reencode(),
        JpegRecompressMethod::Auto => {
            let a = coarsen()?;
            let b = reencode()?;
            Ok(if a.len() <= b.len() { a } else { b })
        }
    }
}

/// Convenience: [`recompress_jpeg_lossy`] with [`JpegRecompressMethod::Coarsen`]
/// (the coefficient-domain PreserveJxl path only).
pub fn recompress_jpeg_lossy_relative(
    jpeg_bytes: &[u8],
    target: f32,
    higher_is_better: bool,
    scorer: &RelativeScorer<'_>,
    effort: u8,
) -> Result<Vec<u8>, At<JxlError>> {
    recompress_jpeg_lossy(
        jpeg_bytes,
        JpegRecompressMethod::Coarsen,
        target,
        higher_is_better,
        scorer,
        effort,
    )
}

/// Which perceptual metric an [`QualityTarget::Inferred`] is expressed in.
/// Used only for the **preliminary** floor table + abs↔relative mapping; the
/// scorer callback still does the actual scoring (supply one matching the
/// metric).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InferredMetric {
    /// zensim-A, native scale 0–100, higher = better (100 = identical).
    ZensimA,
    /// CVVDP JOD, native scale 0–10, higher = better.
    Cvvdp,
    /// Butteraugli (pnorm-3), distance ≥ 0, lower = better.
    Butteraugli,
}

impl InferredMetric {
    /// Score direction (true = higher is better).
    pub fn higher_is_better(self) -> bool {
        !matches!(self, InferredMetric::Butteraugli)
    }

    /// **Preliminary** additive map from an absolute target to the relative
    /// (vs-source) target the loop should aim for, given the source's `floor`.
    /// Model: a degradation of `Δ` below the floor in absolute terms is a
    /// degradation of `Δ` below "identical" in relative terms (perceptual
    /// degradations roughly add near the operating point). NOT calibrated —
    /// pending the abs↔relative sweep; see jxl-encoder
    /// `docs/JPEG_LOSSY_RECOMPRESSION.md`.
    pub fn approx_relative_target(self, abs_level: f32, floor: f32) -> f32 {
        match self {
            // higher-better, "identical" = top of scale (100 / 10):
            InferredMetric::ZensimA => (abs_level - floor + 100.0).clamp(0.0, 100.0),
            InferredMetric::Cvvdp => (abs_level - floor + 10.0).clamp(0.0, 10.0),
            // lower-better distance, "identical" = 0:
            InferredMetric::Butteraugli => (abs_level - floor).max(0.0),
        }
    }
}

/// What quality to hit. `Relative` is distortion vs the source's own pixels
/// (precise, directly measured by the scorer). `Inferred` is quality vs the
/// (unknown) original: the loop **clamps** an unreachable absolute target to the
/// lossless floor (the dominant inferred byte win), and otherwise aims at
/// `relative_target`. Build `Inferred` explicitly, or via the preliminary
/// [`QualityTarget::inferred_preliminary`] helper.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum QualityTarget {
    /// Distortion vs the source's own pixels (generation loss).
    Relative {
        /// Target score in the scorer's units.
        level: f32,
        /// Metric direction (true = higher better).
        higher_is_better: bool,
    },
    /// Quality vs the unknown original.
    Inferred {
        /// Desired absolute quality (vs the original), in the metric's units.
        abs_level: f32,
        /// Best achievable absolute quality from this source (the source's own
        /// quality vs the original) — predict from source quality.
        floor: f32,
        /// The relative (vs-source) score the loop should aim for to land at
        /// `abs_level` (caller-owned abs↔relative mapping).
        relative_target: f32,
        /// Metric direction (true = higher better).
        higher_is_better: bool,
    },
}

impl QualityTarget {
    /// **Preliminary** constructor: probe the source's quality
    /// (`zenjpeg::detect`), predict the floor from the N=5 calibration table,
    /// and derive `relative_target` via the additive map. Returns `None` if the
    /// source quality can't be read on the IJG scale (e.g. jpegli sources report
    /// a butteraugli-distance scale). NOT production-calibrated — see
    /// [`predict_inferred_floor`].
    pub fn inferred_preliminary(
        jpeg_bytes: &[u8],
        metric: InferredMetric,
        abs_level: f32,
    ) -> Option<Self> {
        let floor = predict_inferred_floor(jpeg_bytes, metric)?;
        Some(QualityTarget::Inferred {
            abs_level,
            floor,
            relative_target: metric.approx_relative_target(abs_level, floor),
            higher_is_better: metric.higher_is_better(),
        })
    }
}

/// Recompress to a [`QualityTarget`] via the chosen [`JpegRecompressMethod`].
///
/// `Relative` runs the loop directly. `Inferred` first applies the
/// **achievability clamp**: if the absolute target is *better* than the source's
/// floor it cannot be reached (you can't recover detail the source discarded), so
/// the lossless transcode (the floor — smallest output preserving the source)
/// ships; otherwise the loop aims at `relative_target`. Requires the `jpeg-lossy`
/// feature.
pub fn recompress_jpeg_lossy_target(
    jpeg_bytes: &[u8],
    method: JpegRecompressMethod,
    target: QualityTarget,
    scorer: &RelativeScorer<'_>,
    effort: u8,
) -> Result<Vec<u8>, At<JxlError>> {
    match target {
        QualityTarget::Relative {
            level,
            higher_is_better,
        } => recompress_jpeg_lossy(jpeg_bytes, method, level, higher_is_better, scorer, effort),
        QualityTarget::Inferred {
            abs_level,
            floor,
            relative_target,
            higher_is_better,
        } => {
            let unreachable = if higher_is_better {
                abs_level > floor
            } else {
                abs_level < floor
            };
            if unreachable {
                // Can't beat the source's own quality vs the original → floor.
                return encode_coarsen(jpeg_bytes, 1.0, effort);
            }
            recompress_jpeg_lossy(
                jpeg_bytes,
                method,
                relative_target,
                higher_is_better,
                scorer,
                effort,
            )
        }
    }
}

/// **Preliminary** floor predictor: read the source's IJG quality
/// (`zenjpeg::detect::probe`) and interpolate the N=5 calibration table (CID22,
/// mean of 5 — `jxl-encoder/docs/JPEG_LOSSY_RECOMPRESSION.md`) to the best
/// achievable absolute quality from this source, in `metric`'s units. Returns
/// `None` when the source quality isn't on the IJG scale.
///
/// NOT production-calibrated — the table is a 5-image starting point pending a
/// proper size×quality×content sweep. Treat the result as an estimate; prefer a
/// caller-supplied floor when you have better calibration.
pub fn predict_inferred_floor(jpeg_bytes: &[u8], metric: InferredMetric) -> Option<f32> {
    let probe = zenjpeg::detect::probe(jpeg_bytes).ok()?;
    if probe.quality.scale != zenjpeg::detect::QualityScale::IjgQuality {
        return None;
    }
    let q = probe.quality.value;
    // (ijg_q, zensim_floor, butter_floor, cvvdp_floor) — N=5 CID22 means.
    const TABLE: [(f32, f32, f32, f32); 3] = [
        (72.0, 70.1, 1.477, 9.826),
        (82.0, 76.5, 1.291, 9.865),
        (92.0, 88.2, 0.668, 9.992),
    ];
    let pick = |f: fn(&(f32, f32, f32, f32)) -> f32| -> f32 {
        if q <= TABLE[0].0 {
            return f(&TABLE[0]);
        }
        if q >= TABLE[2].0 {
            return f(&TABLE[2]);
        }
        for w in TABLE.windows(2) {
            let (q0, q1) = (w[0].0, w[1].0);
            if q >= q0 && q <= q1 {
                let t = (q - q0) / (q1 - q0);
                return f(&w[0]) + t * (f(&w[1]) - f(&w[0]));
            }
        }
        f(&TABLE[2])
    };
    Some(match metric {
        InferredMetric::ZensimA => pick(|r| r.1),
        InferredMetric::Butteraugli => pick(|r| r.2),
        InferredMetric::Cvvdp => pick(|r| r.3),
    })
}

/// Thin wrapper: PreserveJxl coarsen at an explicit `scale` (no quality loop),
/// applying the bundled deadzone + chroma policy. `scale <= 1.0` is the lossless
/// transcode. Exposed so callers that already know the scale (or drive their own
/// loop) can reach the coefficient-domain path through the codec wrapper.
pub fn recompress_jpeg_coarsen(
    jpeg_bytes: &[u8],
    scale: f32,
    effort: u8,
) -> Result<Vec<u8>, At<JxlError>> {
    encode_coarsen(jpeg_bytes, scale, effort)
}

/// Verified-endpoint bisection over a quality knob: find the coarsest knob in
/// `[lo, hi]` (higher knob = lower quality = smaller) whose probe still meets the
/// target. `floor` is returned when even the gentlest knob (`lo`) fails.
fn run_loop(
    probe: impl Fn(f32) -> Result<(Vec<u8>, f32), At<JxlError>>,
    lo: f32,
    hi: f32,
    meets: &impl Fn(f32) -> bool,
    floor: &[u8],
) -> Result<Vec<u8>, At<JxlError>> {
    let (lo_cs, lo_sc) = probe(lo)?;
    if !meets(lo_sc) {
        // Even the gentlest setting can't reach the target → lossless floor.
        return Ok(floor.to_vec());
    }
    let mut best = lo_cs;
    let mut a = lo;
    let mut b = hi;

    // Extend the upper bound until coarsening fails to meet the target.
    let mut bracketed = false;
    for _ in 0..6 {
        let (cs, sc) = probe(b)?;
        if meets(sc) {
            best = cs;
            a = b;
            b *= 1.8;
        } else {
            bracketed = true;
            break;
        }
    }
    if !bracketed {
        return Ok(best); // never failed up to the extended cap — coarsest wins
    }

    // Bisect [a (meets), b (fails)] for the coarsest meeting knob.
    for _ in 0..8 {
        let mid = 0.5 * (a + b);
        let (cs, sc) = probe(mid)?;
        if meets(sc) {
            best = cs;
            a = mid;
        } else {
            b = mid;
        }
    }
    Ok(best)
}

/// Decode a bare JXL codestream to tightly-packed RGB8 + dimensions.
fn decode_rgb8(codestream: &[u8]) -> Result<(Vec<u8>, u32, u32), At<JxlError>> {
    let out = decode(codestream, None, &[PixelDescriptor::RGB8])?;
    let w = out.info.width;
    let h = out.info.height;
    Ok((out.pixels.into_vec(), w, h))
}

#[inline]
fn encode_coarsen(jpeg_bytes: &[u8], scale: f32, effort: u8) -> Result<Vec<u8>, At<JxlError>> {
    encode_jpeg_recompress_auto_codestream(jpeg_bytes, scale, effort)
        .map_err(|e| whereat::at!(JxlError::Encode(jxl_encoder::EncodeError::from(e))))
}

/// Re-encode tightly-packed RGB8 pixels as a VarDCT JXL at the given `distance`
/// (0 = lossless, larger = coarser). The pixels are the source's own decoded
/// image (lossless-transcode reference), so this is the "decode → re-encode"
/// path without resurrecting frequencies the source already discarded.
fn encode_pixel(
    ref_px: &[u8],
    w: u32,
    h: u32,
    distance: f32,
    effort: u8,
) -> Result<Vec<u8>, At<JxlError>> {
    let cfg = jxl_encoder::LossyConfig::new(distance).with_effort(effort);
    let pixels: Vec<rgb::Rgb<u8>> = ref_px
        .chunks_exact(3)
        .map(|c| rgb::Rgb {
            r: c[0],
            g: c[1],
            b: c[2],
        })
        .collect();
    let img = imgref::ImgRef::new(&pixels, w as usize, h as usize);
    jxl_encoder::convenience::encode_rgb8(img, &cfg).map_err(|e| whereat::at!(JxlError::Encode(e)))
}
