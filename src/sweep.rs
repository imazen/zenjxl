//! Budgeted sweep-plan construction over the JXL encoder knob space.
//!
//! Port of zenjpeg's variant-generation patterns (see
//! `zenjpeg/docs/VARIANT_GENERATION.md`) to the JXL wrapper. Turns the
//! combinatorial knob space — mode × effort × strategy × expert
//! internal-params × quality — into a **finite, auditable list of encode
//! cells**:
//!
//! 1. **Mode discrimination** — lossy (VarDCT) and lossless (modular)
//!    knobs live on separate variant types ([`LossyVariant`] /
//!    [`LosslessVariant`]). A lossless cell cannot spell a butteraugli
//!    distance or a noise flag; a lossy cell cannot spell an MA-tree
//!    knob. Combinations rejected by jxl-encoder's `validate()` are
//!    skipped and *reported*, never silently lost.
//! 2. **Quality grid** — [`QualityGrid`] encodes the sweep discipline
//!    (step-5 floor for benchmarks, denser grids for training; low-q
//!    coverage never thinned preferentially). The grid applies to lossy
//!    cells only: lossless strata have no quality axis, which is the
//!    grid-level form of mode discrimination.
//! 3. **Fingerprint dedup** — every cell gets a byte-identity
//!    fingerprint over its *resolved* state. The marquee alias for JXL:
//!    the generic-quality calibration table plateaus at `q <= 20` (all
//!    map to native quality 5.0 → distance 8.5), so the five lowest
//!    step-5 grid points collapse into one encode with merged ids
//!    recorded as aliases. Quality-vs-distance spellings of the same
//!    resolved distance merge the same way.
//! 4. **Budget ladder** — [`SweepBuilder::with_budget`] reduces
//!    deterministically: collapse low-tier mode axes one value at a
//!    time (recorded in [`SweepPlan::dropped`]), then coarsen the
//!    quality grid uniformly (endpoints kept, never below 11 points),
//!    and finally set [`SweepPlan::over_budget`] rather than sample
//!    silently. No silent caps.
//! 5. **Queue ordering** — cells are emitted main-effects-first: the
//!    all-defaults stratum of each mode, then every single-deviation
//!    stratum, then interaction combos, milder deviations first.
//!    Quality runs ascending *within* each lossy stratum so an RD curve
//!    is never half-measured; a truncated queue is safe at any stratum
//!    boundary. [`SweepCell::deviations`] exposes the priority class.
//!
//! # Scalar bounds and step provenance
//!
//! Curated steps come from values that already ship inside jxl-encoder
//! (effort-ladder defaults, named preset constructors, documented
//! ranges) — not from invented grids. Empirical validation lives in
//! `examples/sweep_validate.rs`; re-run it whenever these axes or the
//! fingerprint change.
//!
//! | knob | bound | curated steps (modes_full) | provenance |
//! |---|---|---|---|
//! | effort (lossy) | 1–10 | 7, 5, 9, 3, 10 | 7 = `LossyConfig::new` default; ladder semantics in jxl-encoder `effort.rs` per-effort schedules |
//! | effort (lossless) | 1–10 | 7, 5, 9, 3, 1 | same; e1 = fixed-tree fast path |
//! | strategy | enum | Zenjxl, Libjxl, LeanFaster | W44-128 bundles; Zenjxl = shipping default, Libjxl = strict-parity |
//! | encoder mode | enum | Reference, Experimental | jxl-encoder `EncoderMode` |
//! | epf_level | −1–3 | −1, 0, 3 | libjxl `--epf` range; −1 = auto (default) |
//! | gaborish | bool | default, off | effort-profile default; user-disableable mode |
//! | ans | bool | default, off | `with_ans`; off = prefix coding |
//! | progressive | enum | Single, QuantizedAcFullAc, DcVlfLfAc | `ProgressiveMode` variants |
//! | faster_decoding | 0–4 | lossy: 0, 4; lossless: 0, 2 | libjxl tiers. Lossy tier 2 = patches-off only, which never fires on photo content (validated inert); lossless tier 2 forces small groups (live; byte-aliases `group_size_shift = 0`) |
//! | noise | bool | off, on | `with_noise` synthesis |
//! | k_info_loss_mul_base | > 0 | 1.3 probe | libjxl PR #4506 experimental value (reference = 1.2) |
//! | entropy_mul_table | preset | experimental() probe | PR #4506 constructor; reference() is the Reference-mode default |
//! | lossy_search_seeds | ≥ 1 | (none — see note) | RFC#45: e9+ default. Live only under jxl-encoder's `butteraugli-loop` feature, which the default `__expert` build does not enable; not a default probe because it would be structurally inert |
//! | nb_rcts_to_try | 0–19 | 1 probe | `Some(1)` per jxl-encoder#67 (identity-RCT-only — `Some(0)` falls back to GBR_SUBGR and can coincide with the search winner). A 19-wide probe was dropped 2026-06-10: zero new winners over the 7-candidate e7 default across the validation corpus |
//! | wp_num_param_sets | 0–5 | 5 probe | effort schedule (0 at e<8, 2 at e8, 5 at e9+) |
//! | tree_max_buckets | ≥ 1 | 256 probe | effort schedule (32/48/64/96/128/256) |
//! | tree_num_properties | 1–16 | 16 probe | effort schedule (3/4/5/7/10/16) |
//! | tree_sample_fraction | 0–1 | (none) | effort schedule (0.15 at e≤4 → 0.65 at e9+); the override is not consumed upstream (jxl-encoder#69) — re-add when plumbed |
//! | tree_learn_seeds | ≥ 1 | 2 probe | RFC#45 chunk 5 (1 at e≤9, 2 at e10) |
//! | lloyd_max_buckets | bool | on probe | EX-J5 Lloyd-Max bucket boundaries |
//! | gather_dedup | bool | (none) | issue #41 Phase 2. Validated byte-identical to the sort-only path on the whole 2026-06-10 corpus (the post-sort dedup converges); stays in the fingerprint, not worth an axis slot |
//! | modular predictor | 0–15 | 6 (Weighted), 0 (Zero) | upstream predictor ids; None = per-effort selection. 5 (Gradient) and 15 byte-alias the e7 default (validated 2026-06-10) — the #67 trap |
//! | group_size_shift | 0–3 | 0 (128), 3 (1024) | `128 << shift`; None = 256 default |
//! | quality | 0–100 | grids in [`QualityGrid`] | step-5 floor / training-dense per the sweep discipline |
//!
//! **Deliberately excluded axes** (no silent caps — exclusions are
//! documented): `resampling` (changes output geometry class; belongs in
//! a dedicated downscale study), `chroma_subsampling` (non-444 modes
//! are rejected by the encoder today — issue #47 chunk 4 pending),
//! `alpha_distance` / `alpha_squeeze` / `simplify_invisible` (alpha
//! axes need an alpha corpus), `photon_noise_iso` / `manual_noise_lut`
//! (parameterized noise needs its own grid), lossless `lz77` /
//! `lz77_method` / `patches` / `palette_colors` (setters accepted but
//! not consumed by the modular path — jxl-encoder#69; they return as
//! axes when plumbed), `splines` / `force_strategy`
//! / `max_strategy_size` (debug knobs), `lossy_palette` (changes pixels
//! under a "lossless" config; needs metric-class treatment),
//! `butteraugli_iters` and the perceptual-loop family (feature-gated,
//! interacts with `butteraugli-loop` builds), container/metadata knobs
//! (orthogonal to encode params; pinned by the harness), threading
//! knobs (must be byte-neutral; pinned to 1 thread in harnesses).
//!
//! The plan is **per config-cell**; multiply by corpus images and size
//! buckets with [`SweepPlan::encodes`] to get the real encode count.
//! Persistence of encoded bytes/diffmaps and metric scoring belong to
//! the harness consuming this plan, not here.

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use jxl_encoder::api::EncoderStrategy;
use jxl_encoder::entropy_coding::ans::ANSHistogramStrategy;
use jxl_encoder::{
    EncoderMode, EntropyMulTable, LosslessConfig, LosslessInternalParams, LossyConfig,
    LossyInternalParams, ProgressiveMode, calibrated_jxl_quality, quality_to_distance,
};

// ============================================================================
// Resolution helpers (the same code the encoder runs)
// ============================================================================

/// Resolve a generic quality (0–100, libjpeg-turbo scale) to the
/// butteraugli distance the encoder will actually run with.
///
/// This is **the same two-function chain** `JxlEncoderConfig` uses when
/// `with_generic_quality` rebuilds its lossy config
/// (`quality_to_distance(calibrated_jxl_quality(q))`), so plans and
/// fingerprints cannot drift from encode reality. The calibration table
/// plateaus at `q <= 20` (native quality 5.0 → distance 8.5): grid
/// points in that range resolve to the *same* distance and fingerprint-
/// dedupe into one cell.
#[must_use]
pub fn resolve_distance_for_quality(generic_q: f32) -> f32 {
    quality_to_distance(calibrated_jxl_quality(generic_q))
}

// ============================================================================
// Variants: knobs live on the mode that uses them
// ============================================================================

/// A labelled [`LossyInternalParams`] bundle for use as a sweep-axis
/// value. The label is the id token; keep it short, unique within the
/// axis, and stable across runs.
#[derive(Clone, Debug)]
pub struct NamedLossyParams {
    /// Short id token (e.g. `"def"`, `"dct64off"`).
    pub label: String,
    /// The overrides; all-`None` keeps every effort-derived default.
    pub params: LossyInternalParams,
}

impl NamedLossyParams {
    /// The all-defaults bundle (label `"def"`).
    #[must_use]
    pub fn default_probe() -> Self {
        Self {
            label: "def".to_owned(),
            params: LossyInternalParams::default(),
        }
    }

    /// A labelled override bundle.
    #[must_use]
    pub fn new(label: &str, params: LossyInternalParams) -> Self {
        Self {
            label: label.to_owned(),
            params,
        }
    }
}

/// A labelled [`LosslessInternalParams`] bundle for use as a sweep-axis
/// value.
#[derive(Clone, Debug)]
pub struct NamedLosslessParams {
    /// Short id token (e.g. `"def"`, `"rct1"`).
    pub label: String,
    /// The overrides; all-`None` keeps every effort-derived default.
    pub params: LosslessInternalParams,
}

impl NamedLosslessParams {
    /// The all-defaults bundle (label `"def"`).
    #[must_use]
    pub fn default_probe() -> Self {
        Self {
            label: "def".to_owned(),
            params: LosslessInternalParams::default(),
        }
    }

    /// A labelled override bundle.
    #[must_use]
    pub fn new(label: &str, params: LosslessInternalParams) -> Self {
        Self {
            label: label.to_owned(),
            params,
        }
    }
}

/// One lossy (VarDCT) encode variant with its resolved distance.
///
/// Only knobs that are live on the lossy path exist here; modular tree
/// knobs are structurally unspellable (they live on
/// [`LosslessVariant`]).
#[derive(Clone, Debug)]
pub struct LossyVariant {
    /// Resolved butteraugli distance (> 0). When the cell came from a
    /// generic-quality grid this was produced by
    /// [`resolve_distance_for_quality`].
    pub distance: f32,
    /// Effort 1–10 (7 = upstream default).
    pub effort: u8,
    /// W44-128 improvements bundle.
    pub strategy: EncoderStrategy,
    /// Reference (libjxl algorithm parity) vs Experimental.
    pub encoder_mode: EncoderMode,
    /// Expert internal-params overrides (label + bundle).
    pub internal: NamedLossyParams,
    /// `None` = effort-profile default; `Some` pins the gaborish filter.
    pub gaborish: Option<bool>,
    /// Edge-preserving filter level: −1 auto (default), 0 off, 1–3 fixed.
    pub epf_level: i8,
    /// Progressive rendering mode.
    pub progressive: ProgressiveMode,
    /// Film-grain noise synthesis.
    pub noise: bool,
    /// libjxl `--faster_decoding` tier 0–4.
    pub faster_decoding: u8,
    /// `None` = effort-profile default; `Some(false)` = prefix coding.
    pub ans: Option<bool>,
}

impl LossyVariant {
    /// Build the actual encoder config. `with_effort` is applied first
    /// because it re-derives profile defaults (gaborish, ans, epf, …);
    /// pinned knobs are applied after so they survive.
    #[must_use]
    pub fn build(&self) -> LossyConfig {
        let mut c = LossyConfig::new(self.distance)
            .with_effort(self.effort)
            .with_mode(self.encoder_mode)
            .with_strategy(self.strategy.clone())
            .with_epf_level(self.epf_level)
            .with_progressive(self.progressive)
            .with_noise(self.noise)
            .with_faster_decoding(self.faster_decoding)
            .with_internal_params(self.internal.params.clone());
        if let Some(g) = self.gaborish {
            c = c.with_gaborish(g);
        }
        if let Some(a) = self.ans {
            c = c.with_ans(a);
        }
        c
    }
}

/// One lossless (modular) encode variant.
///
/// No distance, no noise, no VarDCT knobs — those are structurally
/// unspellable here. Also deliberately absent: `lz77`, `lz77_method`,
/// `patches`, and `palette_colors` — those `LosslessConfig` setters are
/// accepted but not consumed by the modular path today
/// (jxl-encoder#69, proven inert in both directions by
/// `sweep_validate` 2026-06-10, including palette's best-case 2-color
/// checkerboard). Knobs live on the variant only when they act; these
/// return as axes when upstream plumbs them.
#[derive(Clone, Debug)]
pub struct LosslessVariant {
    /// Effort 1–10 (7 = upstream default).
    pub effort: u8,
    /// Reference vs Experimental. NOTE: `EffortProfile::lossless` is
    /// currently mode-invariant (validated inert at e7 across the
    /// harness corpus), so curated axes sweep `Reference` only; the
    /// field stays because the upstream profile constructor consumes
    /// it and Experimental divergences may ship later.
    pub encoder_mode: EncoderMode,
    /// Expert internal-params overrides (label + bundle).
    pub internal: NamedLosslessParams,
    /// `None` = per-effort predictor selection; `Some(0..=15)` forces
    /// one. Plumbing validated 2026-06-10: `Some(0)` (Zero) inflates a
    /// CID22-512 photo 3×; beware default-aliases — `Some(5)`
    /// (Gradient) IS the e7 default selection and byte-aliases `None`
    /// (the jxl-encoder#67 "override equals fallback" trap).
    pub predictor: Option<u8>,
    /// Modular group dimension `128 << shift`; `None` = 256 default.
    pub group_size_shift: Option<u8>,
    /// libjxl `--faster_decoding` tier 0–4. Tier 2 forces small groups
    /// on the modular path (its output aliases `group_size_shift =
    /// Some(0)` — distinct fingerprints, identical bytes; an accepted
    /// under-merge).
    pub faster_decoding: u8,
}

impl LosslessVariant {
    /// Build the actual encoder config. `with_effort` first, pinned
    /// knobs after (same re-derivation caveat as [`LossyVariant::build`]).
    #[must_use]
    pub fn build(&self) -> LosslessConfig {
        LosslessConfig::new()
            .with_effort(self.effort)
            .with_mode(self.encoder_mode)
            .with_modular_predictor(self.predictor)
            .with_modular_group_size(self.group_size_shift)
            .with_faster_decoding(self.faster_decoding)
            .with_internal_params(self.internal.params.clone())
    }
}

/// A mode-discriminated sweep variant: the cell-level form of "knobs
/// live on the variant that uses them".
#[derive(Clone, Debug)]
pub enum SweepVariant {
    /// Lossy VarDCT cell (carries its resolved distance).
    Lossy(LossyVariant),
    /// Lossless modular cell (no quality axis).
    Lossless(LosslessVariant),
}

/// A built, ready-to-encode config (mirror of the variant split).
#[derive(Clone, Debug)]
pub enum BuiltConfig {
    /// Lossy VarDCT config.
    Lossy(LossyConfig),
    /// Lossless modular config.
    Lossless(LosslessConfig),
}

impl BuiltConfig {
    /// Fail-fast validation via jxl-encoder's own `validate()`.
    pub fn validate(&self) -> Result<(), jxl_encoder::ValidationError> {
        match self {
            Self::Lossy(c) => c.validate(),
            Self::Lossless(c) => c.validate(),
        }
    }
}

impl SweepVariant {
    /// Build the actual encoder config for this variant.
    #[must_use]
    pub fn build(&self) -> BuiltConfig {
        match self {
            Self::Lossy(v) => BuiltConfig::Lossy(v.build()),
            Self::Lossless(v) => BuiltConfig::Lossless(v.build()),
        }
    }
}

// ============================================================================
// Axes
// ============================================================================

/// Concrete values per lossy categorical axis, **most-important value
/// first** (index 0 is the production default; the budget ladder sheds
/// from the end).
#[derive(Clone, Debug)]
pub struct LossyAxes {
    /// Effort levels (floor 3 under the budget ladder).
    pub efforts: Vec<u8>,
    /// Improvements bundles (floor 2: Zenjxl + Libjxl parity).
    pub strategies: Vec<EncoderStrategy>,
    /// Reference / Experimental.
    pub encoder_modes: Vec<EncoderMode>,
    /// Expert internal-params probes (labelled; floor 1 = `"def"`).
    pub internal: Vec<NamedLossyParams>,
    /// Gaborish pin (None = effort default).
    pub gaborish: Vec<Option<bool>>,
    /// EPF levels.
    pub epf_levels: Vec<i8>,
    /// Progressive modes.
    pub progressive: Vec<ProgressiveMode>,
    /// Noise synthesis.
    pub noise: Vec<bool>,
    /// Faster-decoding tiers.
    pub faster_decoding: Vec<u8>,
    /// ANS pin (None = effort default; Some(false) = prefix coding).
    pub ans: Vec<Option<bool>>,
}

/// Concrete values per lossless categorical axis, most-important first.
/// (`lz77` / `palette_colors` are deliberately absent — jxl-encoder#69.)
#[derive(Clone, Debug)]
pub struct LosslessAxes {
    /// Effort levels (floor 3 under the budget ladder).
    pub efforts: Vec<u8>,
    /// Reference / Experimental (curated axes use Reference only —
    /// the lossless profile is mode-invariant today).
    pub encoder_modes: Vec<EncoderMode>,
    /// Expert internal-params probes (labelled; floor 1 = `"def"`).
    pub internal: Vec<NamedLosslessParams>,
    /// Forced predictors (None = per-effort selection).
    pub predictors: Vec<Option<u8>>,
    /// Group-size shifts (None = 256 default).
    pub group_size_shifts: Vec<Option<u8>>,
    /// Faster-decoding tiers.
    pub faster_decoding: Vec<u8>,
}

/// The full axis bundle: either or both modes. `None` = that mode is
/// not swept at all (its knob space is structurally absent, not zeroed).
#[derive(Clone, Debug)]
pub struct SweepAxes {
    /// Lossy (VarDCT) axes; cells multiply by the quality grid.
    pub lossy: Option<LossyAxes>,
    /// Lossless (modular) axes; one cell per stratum.
    pub lossless: Option<LosslessAxes>,
}

impl LossyAxes {
    /// The axes that move the lossy rate-distortion front, everything
    /// else at production defaults: effort {7, 5, 9} × strategy
    /// {Zenjxl, Libjxl}.
    #[must_use]
    pub fn rd_core() -> Self {
        Self {
            efforts: vec![7, 5, 9],
            strategies: vec![EncoderStrategy::Zenjxl, EncoderStrategy::Libjxl],
            encoder_modes: vec![EncoderMode::Reference],
            internal: vec![NamedLossyParams::default_probe()],
            gaborish: vec![None],
            epf_levels: vec![-1],
            progressive: vec![ProgressiveMode::Single],
            noise: vec![false],
            faster_decoding: vec![0],
            ans: vec![None],
        }
    }

    /// Every user-disableable lossy mode axis on top of
    /// [`rd_core`](Self::rd_core), plus the expert internal-params
    /// probes from the provenance table. Large — pair with
    /// [`SweepBuilder::with_budget`].
    #[must_use]
    pub fn modes_full() -> Self {
        let mut axes = Self::rd_core();
        axes.efforts.extend([3, 10]);
        axes.strategies.push(EncoderStrategy::LeanFaster);
        axes.encoder_modes.push(EncoderMode::Experimental);
        axes.gaborish.push(Some(false));
        axes.epf_levels.extend([0, 3]);
        axes.progressive.extend([
            ProgressiveMode::QuantizedAcFullAc,
            ProgressiveMode::DcVlfLfAc,
        ]);
        axes.noise.push(true);
        // Tier 4 only: tier 2's lossy-side effect is patches-off, and
        // the patches detector produces nothing on photo-class content
        // (validated inert 0/42 on the harness corpus — the lossless
        // path keeps tier 2, where it forces small groups and fires
        // everywhere).
        axes.faster_decoding.push(4);
        axes.ans.push(Some(false));
        axes.internal.extend(lossy_internal_probes());
        axes
    }
}

/// The curated single-knob lossy internal-params probes (provenance in
/// the module docs table). Each probe deviates in exactly one field so
/// dev-1 strata answer "does this knob matter".
#[must_use]
pub fn lossy_internal_probes() -> Vec<NamedLossyParams> {
    let mut probes = Vec::new();
    // Labels are id tokens: no '-' (the flag separator) or '_' (the
    // token separator) inside a label, or downstream id parsing breaks.
    let mut p = LossyInternalParams::default();
    p.entropy_mul_table = Some(EntropyMulTable::experimental());
    probes.push(NamedLossyParams::new("emulexp", p));

    // try_dct32 is NOT probed: 32×32-class merges never won on the
    // validation corpus at e7 (0/42 byte changes — W44-68/W44-123
    // suppression composes on gated content and 16/64-class merges
    // shadow the rest), so the probe carried no signal. dct16off and
    // dct64off keep the DCT-class coverage; sweep try_dct32 under a
    // Libjxl-strategy stratum when studying the gate interactions.
    for (label, set) in [("dct16off", 16u8), ("dct64off", 64), ("dct4x8off", 48)] {
        let mut p = LossyInternalParams::default();
        match set {
            16 => p.try_dct16 = Some(false),
            64 => p.try_dct64 = Some(false),
            _ => p.try_dct4x8_afv = Some(false),
        }
        probes.push(NamedLossyParams::new(label, p));
    }

    // chromacity_adjustment defaults ON at e7+ and acts per-pixel —
    // live on every content class (unlike the gate-shadowed dct32).
    let mut p = LossyInternalParams::default();
    p.chromacity_adjustment = Some(false);
    probes.push(NamedLossyParams::new("chroma0", p));

    let mut p = LossyInternalParams::default();
    p.non_aligned_eval = Some(false);
    probes.push(NamedLossyParams::new("nonalign0", p));

    let mut p = LossyInternalParams::default();
    p.cfl_two_pass = Some(false);
    probes.push(NamedLossyParams::new("cfl1pass", p));

    let mut p = LossyInternalParams::default();
    p.k_info_loss_mul_base = Some(1.3);
    probes.push(NamedLossyParams::new("kinfo1.3", p));

    // NOTE: `lossy_search_seeds` is deliberately NOT a default probe —
    // it acts inside the butteraugli quality loop, which only exists
    // when jxl-encoder's `butteraugli-loop` feature is compiled in
    // (zenjxl's `__expert` build does not enable it). A knob that is
    // structurally dead under the build config would be a guaranteed
    // inert step. Add it to custom axes only when sweeping a
    // `butteraugli-loop` build.

    let mut p = LossyInternalParams::default();
    p.ans_histogram_strategy_vardct = Some(ANSHistogramStrategy::Fast);
    probes.push(NamedLossyParams::new("ansfast", p));

    probes
}

impl LosslessAxes {
    /// Lossless RD core: the effort ladder {7, 5, 9} at production
    /// defaults. Lossless output is pixel-exact by definition, so
    /// "RD" here is bytes-vs-CPU.
    ///
    /// jxl-encoder#68 (e9+ lossless emitted undecodable bitstreams)
    /// was caught by `sweep_validate`'s roundtrip gate on 2026-06-10
    /// and fixed upstream same-day — two independent causes
    /// (`5eefe5f7` mid-group ref-property stride truncation,
    /// `329f207d` spec-divergent group_id stream numbering). The
    /// harness runs fully green since; the e9 axis value staying put
    /// through the red phase is what forced both root causes out.
    #[must_use]
    pub fn rd_core() -> Self {
        Self {
            efforts: vec![7, 5, 9],
            encoder_modes: vec![EncoderMode::Reference],
            internal: vec![NamedLosslessParams::default_probe()],
            predictors: vec![None],
            group_size_shifts: vec![None],
            faster_decoding: vec![0],
        }
    }

    /// Every *live* user-disableable lossless mode axis on top of
    /// [`rd_core`](Self::rd_core), plus the expert internal-params
    /// probes from the provenance table. (lz77 / palette / patches
    /// setters are not consumed by the modular path today —
    /// jxl-encoder#69 — and Experimental mode is profile-invariant for
    /// lossless, so none of those are axes.)
    #[must_use]
    pub fn modes_full() -> Self {
        let mut axes = Self::rd_core();
        axes.efforts.extend([3, 1]);
        // Predictor probes chosen for proven liveness (validated
        // 2026-06-10): 6 = Weighted (a real alternative selection),
        // 0 = Zero (extreme negative control, ~3× bytes on photos).
        // 5 (Gradient) and 15 (Variable) byte-alias the e7 default.
        axes.predictors.extend([Some(6), Some(0)]);
        axes.group_size_shifts.extend([Some(0), Some(3)]);
        axes.faster_decoding.push(2);
        axes.internal.extend(lossless_internal_probes());
        axes
    }
}

/// The curated single-knob lossless internal-params probes (provenance
/// in the module docs table). `rct1` uses `Some(1)` — identity-RCT-only
/// — as the override-propagation signal per jxl-encoder#67 (`Some(0)`
/// falls back to GBR_SUBGR, which the default search often picks anyway,
/// so 0-vs-default can be byte-identical by content coincidence).
///
/// Dropped after the 2026-06-10 validation run, each with its reason:
/// - `rct19` (search width 7→19): zero new winners across the 7-image
///   corpus — the probe carried no signal; `rct1` keeps the
///   override-liveness coverage.
/// - `frac065` (`tree_sample_fraction`): the override is not consumed
///   upstream (jxl-encoder#69) — a structurally-dead probe.
/// - `gatherdedup`: byte-identical to the sort-only path on the whole
///   corpus (the post-`pre_quantize` sort dedup converges to the same
///   surviving set). It REMAINS in the fingerprint (upstream documents
///   content where bytes can differ); it is just not worth a curated
///   axis slot.
#[must_use]
pub fn lossless_internal_probes() -> Vec<NamedLosslessParams> {
    let mut probes = Vec::new();

    let mut p = LosslessInternalParams::default();
    p.nb_rcts_to_try = Some(1);
    probes.push(NamedLosslessParams::new("rct1", p));

    let mut p = LosslessInternalParams::default();
    p.wp_num_param_sets = Some(5);
    probes.push(NamedLosslessParams::new("wp5", p));

    let mut p = LosslessInternalParams::default();
    p.tree_max_buckets = Some(256);
    probes.push(NamedLosslessParams::new("buckets256", p));

    let mut p = LosslessInternalParams::default();
    p.tree_num_properties = Some(16);
    probes.push(NamedLosslessParams::new("props16", p));

    let mut p = LosslessInternalParams::default();
    p.tree_learn_seeds = Some(2);
    probes.push(NamedLosslessParams::new("seeds2", p));

    let mut p = LosslessInternalParams::default();
    p.lloyd_max_buckets = Some(true);
    probes.push(NamedLosslessParams::new("lloyd", p));

    probes
}

impl SweepAxes {
    /// RD-front core for both modes: lossy effort×strategy plus the
    /// lossless effort ladder.
    #[must_use]
    pub fn rd_core() -> Self {
        Self {
            lossy: Some(LossyAxes::rd_core()),
            lossless: Some(LosslessAxes::rd_core()),
        }
    }

    /// Every user-disableable mode axis for both modes (the calibration
    /// mandate). Large — pair with [`SweepBuilder::with_budget`].
    #[must_use]
    pub fn modes_full() -> Self {
        Self {
            lossy: Some(LossyAxes::modes_full()),
            lossless: Some(LosslessAxes::modes_full()),
        }
    }
}

// ============================================================================
// Quality grid
// ============================================================================

/// Quality grids per the sweep discipline. Low-q density is never below
/// high-q density. Applies to lossy cells only.
#[derive(Clone, Debug)]
pub enum QualityGrid {
    /// q ∈ {1, 5, 10, …, 100} — the 21-point floor for benchmarks and
    /// anchor tables.
    Step5,
    /// Step 5 through q65, step 2 from q70 — the training-density grid
    /// (31 points).
    TrainingDense,
    /// Caller-provided generic-quality points (kept in the given order,
    /// deduplicated).
    ExplicitQuality(Vec<f32>),
    /// Caller-provided butteraugli distances (kept in the given order,
    /// deduplicated). Cells from this grid have
    /// [`SweepCell::quality`]` == None`; the distance lives on the
    /// variant.
    ExplicitDistance(Vec<f32>),
}

impl QualityGrid {
    /// Materialize the grid points as `(generic_q, resolved_distance)`.
    /// Generic-q grids resolve through [`resolve_distance_for_quality`]
    /// (the same chain the encoder runs).
    #[must_use]
    pub fn points(&self) -> Vec<(Option<f32>, f32)> {
        let qs: Vec<f32> = match self {
            Self::Step5 => {
                let mut v = vec![1.0];
                v.extend((1..=20).map(|i| (i * 5) as f32));
                v
            }
            Self::TrainingDense => {
                let mut v = vec![1.0];
                v.extend((1..=13).map(|i| (i * 5) as f32)); // 5..=65
                v.extend((35..=50).map(|i| (i * 2) as f32)); // 70..=100
                v
            }
            Self::ExplicitQuality(pts) => dedup_keep_order(pts),
            Self::ExplicitDistance(pts) => {
                return dedup_keep_order(pts)
                    .into_iter()
                    .map(|d| (None, d))
                    .collect();
            }
        };
        qs.into_iter()
            .map(|q| (Some(q), resolve_distance_for_quality(q)))
            .collect()
    }
}

fn dedup_keep_order(pts: &[f32]) -> Vec<f32> {
    let mut v = Vec::new();
    for &p in pts {
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

// ============================================================================
// Plan output
// ============================================================================

/// One encode cell: a fully-described variant (lossy cells carry their
/// resolved distance; lossless cells have no quality axis).
#[derive(Clone, Debug)]
pub struct SweepCell {
    /// Stable human-readable id (mode/effort/strategy/internal tokens +
    /// deviation flags + `_q…`/`_d…` for lossy cells).
    pub id: String,
    /// The variant to encode with.
    pub variant: SweepVariant,
    /// The generic-quality grid point, when the cell came from a
    /// generic-quality grid. `None` for lossless cells and
    /// [`QualityGrid::ExplicitDistance`] cells.
    pub quality: Option<f32>,
    /// Byte-identity fingerprint of the resolved state. Cells with
    /// equal fingerprints produce identical bytes for the same input.
    pub fingerprint: u64,
    /// Ids of candidate cells merged into this one (identical
    /// fingerprints).
    pub aliases: Vec<String>,
    /// How many axes deviate from the default stratum of the cell's
    /// mode (index 0 of every axis). 0 = the production-default cell;
    /// 1 = a main-effect probe; ≥2 = interaction combos. Cells are
    /// emitted in ascending order.
    pub deviations: u8,
}

impl SweepCell {
    /// Build the actual encoder config for this cell.
    #[must_use]
    pub fn build(&self) -> BuiltConfig {
        self.variant.build()
    }
}

/// A mode axis collapsed by the budget ladder.
#[derive(Clone, Debug)]
pub struct DroppedAxis {
    /// Axis name (`"lossy.ans"`, `"lossless.predictors"`, …).
    pub axis: &'static str,
    /// The values kept (Debug-rendered).
    pub kept: String,
    /// The values dropped (Debug-rendered).
    pub dropped: Vec<String>,
}

/// The finite, auditable sweep plan.
#[derive(Clone, Debug)]
pub struct SweepPlan {
    /// Deduplicated encode cells, main-effects-first.
    pub cells: Vec<SweepCell>,
    /// Stratum/cell ids rejected by jxl-encoder `validate()` (or by a
    /// non-positive resolved distance).
    pub invalid_skipped: Vec<String>,
    /// Mode axes collapsed to fit the budget — the explicit
    /// no-silent-caps report.
    pub dropped: Vec<DroppedAxis>,
    /// Candidate cells merged by fingerprint identity.
    pub duplicates_merged: usize,
    /// How many times the quality grid was uniformly coarsened.
    pub q_coarsenings: u32,
    /// The budget could not be met even after the full reduction
    /// ladder. The plan is complete (nothing was sampled away); the
    /// caller decides whether to spend or cut axes manually.
    pub over_budget: bool,
}

impl SweepPlan {
    /// Total encodes when this plan runs over a corpus: cells × images ×
    /// size buckets.
    #[must_use]
    pub fn encodes(&self, images: usize, size_buckets: usize) -> usize {
        self.cells.len() * images * size_buckets
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builds a [`SweepPlan`] from axes × quality grid under an optional
/// encode-cell budget.
#[derive(Clone, Debug)]
pub struct SweepBuilder {
    axes: SweepAxes,
    grid: QualityGrid,
    budget: Option<usize>,
}

impl SweepBuilder {
    /// New builder over the given axes and quality grid (the grid
    /// applies to lossy cells only).
    #[must_use]
    pub fn new(axes: SweepAxes, grid: QualityGrid) -> Self {
        Self {
            axes,
            grid,
            budget: None,
        }
    }

    /// Cap the number of (deduplicated) cells. The reduction ladder
    /// collapses lossy mode axes lowest-tier-first (ans,
    /// faster_decoding, noise, progressive, epf, gaborish,
    /// encoder_modes, internal probes, strategies to a floor of 2),
    /// then the lossless axes (faster_decoding, group size,
    /// predictors, encoder_modes, internal probes), then
    /// coarsens the quality grid (uniformly, endpoints kept, ≥ 11
    /// points). Efforts are never reduced below their rd_core floor of
    /// 3. Every reduction is recorded.
    #[must_use]
    pub fn with_budget(mut self, max_cells: usize) -> Self {
        self.budget = Some(max_cells);
        self
    }

    /// Build the plan.
    #[must_use]
    pub fn plan(&self) -> SweepPlan {
        let mut axes = self.axes.clone();
        let mut q_points = self.grid.points();
        let mut dropped = Vec::new();
        let mut q_coarsenings = 0u32;
        let mut over_budget = false;

        loop {
            let (cells, invalid_skipped, duplicates_merged) = cross(&axes, &q_points);

            let within = match self.budget {
                None => true,
                Some(b) => cells.len() <= b,
            };
            if within {
                return SweepPlan {
                    cells,
                    invalid_skipped,
                    dropped,
                    duplicates_merged,
                    q_coarsenings,
                    over_budget,
                };
            }

            // Reduction ladder, one step per iteration.
            if let Some(d) = collapse_one_axis(&mut axes) {
                // Coalesce repeated single-value drops of the same axis.
                if let Some(last) = dropped.last_mut()
                    && last.axis == d.axis
                {
                    last.dropped.extend(d.dropped);
                    last.kept = d.kept;
                    continue;
                }
                dropped.push(d);
                continue;
            }
            if q_points.len() > 11 {
                q_points = coarsen_keep_endpoints(&q_points);
                q_coarsenings += 1;
                continue;
            }

            // Nothing left to reduce: report rather than sample.
            over_budget = true;
            let (cells, invalid_skipped, duplicates_merged) = cross(&axes, &q_points);
            return SweepPlan {
                cells,
                invalid_skipped,
                dropped,
                duplicates_merged,
                q_coarsenings,
                over_budget,
            };
        }
    }
}

fn collapse<T: core::fmt::Debug + Clone>(
    name: &'static str,
    v: &mut Vec<T>,
    floor: usize,
) -> Option<DroppedAxis> {
    // Shed ONE value per ladder step — the last (lowest-priority)
    // entry — so the budget is approached from above instead of
    // overshot by whole-axis removals. Axis vecs are ordered
    // most-important-first.
    if v.len() <= floor {
        return None;
    }
    let dropped = vec![format!("{:?}", v[v.len() - 1])];
    v.truncate(v.len() - 1);
    let kept = v
        .iter()
        .map(|x| format!("{x:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(DroppedAxis {
        axis: name,
        kept,
        dropped,
    })
}

fn collapse_named<T: Clone>(
    name: &'static str,
    v: &mut Vec<T>,
    floor: usize,
    label: impl Fn(&T) -> String,
) -> Option<DroppedAxis> {
    if v.len() <= floor {
        return None;
    }
    let dropped = vec![label(&v[v.len() - 1])];
    v.truncate(v.len() - 1);
    let kept = v.iter().map(&label).collect::<Vec<_>>().join(", ");
    Some(DroppedAxis {
        axis: name,
        kept,
        dropped,
    })
}

/// Collapse the lowest-tier multi-valued axis to its floor, lossy axes
/// first (they multiply by the quality grid, so they dominate the cell
/// count), then lossless. Efforts have a floor of 3 (the rd_core
/// ladder); strategies a floor of 2 (Zenjxl + Libjxl parity).
fn collapse_one_axis(axes: &mut SweepAxes) -> Option<DroppedAxis> {
    if let Some(lossy) = axes.lossy.as_mut() {
        let d = collapse("lossy.ans", &mut lossy.ans, 1)
            .or_else(|| collapse("lossy.faster_decoding", &mut lossy.faster_decoding, 1))
            .or_else(|| collapse("lossy.noise", &mut lossy.noise, 1))
            .or_else(|| collapse("lossy.progressive", &mut lossy.progressive, 1))
            .or_else(|| collapse("lossy.epf_levels", &mut lossy.epf_levels, 1))
            .or_else(|| collapse("lossy.gaborish", &mut lossy.gaborish, 1))
            .or_else(|| collapse("lossy.encoder_modes", &mut lossy.encoder_modes, 1))
            .or_else(|| {
                collapse_named("lossy.internal", &mut lossy.internal, 1, |p| {
                    p.label.clone()
                })
            })
            .or_else(|| collapse("lossy.strategies", &mut lossy.strategies, 2))
            .or_else(|| collapse("lossy.efforts", &mut lossy.efforts, 3));
        if d.is_some() {
            return d;
        }
    }
    if let Some(ll) = axes.lossless.as_mut() {
        let d = collapse("lossless.faster_decoding", &mut ll.faster_decoding, 1)
            .or_else(|| collapse("lossless.group_size_shifts", &mut ll.group_size_shifts, 1))
            .or_else(|| collapse("lossless.predictors", &mut ll.predictors, 1))
            .or_else(|| collapse("lossless.encoder_modes", &mut ll.encoder_modes, 1))
            .or_else(|| {
                collapse_named("lossless.internal", &mut ll.internal, 1, |p| {
                    p.label.clone()
                })
            })
            .or_else(|| collapse("lossless.efforts", &mut ll.efforts, 3));
        if d.is_some() {
            return d;
        }
    }
    None
}

/// Drop every second interior point (endpoints kept).
fn coarsen_keep_endpoints(points: &[(Option<f32>, f32)]) -> Vec<(Option<f32>, f32)> {
    let last = points.len() - 1;
    points
        .iter()
        .enumerate()
        .filter(|(i, _)| *i == 0 || *i == last || i % 2 == 0)
        .map(|(_, &p)| p)
        .collect()
}

// ============================================================================
// Cross product
// ============================================================================

fn strategy_token(s: &EncoderStrategy) -> &'static str {
    match s {
        EncoderStrategy::Libjxl => "libjxl",
        EncoderStrategy::LeanFaster => "lean",
        EncoderStrategy::Zenjxl => "zen",
        EncoderStrategy::Aggressive => "aggr",
        EncoderStrategy::Custom(_) => "custom",
    }
}

impl LossyVariant {
    /// Base id (no quality token): `vd-e{eff}_{strategy}_{internal}` +
    /// '-'-joined deviation flags.
    fn base_id(&self) -> String {
        let mut s = format!(
            "vd-e{}_{}_{}",
            self.effort,
            strategy_token(&self.strategy),
            self.internal.label
        );
        if let EncoderStrategy::Custom(c) = &self.strategy {
            // Compact content hash keeps custom-bundle ids unique
            // without leaking the whole Debug dump into every row.
            let mut h = Fnv::new();
            h.write(format!("{c:?}").as_bytes());
            s.push_str(&format!("#{:04x}", h.0 & 0xffff));
        }
        if self.encoder_mode == EncoderMode::Experimental {
            s.push_str("-exp");
        }
        match self.gaborish {
            None => {}
            Some(true) => s.push_str("-gab1"),
            Some(false) => s.push_str("-gab0"),
        }
        if self.epf_level != -1 {
            s.push_str(&format!("-epf{}", self.epf_level));
        }
        match self.progressive {
            ProgressiveMode::Single => {}
            ProgressiveMode::QuantizedAcFullAc => s.push_str("-prog1"),
            ProgressiveMode::DcVlfLfAc => s.push_str("-prog2"),
        }
        if self.noise {
            s.push_str("-noise");
        }
        if self.faster_decoding != 0 {
            s.push_str(&format!("-fd{}", self.faster_decoding));
        }
        match self.ans {
            None => {}
            Some(true) => s.push_str("-ans1"),
            Some(false) => s.push_str("-ans0"),
        }
        s
    }
}

impl LosslessVariant {
    /// Base id: `mod-e{eff}_{internal}` + '-'-joined deviation flags.
    fn base_id(&self) -> String {
        let mut s = format!("mod-e{}_{}", self.effort, self.internal.label);
        if self.encoder_mode == EncoderMode::Experimental {
            s.push_str("-exp");
        }
        if let Some(p) = self.predictor {
            s.push_str(&format!("-pred{p}"));
        }
        if let Some(g) = self.group_size_shift {
            s.push_str(&format!("-gss{g}"));
        }
        if self.faster_decoding != 0 {
            s.push_str(&format!("-fd{}", self.faster_decoding));
        }
        s
    }
}

/// One enumerated stratum with its queue-ordering keys.
struct Entry {
    variant: SweepVariant,
    base_id: String,
    deviations: u8,
    mode_rank: u8,
    idx_sum: usize,
    seq: usize,
}

/// Cross axes × quality points into deduplicated, priority-ordered
/// cells.
fn cross(
    axes: &SweepAxes,
    q_points: &[(Option<f32>, f32)],
) -> (Vec<SweepCell>, Vec<String>, usize) {
    let mut entries: Vec<Entry> = Vec::new();
    let mut invalid: Vec<String> = Vec::new();
    let mut seq = 0usize;

    // Pass 1: enumerate strata with per-axis value indices. Lossy
    // stratum validity is distance-independent, so it is checked once
    // per stratum at a probe distance; non-positive grid distances are
    // reported per-point below.
    if let Some(lossy) = &axes.lossy {
        for (ei, &effort) in lossy.efforts.iter().enumerate() {
            for (si, strategy) in lossy.strategies.iter().enumerate() {
                for (mi, &encoder_mode) in lossy.encoder_modes.iter().enumerate() {
                    for (ii, internal) in lossy.internal.iter().enumerate() {
                        for (gi, &gaborish) in lossy.gaborish.iter().enumerate() {
                            for (pi, &epf_level) in lossy.epf_levels.iter().enumerate() {
                                for (ri, &progressive) in lossy.progressive.iter().enumerate() {
                                    for (ni, &noise) in lossy.noise.iter().enumerate() {
                                        for (fi, &faster_decoding) in
                                            lossy.faster_decoding.iter().enumerate()
                                        {
                                            for (ai, &ans) in lossy.ans.iter().enumerate() {
                                                let idxs = [ei, si, mi, ii, gi, pi, ri, ni, fi, ai];
                                                let v = LossyVariant {
                                                    distance: 1.0, // probe; per-cell below
                                                    effort,
                                                    strategy: strategy.clone(),
                                                    encoder_mode,
                                                    internal: internal.clone(),
                                                    gaborish,
                                                    epf_level,
                                                    progressive,
                                                    noise,
                                                    faster_decoding,
                                                    ans,
                                                };
                                                let base_id = v.base_id();
                                                if v.build().validate().is_err() {
                                                    invalid.push(base_id);
                                                    continue;
                                                }
                                                entries.push(Entry {
                                                    variant: SweepVariant::Lossy(v),
                                                    base_id,
                                                    deviations: idxs
                                                        .iter()
                                                        .filter(|&&x| x != 0)
                                                        .count()
                                                        as u8,
                                                    mode_rank: 0,
                                                    idx_sum: idxs.iter().sum(),
                                                    seq,
                                                });
                                                seq += 1;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(ll) = &axes.lossless {
        for (ei, &effort) in ll.efforts.iter().enumerate() {
            for (mi, &encoder_mode) in ll.encoder_modes.iter().enumerate() {
                for (ii, internal) in ll.internal.iter().enumerate() {
                    for (pi, &predictor) in ll.predictors.iter().enumerate() {
                        for (gi, &group_size_shift) in ll.group_size_shifts.iter().enumerate() {
                            for (fi, &faster_decoding) in ll.faster_decoding.iter().enumerate() {
                                let idxs = [ei, mi, ii, pi, gi, fi];
                                let v = LosslessVariant {
                                    effort,
                                    encoder_mode,
                                    internal: internal.clone(),
                                    predictor,
                                    group_size_shift,
                                    faster_decoding,
                                };
                                let base_id = v.base_id();
                                if v.build().validate().is_err() {
                                    invalid.push(base_id);
                                    continue;
                                }
                                entries.push(Entry {
                                    variant: SweepVariant::Lossless(v),
                                    base_id,
                                    deviations: idxs.iter().filter(|&&x| x != 0).count() as u8,
                                    mode_rank: 1,
                                    idx_sum: idxs.iter().sum(),
                                    seq,
                                });
                                seq += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // Main effects before interactions; lossy before lossless within a
    // deviation class; milder deviations before extreme ones; nested
    // order as the deterministic tie-break.
    entries.sort_by_key(|e| (e.deviations, e.mode_rank, e.idx_sum, e.seq));

    // Pass 2: expand quality (lossy only) ascending within each stratum
    // and dedupe by resolved fingerprint. Keep-first means the merged
    // cell carries the highest-priority spelling; later aliases record
    // the exotic ones.
    let mut cells: Vec<SweepCell> = Vec::new();
    let mut by_fingerprint: BTreeMap<u64, usize> = BTreeMap::new();
    let mut merged = 0usize;

    let mut push = |cell_variant: SweepVariant,
                    id: String,
                    quality: Option<f32>,
                    deviations: u8,
                    cells: &mut Vec<SweepCell>,
                    merged: &mut usize| {
        let fp = fingerprint(&cell_variant);
        if let Some(&idx) = by_fingerprint.get(&fp) {
            cells[idx].aliases.push(id);
            *merged += 1;
        } else {
            by_fingerprint.insert(fp, cells.len());
            cells.push(SweepCell {
                id,
                variant: cell_variant,
                quality,
                fingerprint: fp,
                aliases: Vec::new(),
                deviations,
            });
        }
    };

    for e in &entries {
        match &e.variant {
            SweepVariant::Lossy(v) => {
                for &(generic_q, distance) in q_points {
                    let q_token = match generic_q {
                        Some(q) => format!("_q{q}"),
                        None => format!("_d{distance}"),
                    };
                    let id = format!("{}{}", e.base_id, q_token);
                    if !(distance > 0.0 && distance.is_finite()) {
                        invalid.push(id);
                        continue;
                    }
                    let mut cv = v.clone();
                    cv.distance = distance;
                    push(
                        SweepVariant::Lossy(cv),
                        id,
                        generic_q,
                        e.deviations,
                        &mut cells,
                        &mut merged,
                    );
                }
            }
            SweepVariant::Lossless(v) => {
                push(
                    SweepVariant::Lossless(v.clone()),
                    e.base_id.clone(),
                    None,
                    e.deviations,
                    &mut cells,
                    &mut merged,
                );
            }
        }
    }
    (cells, invalid, merged)
}

// ============================================================================
// Byte-identity fingerprint
// ============================================================================
// Cell-id grammar: the stable identity contract (playbook pattern 7)
// ============================================================================

/// Reconstruct the [`SweepVariant`] a plan cell id denotes — including
/// the trailing quality/distance token, since a lossy variant carries
/// its resolved distance.
///
/// Grammar (see `LossyVariant::base_id` / `LosslessVariant::base_id`,
/// which this parser must mirror in lockstep — the
/// `cell_ids_roundtrip_to_their_variants` test enforces totality over
/// everything the planner emits):
///
/// ```text
/// lossy    = vd-e<u8>_<strategy>_<label>[-flag…](_q<f32> | _d<f32>)
/// lossless = mod-e<u8>_<label>[-flag…]
/// strategy = libjxl | lean | zen | aggr        (custom#… errors)
/// lossy flags    = exp | gab0 | gab1 | epf<i8> | prog1 | prog2
///                | noise | fd<u8> | ans0 | ans1
/// lossless flags = exp | pred<u8> | gss<u8> | fd<u8>
/// ```
///
/// Internal-params labels resolve through the curated probe registries
/// (`lossy_internal_probes` / `lossless_internal_probes`, plus `"def"`)
/// — a label not in the registry errors, as do `custom#…` strategy
/// bundles (content-hashed, not self-describing). `_q` tokens resolve
/// distance through [`resolve_distance_for_quality`], the same chain
/// the planner used. Numbers render via `Display` (lossless), so the
/// reconstruction is exact; consumers carrying the cell fingerprint
/// should verify `fingerprint(&variant)` equals it after parsing.
pub fn variant_from_cell_id(id: &str) -> Result<SweepVariant, String> {
    if id.contains('#') {
        return Err(format!(
            "cell id {id:?} carries a content-hashed custom bundle and is not self-describing"
        ));
    }
    if let Some(rest) = id.strip_prefix("vd-e") {
        let mut toks = rest.splitn(3, '_');
        let (Some(eff_s), Some(strat_s), Some(tail)) = (toks.next(), toks.next(), toks.next())
        else {
            return Err(format!("lossy id {id:?} missing tokens"));
        };
        let effort: u8 = eff_s
            .parse()
            .map_err(|e| format!("bad effort in {id:?}: {e}"))?;
        let strategy = match strat_s {
            "libjxl" => EncoderStrategy::Libjxl,
            "lean" => EncoderStrategy::LeanFaster,
            "zen" => EncoderStrategy::Zenjxl,
            "aggr" => EncoderStrategy::Aggressive,
            other => return Err(format!("unknown strategy token {other:?} in {id:?}")),
        };
        // tail = label[-flags…][_q<q> | _d<d>] — but splitn(3, '_') keeps
        // the q/d token inside `tail`; split it back off.
        let (flags_part, q_part) = match tail.rsplit_once('_') {
            Some((f, q)) if q.starts_with('q') || q.starts_with('d') => (f, Some(q)),
            _ => (tail, None),
        };
        let Some(q_tok) = q_part else {
            return Err(format!("lossy id {id:?} missing _q/_d quality token"));
        };
        let distance = if let Some(q) = q_tok.strip_prefix('q') {
            let q: f32 = q.parse().map_err(|e| format!("bad q in {id:?}: {e}"))?;
            resolve_distance_for_quality(q)
        } else if let Some(d) = q_tok.strip_prefix('d') {
            d.parse()
                .map_err(|e| format!("bad distance in {id:?}: {e}"))?
        } else {
            return Err(format!("bad quality token {q_tok:?} in {id:?}"));
        };
        let mut parts = flags_part.split('-');
        let label = parts.next().unwrap_or_default();
        let internal = lossy_params_by_label(label)
            .ok_or_else(|| format!("internal-params label {label:?} not in the registry"))?;
        let mut v = LossyVariant {
            distance,
            effort,
            strategy,
            encoder_mode: EncoderMode::Reference,
            internal,
            gaborish: None,
            epf_level: -1,
            progressive: ProgressiveMode::Single,
            noise: false,
            faster_decoding: 0,
            ans: None,
        };
        for f in parts {
            match f {
                "exp" => v.encoder_mode = EncoderMode::Experimental,
                "gab1" => v.gaborish = Some(true),
                "gab0" => v.gaborish = Some(false),
                "prog1" => v.progressive = ProgressiveMode::QuantizedAcFullAc,
                "prog2" => v.progressive = ProgressiveMode::DcVlfLfAc,
                "noise" => v.noise = true,
                "ans1" => v.ans = Some(true),
                "ans0" => v.ans = Some(false),
                f if f.starts_with("epf") => {
                    v.epf_level = f[3..]
                        .parse()
                        .map_err(|e| format!("bad epf in {id:?}: {e}"))?;
                }
                f if f.starts_with("fd") => {
                    v.faster_decoding = f[2..]
                        .parse()
                        .map_err(|e| format!("bad fd in {id:?}: {e}"))?;
                }
                other => return Err(format!("unknown lossy flag {other:?} in {id:?}")),
            }
        }
        Ok(SweepVariant::Lossy(v))
    } else if let Some(rest) = id.strip_prefix("mod-e") {
        let mut toks = rest.splitn(2, '_');
        let (Some(eff_s), Some(tail)) = (toks.next(), toks.next()) else {
            return Err(format!("lossless id {id:?} missing tokens"));
        };
        let effort: u8 = eff_s
            .parse()
            .map_err(|e| format!("bad effort in {id:?}: {e}"))?;
        let mut parts = tail.split('-');
        let label = parts.next().unwrap_or_default();
        let internal = lossless_params_by_label(label)
            .ok_or_else(|| format!("internal-params label {label:?} not in the registry"))?;
        let mut v = LosslessVariant {
            effort,
            encoder_mode: EncoderMode::Reference,
            internal,
            predictor: None,
            group_size_shift: None,
            faster_decoding: 0,
        };
        for f in parts {
            match f {
                "exp" => v.encoder_mode = EncoderMode::Experimental,
                f if f.starts_with("pred") => {
                    v.predictor = Some(
                        f[4..]
                            .parse()
                            .map_err(|e| format!("bad pred in {id:?}: {e}"))?,
                    );
                }
                f if f.starts_with("gss") => {
                    v.group_size_shift = Some(
                        f[3..]
                            .parse()
                            .map_err(|e| format!("bad gss in {id:?}: {e}"))?,
                    );
                }
                f if f.starts_with("fd") => {
                    v.faster_decoding = f[2..]
                        .parse()
                        .map_err(|e| format!("bad fd in {id:?}: {e}"))?;
                }
                other => return Err(format!("unknown lossless flag {other:?} in {id:?}")),
            }
        }
        Ok(SweepVariant::Lossless(v))
    } else {
        Err(format!(
            "cell id {id:?} is neither a vd- (lossy) nor mod- (lossless) id"
        ))
    }
}

/// Registry lookup: the curated lossy probe labels plus `"def"`.
#[must_use]
pub fn lossy_params_by_label(label: &str) -> Option<NamedLossyParams> {
    if label == "def" {
        return Some(NamedLossyParams::default_probe());
    }
    lossy_internal_probes()
        .into_iter()
        .find(|p| p.label == label)
}

/// Registry lookup: the curated lossless probe labels plus `"def"`.
#[must_use]
pub fn lossless_params_by_label(label: &str) -> Option<NamedLosslessParams> {
    if label == "def" {
        return Some(NamedLosslessParams::default_probe());
    }
    lossless_internal_probes()
        .into_iter()
        .find(|p| p.label == label)
}

// ============================================================================

struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn u8(&mut self, v: u8) {
        self.write(&[v]);
    }
    fn u16(&mut self, v: u16) {
        self.write(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.write(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.write(&v.to_bits().to_le_bytes());
    }
    fn opt_bool(&mut self, v: Option<bool>) {
        match v {
            None => self.u8(0),
            Some(false) => self.u8(1),
            Some(true) => self.u8(2),
        }
    }
    fn opt_u8(&mut self, v: Option<u8>) {
        match v {
            None => self.u8(0),
            Some(x) => {
                self.u8(1);
                self.u8(x);
            }
        }
    }
    fn opt_f32(&mut self, v: Option<f32>) {
        match v {
            None => self.u8(0),
            Some(x) => {
                self.u8(1);
                self.f32(x);
            }
        }
    }
}

/// Byte-identity fingerprint of a variant's resolved state.
///
/// Two variants with equal fingerprints produce identical bytes for the
/// same input image (with identical pinned constants: container mode,
/// metadata, thread count). Built from the RESOLVED state, so it sees
/// through aliases:
///
/// - generic quality is fully mediated by the resolved distance
///   ([`resolve_distance_for_quality`] is the same chain the encoder
///   runs) — the q ≤ 20 calibration plateau and quality-vs-distance
///   spellings of the same distance merge;
/// - `gather_dedup_phase3` is NOT hashed: jxl-encoder documents it as
///   byte-neutral (it changes the gather-dedup table implementation,
///   not the post-`pre_quantize` sort path that determines bytes) both
///   when `gather_dedup` is off (inert prerequisite) and when it is on.
///   `examples/sweep_validate.rs` proves this with encode pairs; if a
///   future jxl-encoder falsifies it, add it to the hash and bump the
///   axes.
/// - `tree_parallel_*` knobs and `smart_fanout` are scheduling-only by
///   upstream design (parallel tree learning is bitstream-equivalent);
///   they are not sweep axes and are not hashed. The harness encodes
///   alias pairs to keep that claim honest.
///
/// Everything else output-plausible IS hashed — including search-bound
/// knobs (`lossy_search_seeds`, `tree_learn_seeds`,
/// `ans_histogram_strategy_vardct`, `gather_dedup`,
/// `use_streaming_dedup`, `lloyd_max_buckets`): zenjpeg's
/// `TrellisSpeedMode` lesson is that "output-neutral by construction"
/// claims about search knobs are usually wrong, so neutrality must be
/// proven by encode before an exclusion lands here.
///
/// Note: overrides equal to the effort-derived default (e.g.
/// `nb_rcts_to_try: Some(7)` at e7) do NOT merge with `None` — the
/// per-effort defaults are not exposed by jxl-encoder, so this
/// fingerprint under-merges rather than risking a false merge.
#[must_use]
pub fn fingerprint(variant: &SweepVariant) -> u64 {
    let mut h = Fnv::new();
    match variant {
        SweepVariant::Lossy(v) => {
            h.u8(1);
            h.f32(v.distance);
            h.u8(v.effort);
            h.u8(match v.encoder_mode {
                EncoderMode::Reference => 0,
                EncoderMode::Experimental => 1,
            });
            match &v.strategy {
                EncoderStrategy::Libjxl => h.u8(0),
                EncoderStrategy::LeanFaster => h.u8(1),
                EncoderStrategy::Zenjxl => h.u8(2),
                EncoderStrategy::Aggressive => h.u8(3),
                EncoderStrategy::Custom(c) => {
                    h.u8(4);
                    h.write(format!("{c:?}").as_bytes());
                }
            }
            h.opt_bool(v.gaborish);
            h.u8(v.epf_level as u8);
            h.u8(match v.progressive {
                ProgressiveMode::Single => 0,
                ProgressiveMode::QuantizedAcFullAc => 1,
                ProgressiveMode::DcVlfLfAc => 2,
            });
            h.u8(u8::from(v.noise));
            h.u8(v.faster_decoding);
            h.opt_bool(v.ans);

            let p = &v.internal.params;
            h.opt_bool(p.try_dct16);
            h.opt_bool(p.try_dct32);
            h.opt_bool(p.try_dct64);
            h.opt_bool(p.try_dct4x8_afv);
            h.opt_u8(p.fine_grained_step);
            h.opt_f32(p.k_info_loss_mul_base);
            match &p.entropy_mul_table {
                None => h.u8(0),
                Some(t) => {
                    h.u8(1);
                    for x in [
                        t.dct8, t.dct4x4, t.dct4x8, t.identity, t.dct2x2, t.afv, t.dct16x8,
                        t.dct16x16, t.dct16x32, t.dct32x32, t.dct64x32, t.dct64x64,
                    ] {
                        h.f32(x);
                    }
                }
            }
            h.opt_bool(p.cfl_two_pass);
            h.opt_bool(p.chromacity_adjustment);
            h.opt_bool(p.patch_ref_tree_learning);
            h.opt_bool(p.non_aligned_eval);
            h.opt_bool(p.enhanced_clustering_vardct);
            match p.ans_histogram_strategy_vardct {
                None => h.u8(0),
                Some(ANSHistogramStrategy::Fast) => h.u8(1),
                Some(ANSHistogramStrategy::Approximate) => h.u8(2),
                Some(ANSHistogramStrategy::Precise) => h.u8(3),
            }
            h.opt_f32(p.k_ac_quant);
            h.opt_u8(p.lossy_search_seeds);
        }
        SweepVariant::Lossless(v) => {
            h.u8(2);
            h.u8(v.effort);
            h.u8(match v.encoder_mode {
                EncoderMode::Reference => 0,
                EncoderMode::Experimental => 1,
            });
            h.opt_u8(v.predictor);
            h.opt_u8(v.group_size_shift);
            h.u8(v.faster_decoding);

            let p = &v.internal.params;
            h.opt_u8(p.nb_rcts_to_try);
            match &p.forced_rct {
                None => h.u8(0),
                Some(rct) => {
                    h.u8(1);
                    h.write(format!("{rct:?}").as_bytes());
                }
            }
            h.opt_u8(p.wp_num_param_sets);
            match p.tree_max_buckets {
                None => h.u8(0),
                Some(x) => {
                    h.u8(1);
                    h.u16(x);
                }
            }
            h.opt_u8(p.tree_num_properties);
            h.opt_f32(p.tree_threshold_base);
            h.opt_f32(p.tree_sample_fraction);
            match p.tree_max_samples_fixed {
                None => h.u8(0),
                Some(x) => {
                    h.u8(1);
                    h.u32(x);
                }
            }
            h.opt_bool(p.use_streaming_dedup);
            h.opt_bool(p.gather_dedup);
            // gather_dedup_phase3: deliberately NOT hashed (see fn docs).
            // tree_parallel_{max_depth, floor, root_threshold,
            // small_image_fallback}: deliberately NOT hashed (see fn
            // docs).
            h.opt_u8(p.tree_learn_seeds);
            h.opt_bool(p.lloyd_max_buckets);
        }
    }
    h.0
}

#[cfg(test)]
mod tests {
    #[test]
    fn cell_ids_roundtrip_to_their_variants() {
        // Grammar-totality gate (playbook pattern 7): every id the
        // planner emits — canonical AND alias spellings, both modes,
        // q- and d-grids — parses back to a variant whose fingerprint
        // is IDENTICAL. Renderer and parser move in lockstep.
        use super::*;
        let mut checked = 0usize;
        for (axes, grid) in [
            (SweepAxes::rd_core(), QualityGrid::Step5),
            (
                SweepAxes::modes_full(),
                QualityGrid::ExplicitQuality(vec![10.0, 85.0]),
            ),
            (
                SweepAxes::rd_core(),
                QualityGrid::ExplicitDistance(vec![0.5, 8.5]),
            ),
        ] {
            let plan = SweepBuilder::new(axes, grid).plan();
            for cell in &plan.cells {
                for id in core::iter::once(&cell.id).chain(cell.aliases.iter()) {
                    let v = variant_from_cell_id(id).unwrap_or_else(|e| panic!("{id}: {e}"));
                    assert_eq!(
                        fingerprint(&v),
                        cell.fingerprint,
                        "fingerprint drift for {id}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 50,
            "grammar coverage suspiciously thin: {checked}"
        );
    }

    #[test]
    fn malformed_and_non_self_describing_ids_error() {
        use super::*;
        for bad in [
            "vd-e7_custom#1a2b_def_q85", // content-hashed bundle
            "vd-e7_zen_nolabel_q85",     // unknown registry label
            "vd-e7_zen_def",             // missing quality token
            "mod-e7_def-warp",           // unknown flag
            "px-e7_zen_def_q85",         // unknown mode prefix
        ] {
            assert!(
                variant_from_cell_id(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    use super::*;

    fn tiny_lossy_axes() -> LossyAxes {
        LossyAxes {
            efforts: vec![7],
            strategies: vec![EncoderStrategy::Zenjxl],
            encoder_modes: vec![EncoderMode::Reference],
            internal: vec![NamedLossyParams::default_probe()],
            gaborish: vec![None],
            epf_levels: vec![-1],
            progressive: vec![ProgressiveMode::Single],
            noise: vec![false],
            faster_decoding: vec![0],
            ans: vec![None],
        }
    }

    fn tiny_axes() -> SweepAxes {
        SweepAxes {
            lossy: Some(tiny_lossy_axes()),
            lossless: None,
        }
    }

    #[test]
    fn low_q_calibration_plateau_dedupes() {
        // The generic-quality calibration table maps every q <= 20 to
        // native quality 5.0 → distance 8.5: five grid points, one cell.
        let plan = SweepBuilder::new(
            tiny_axes(),
            QualityGrid::ExplicitQuality(vec![1.0, 5.0, 10.0, 15.0, 20.0]),
        )
        .plan();
        assert_eq!(plan.cells.len(), 1, "cells: {:?}", plan.cells);
        assert_eq!(plan.duplicates_merged, 4);
        assert_eq!(plan.cells[0].aliases.len(), 4);
    }

    #[test]
    fn quality_and_distance_spellings_alias() {
        let d = resolve_distance_for_quality(85.0);
        let via_q = SweepBuilder::new(tiny_axes(), QualityGrid::ExplicitQuality(vec![85.0])).plan();
        let via_d = SweepBuilder::new(tiny_axes(), QualityGrid::ExplicitDistance(vec![d])).plan();
        assert_eq!(via_q.cells[0].fingerprint, via_d.cells[0].fingerprint);
        // The quality spelling records its grid point; the distance
        // spelling has none.
        assert_eq!(via_q.cells[0].quality, Some(85.0));
        assert_eq!(via_d.cells[0].quality, None);
    }

    #[test]
    fn queue_is_main_effects_first() {
        let mut axes = SweepAxes::rd_core();
        axes.lossy.as_mut().unwrap().noise = vec![false, true];
        let plan = SweepBuilder::new(axes, QualityGrid::ExplicitQuality(vec![50.0, 85.0])).plan();

        // The very first cell is the lossy production-default stratum.
        assert_eq!(plan.cells[0].deviations, 0);
        assert!(
            plan.cells[0].id.starts_with("vd-e7_zen_def"),
            "first cell must be the default stratum, got {}",
            plan.cells[0].id
        );
        // Deviations are non-decreasing along the queue.
        assert!(
            plan.cells
                .windows(2)
                .all(|w| w[1].deviations >= w[0].deviations),
            "queue must be priority-ordered"
        );
        // Quality ascends within the leading default stratum.
        assert!(plan.cells[0].quality.unwrap() < plan.cells[1].quality.unwrap());
        // The lossless default stratum is present at deviation 0.
        assert!(
            plan.cells
                .iter()
                .any(|c| c.deviations == 0 && c.id.starts_with("mod-e7_def")),
            "lossless default stratum missing"
        );
    }

    #[test]
    fn lossless_cells_have_no_quality_axis() {
        let axes = SweepAxes {
            lossy: None,
            lossless: Some(LosslessAxes::rd_core()),
        };
        let plan = SweepBuilder::new(axes, QualityGrid::Step5).plan();
        // 3 efforts, no q multiplication.
        assert_eq!(plan.cells.len(), 3);
        assert!(plan.cells.iter().all(|c| c.quality.is_none()));
    }

    #[test]
    fn plan_is_deterministic() {
        let a = SweepBuilder::new(SweepAxes::rd_core(), QualityGrid::Step5).plan();
        let b = SweepBuilder::new(SweepAxes::rd_core(), QualityGrid::Step5).plan();
        assert_eq!(a.cells.len(), b.cells.len());
        for (x, y) in a.cells.iter().zip(&b.cells) {
            assert_eq!(x.id, y.id);
            assert_eq!(x.fingerprint, y.fingerprint);
        }
    }

    #[test]
    fn modes_full_covers_the_scalar_axes() {
        let axes = SweepAxes::modes_full();
        let lossy = axes.lossy.as_ref().unwrap();
        assert!(lossy.epf_levels.contains(&0) && lossy.epf_levels.contains(&3));
        assert!(lossy.faster_decoding.contains(&4));
        assert!(lossy.internal.iter().any(|p| p.label == "emulexp"));
        // Labels are id tokens: '-' is the flag separator and '_' the
        // token separator, so neither may appear inside a label.
        for p in lossy.internal.iter().map(|p| p.label.as_str()).chain(
            axes.lossless
                .as_ref()
                .unwrap()
                .internal
                .iter()
                .map(|p| p.label.as_str()),
        ) {
            assert!(
                !p.contains('-') && !p.contains('_'),
                "label {p} contains an id-separator character"
            );
        }
        // lossy_search_seeds must NOT be a default probe: it is dead
        // without jxl-encoder's `butteraugli-loop` feature, and a
        // structurally-dead knob is a guaranteed inert step.
        assert!(
            lossy
                .internal
                .iter()
                .all(|p| p.params.lossy_search_seeds.is_none()),
            "lossy_search_seeds probe present but dead under this build"
        );
        assert!(
            lossy
                .internal
                .iter()
                .any(|p| p.params.k_info_loss_mul_base == Some(1.3)),
            "kinfo1.3 probe missing"
        );
        let ll = axes.lossless.as_ref().unwrap();
        assert!(
            ll.internal
                .iter()
                .any(|p| p.params.nb_rcts_to_try == Some(1)),
            "rct1 probe (jxl-encoder#67 signal) missing"
        );
        assert!(
            ll.predictors.contains(&Some(6)) && ll.predictors.contains(&Some(0)),
            "live predictor probes (Weighted/Zero) missing"
        );
        // Internal probe labels must be unique (they are id tokens).
        let mut seen = alloc::collections::BTreeSet::new();
        for p in &lossy.internal {
            assert!(seen.insert(p.label.clone()), "dup lossy label {}", p.label);
        }
        let mut seen = alloc::collections::BTreeSet::new();
        for p in &ll.internal {
            assert!(
                seen.insert(p.label.clone()),
                "dup lossless label {}",
                p.label
            );
        }
    }

    #[test]
    fn cell_ids_are_unique_across_modes_full() {
        // Single-quality grid keeps this fast while preserving every
        // stratum spelling (the id-collision surface).
        let plan = SweepBuilder::new(
            SweepAxes::modes_full(),
            QualityGrid::ExplicitQuality(vec![85.0]),
        )
        .plan();
        let mut seen = alloc::collections::BTreeSet::new();
        for cell in &plan.cells {
            assert!(seen.insert(cell.id.clone()), "duplicate id {}", cell.id);
            for a in &cell.aliases {
                assert!(seen.insert(a.clone()), "duplicate alias id {a}");
            }
        }
    }

    #[test]
    fn gather_dedup_phase3_is_excluded_from_fingerprint() {
        // Upstream documents phase3 as byte-neutral (table implementation
        // only); the harness proves it with encode pairs. Equal
        // fingerprints assert the exclusion.
        let mut a = LosslessVariant {
            effort: 7,
            encoder_mode: EncoderMode::Reference,
            internal: NamedLosslessParams::default_probe(),
            predictor: None,
            group_size_shift: None,
            faster_decoding: 0,
        };
        let mut b = a.clone();
        a.internal.params.gather_dedup = Some(true);
        b.internal.params.gather_dedup = Some(true);
        b.internal.params.gather_dedup_phase3 = Some(true);
        assert_eq!(
            fingerprint(&SweepVariant::Lossless(a.clone())),
            fingerprint(&SweepVariant::Lossless(b))
        );
        // Negative control: gather_dedup itself IS hashed (bytes differ
        // from the sort-only path per upstream docs).
        let mut c = a.clone();
        c.internal.params.gather_dedup = None;
        assert_ne!(
            fingerprint(&SweepVariant::Lossless(a)),
            fingerprint(&SweepVariant::Lossless(c))
        );
    }

    #[test]
    fn search_bound_knobs_change_fingerprint() {
        // The zenjpeg TrellisSpeedMode lesson: search-effort knobs are
        // output-affecting until proven otherwise — they must be hashed.
        let base = LosslessVariant {
            effort: 10,
            encoder_mode: EncoderMode::Reference,
            internal: NamedLosslessParams::default_probe(),
            predictor: None,
            group_size_shift: None,
            faster_decoding: 0,
        };
        let mut seeded = base.clone();
        seeded.internal.params.tree_learn_seeds = Some(1);
        assert_ne!(
            fingerprint(&SweepVariant::Lossless(base)),
            fingerprint(&SweepVariant::Lossless(seeded))
        );
    }

    #[test]
    fn invalid_internal_params_are_reported_not_lost() {
        let mut axes = tiny_axes();
        let mut bad = LossyInternalParams::default();
        bad.k_ac_quant = Some(-1.0); // rejected by upstream validate()
        axes.lossy.as_mut().unwrap().internal = vec![NamedLossyParams::new("bad", bad)];
        let plan = SweepBuilder::new(axes, QualityGrid::ExplicitQuality(vec![75.0])).plan();
        assert!(plan.cells.is_empty());
        assert_eq!(plan.invalid_skipped.len(), 1);
        assert!(plan.invalid_skipped[0].contains("bad"));
    }

    #[test]
    fn non_positive_distance_is_reported_not_lost() {
        let plan =
            SweepBuilder::new(tiny_axes(), QualityGrid::ExplicitDistance(vec![0.0, 1.0])).plan();
        assert_eq!(plan.cells.len(), 1);
        assert_eq!(plan.invalid_skipped.len(), 1);
        assert!(plan.invalid_skipped[0].ends_with("_d0"));
    }

    #[test]
    fn budget_ladder_collapses_lossy_mode_axes_first_and_reports() {
        let mut axes = SweepAxes::rd_core();
        {
            let lossy = axes.lossy.as_mut().unwrap();
            lossy.ans = vec![None, Some(false)];
            lossy.noise = vec![false, true];
            lossy.epf_levels = vec![-1, 0];
        }
        let unbudgeted = SweepBuilder::new(axes.clone(), QualityGrid::Step5).plan();
        let budget = unbudgeted.cells.len() / 4;
        let plan = SweepBuilder::new(axes, QualityGrid::Step5)
            .with_budget(budget)
            .plan();
        assert!(plan.cells.len() <= budget);
        assert!(!plan.dropped.is_empty());
        assert_eq!(plan.dropped[0].axis, "lossy.ans");
        assert!(!plan.over_budget);
        for d in &plan.dropped {
            assert!(!d.dropped.is_empty(), "drop report must list values");
        }
    }

    #[test]
    fn q_coarsening_keeps_endpoints_and_floor() {
        let pts = QualityGrid::Step5.points();
        let coarse = coarsen_keep_endpoints(&pts);
        assert_eq!(coarse.first(), pts.first());
        assert_eq!(coarse.last(), pts.last());
        assert!(coarse.len() >= 11);
    }

    #[test]
    fn over_budget_reports_rather_than_samples() {
        // Impossible budget: 1 cell. Ladder exhausts, flag set, plan
        // complete.
        let plan = SweepBuilder::new(SweepAxes::rd_core(), QualityGrid::Step5)
            .with_budget(1)
            .plan();
        assert!(plan.over_budget);
        assert!(plan.cells.len() > 1, "nothing may be silently sampled away");
    }

    #[test]
    fn encodes_math() {
        let plan =
            SweepBuilder::new(tiny_axes(), QualityGrid::ExplicitQuality(vec![50.0, 80.0])).plan();
        assert_eq!(plan.encodes(50, 4), plan.cells.len() * 200);
    }
}
