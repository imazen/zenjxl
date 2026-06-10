# Variant Generation: zenjxl's adoption of the zenjpeg patterns

Written 2026-06-10. The codec-neutral patterns are stated in
`zenjpeg/docs/VARIANT_GENERATION.md` (the reference implementation);
this document records how zenjxl adopts them, what is different because
zenjxl is a *wrapper* around jxl-encoder rather than the encoder
itself, and the dominance/trial/metric audit that document queued for
JXL.

Code: `src/sweep.rs` (axes, grid, planner, fingerprint — gated
`encode` + `__expert`), `JxlEncoderConfig::resolve_plan()` in
`src/codec.rs` (gated `zencodec` + `encode` + `__expert`),
`examples/sweep_validate.rs` (the empirical harness). Everything is
behind `__expert` deliberately: the sweep surface drives jxl-encoder's
internal-params escape hatch and inherits its no-semver-guarantees
contract. Promotion of `resolve_plan` to the stable surface is a
follow-up that needs an explicit API review.

## Pattern-by-pattern

### 1. Knobs live on the variant that uses them

JXL's discrimination axis is **mode**: lossy (VarDCT) and lossless
(modular) share almost no knob space. `SweepVariant` is an enum of
`LossyVariant` (distance, strategy, EPF, gaborish, noise, progressive,
faster-decoding, ANS pin, `LossyInternalParams`) and `LosslessVariant`
(predictor, group size, palette, LZ77, `LosslessInternalParams`) — a
lossless cell cannot spell a butteraugli distance, structurally. The
quality grid applies to lossy cells only; lossless strata emit one cell
each (the grid-level form of the same discrimination).

Where reality itself was confused, it got fixed rather than modeled:
`JxlEncoderConfig::with_noise(true)` under lossless mode was a silent
no-op; `validate()` now rejects it
(`ValidationError::NoiseInLosslessMode`). The quality knobs under
lossless are tolerated (generic zencodec pipelines set quality before
toggling lossless) but reported as `inert_knobs` by `resolve_plan()`.

Build-feature liveness is part of discrimination too:
`lossy_search_seeds` only acts inside the butteraugli quality loop,
which the default `__expert` build does not compile in — so it is NOT a
default probe (a structurally-dead knob is a guaranteed inert step).
Same logic applies to the `ssim2-loop`/`zensim-loop` knob families.

### 2. Dominance / trial / metric — the JXL audit

This fills in the "audit needed" cells from the zenjpeg doc's
per-codec table.

**Dominance** (one side always wins; take it, zero trials):

- Container framing: `ContainerMode::Auto` emits the bare codestream
  unless boxes (gain map, Exif/XMP, JUMBF) force the container —
  pure header pruning, already implemented upstream as the default.
- That is the only dominance case found. JXL has no analog of
  zenjpeg's optimized-vs-fixed Huffman dominance: its entropy choices
  are content-dependent (see trial class).

**Trial** (decoded pixels identical across candidates; `min(bytes)` is
exact):

- **The entire lossless knob space is trial-class by definition** —
  RCT choice, predictor, MA-tree shape, LZ77, palette, group size all
  reconstruct the same pixels. The encoder's internal searches
  (`nb_rcts_to_try`, predictor search, tree learning) are
  heuristic-cost trials over that space; the effort dial is the trial
  budget. An *exact* wrapper-level trial (encode N lossless configs,
  ship the smallest) is possible but costs N full encodes — unlike
  zenjpeg's entropy-stage trials there is no cheap shared state at the
  wrapper layer. If exact lossless trials land, they belong inside
  jxl-encoder where the gathered samples / token streams could be
  shared across candidates, byte-gated per the zenjpeg discipline.
- **Lossy entropy-stage knobs** are trial-class per pipeline ordering
  (AC-strategy decisions use the cost model, not the final
  histograms): `use_ans` (ANS vs prefix coding),
  `ans_histogram_strategy_vardct` (Fast/Approximate/Precise
  normalization), `enhanced_clustering_vardct` (k-means vs pair-merge
  context clustering). All three change bytes, none change pixels.
  Today they are effort-scheduled, not trialed; they are the natural
  candidates for an upstream `Smallest`-style exact entropy trial.
  Verify the pixel-invariance claim by encode before building a trial
  on it — that is a one-evening jxl-encoder experiment, not a wrapper
  feature.
- **Pass-structure knobs** (`progressive`, `group_order`,
  `center_first`, `progressive_dc`) reorder/stage the same quantized
  coefficients — pixel-invariant. Expect `Single` to dominate on
  bytes (progressive exists for UX, not size); the harness's per-label
  size table measures the actual cost (see `benchmarks/`).

**Metric** (pixels change; sweeps and pickers exist for these):
distance/quality, effort (gates pixel-affecting tools), strategy
bundle, encoder mode, EPF, gaborish, noise synthesis,
faster-decoding tiers, resampling, the DCT-class toggles,
`k_info_loss_mul_base`, `k_ac_quant`, `entropy_mul_table` (it steers
AC-strategy *choices*, hence pixels — do not confuse it with the
entropy-stage trial knobs above), CfL, chromacity adjustment, patches,
dots, splines.

### 3. Resolution is a function, introspection calls the same function

zenjxl's resolution layer is thin and the plan exploits that:
`JxlEncoderConfig`'s setters rebuild the stored
`LossyConfig`/`LosslessConfig` on every change, so
`resolve_plan()` simply **reads the same object the encode call
consumes** (`distance()`, `effort()`, `noise()` getters). There is no
second implementation to drift. The quality→distance chain
(`quality_to_distance(calibrated_jxl_quality(q))`) is re-exported as
`sweep::resolve_distance_for_quality` and `tests/resolve_plan.rs` pins
that plan, sweep module, and encoder agree on it.

The deliberate boundary: effort→tool resolution (which DCT classes,
tree parameters, iteration counts an effort level enables) lives in
jxl-encoder's `EffortProfile` and is NOT duplicated in zenjxl. The
plan reports the resolved (mode, distance, effort, noise, container)
and the per-mode dead knobs; per-tool introspection would require
upstream to expose its resolved profile (a reasonable future `__expert`
addition — see Known limits).

### 4. Byte-identity fingerprints over RESOLVED state

`sweep::fingerprint` (FNV-1a) hashes the resolved variant state. The
marquee JXL alias is in the quality mapping itself: the
CID22-calibrated generic-quality table **plateaus at q ≤ 20** (all map
to native quality 5.0 → distance 8.5), so five of the 21 step-5 grid
points are one encode — a 19% grid reduction before any encode runs,
recorded as aliases exactly like zenjpeg's Glassa anchor clamp.
Quality-vs-distance spellings of the same resolved distance merge the
same way.

Exclusions (each must be encode-provable, per the zenjpeg
`TrellisSpeedMode` lesson):

- raw generic quality — mediated by resolved distance (same code
  chain; harness proves the plateau pair byte-identical);
- `gather_dedup_phase3` — upstream-documented byte-neutral (dedup
  *table implementation*, not the byte-determining sort path); proven
  by encode pairs with `gather_dedup` both off and on;
- `tree_parallel_{max_depth, floor, root_threshold,
  small_image_fallback}` and `smart_fanout` — scheduling-only by
  upstream design. The harness proves the sequential build; the
  parallel-build bitstream-equivalence claim is upstream's (backed by
  its hash-lock suite). These are not sweep axes.

Everything else output-plausible IS hashed, including every
search-bound knob (`tree_learn_seeds`, `ans_histogram_strategy_vardct`,
`gather_dedup`, `use_streaming_dedup`, `lloyd_max_buckets`).

Known under-merge: an override equal to its effort-derived default
(`nb_rcts_to_try: Some(7)` at e7) does not merge with `None`, because
jxl-encoder does not expose the per-effort defaults. Under-merging
costs duplicate encodes, never correctness; the safe direction.

### 5. Budgeted, ordered, no-silent-caps sweep plans

Direct port: `SweepAxes` (per-mode, most-important-first) ×
`QualityGrid` (`Step5` floor / `TrainingDense` / explicit quality or
distance points) → deduplicated cells with validity filtering reported
(`invalid_skipped`, via jxl-encoder's own `validate()`), a budget
ladder that sheds one lowest-tier value at a time (lossy axes first —
they multiply by the grid — then lossless, then uniform q-coarsening
with endpoints kept and an 11-point floor; efforts never drop below
the 3-value rd_core, strategies never below Zenjxl+Libjxl), and
`over_budget` instead of silent sampling. Queue ordering is
main-effects-first with lossy-before-lossless inside each deviation
class and quality ascending within a stratum.

Scalar step provenance is in the `src/sweep.rs` module-docs table; the
rule here is that curated steps come from values that already ship in
the encoder (effort-ladder defaults, named preset constructors,
documented ranges) — not invented grids. `nb_rcts_to_try`'s probe is
`Some(1)` (identity-RCT-only), not `Some(0)`, per jxl-encoder#67:
the 0-fallback is GBR_SUBGR, which the default search often picks
anyway, so 0-vs-default can be byte-identical by content coincidence.

### 6. Empirical validation

`examples/sweep_validate.rs` encodes the default + every
single-deviation stratum of `modes_full` on 3 CID22-512 photos +
noise/complex/checker synthetics + one 64×64 tiny, and hard-fails on
inert steps, fingerprint-contract violations (alias pairs AND negative
controls), **lossless roundtrip mismatches** (decoded pixels must equal
input exactly — the zero-tolerance rule applied as a harness gate),
ordering breakage, and ssim2 sanity-floor violations at q85. Soft
checks: bytes monotone in quality, e9 ≤ e5 mean bytes, faster-decoding
and LZ77-off cost bytes. `lean` (LeanFaster vs Zenjxl) is the one
soft-exempted label: that bundle only diverges on content that trips
its per-image gates, which this corpus may not contain.

Results land in `benchmarks/sweep_validate_jxl_<date>.tsv` with the
git commit in the header. Re-run the harness whenever the axes, the
fingerprint, or jxl-encoder's internal-params surface changes — the
`*InternalParams` structs are `#[non_exhaustive]`, so new upstream
knobs do NOT automatically enter the fingerprint; the harness re-run
is what keeps the exclusion set honest.

### What the first run caught (2026-06-10)

The maiden run found one critical encoder bug, one API-liveness bug
class, and five mis-curated probes — none visible to unit tests that
only check plan structure:

1. **jxl-encoder#68 — e9+ lossless emits undecodable bitstreams** on
   photographic content (`SectionTooShort` from zenjxl-decoder;
   jxl-oxide and libjxl djxl reject the same streams, so the bitstream
   itself is malformed). Caught by the lossless-roundtrip-exactness
   gate. Bisect from the public API ruled out every individually
   overridable e8→e9 knob; the remaining suspects (`lz77_method`
   Greedy→Optimal, `tree_sample_fraction` 0.55→0.65) are exactly the
   ones whose overrides don't propagate (#69). The e9 axis value
   deliberately stays; the harness stays red on those cells until the
   fix lands.
2. **jxl-encoder#69 — silently unconsumed setters**: lossless
   `with_lz77` / `with_lz77_method` / `with_patches` /
   `with_modular_palette_colors` and the
   `tree_sample_fraction` override change nothing in either direction
   (including palette's best-case 2-color checkerboard). The lz77 and
   palette axes were removed from `LosslessAxes` until plumbed.
3. **Default-alias probes** (the #67 trap, twice more): predictor
   `Some(5)`/`Some(15)` byte-alias the e7 default selection (replaced
   with 6 = Weighted and 0 = Zero, both proven live); `rct19` never
   beat the 7-candidate default search on the corpus (dropped; `rct1`
   keeps override-liveness coverage).
4. **Gate-shadowed probes**: `try_dct32 = false` discriminated nothing
   (0/42) — 32-class merges never win on this corpus under Zenjxl
   defaults (replaced with `chromacity_adjustment = false`, live
   everywhere); lossy `faster_decoding = 2` is patches-off only, and
   patches never fire on photo content (lossy axis now {0, 4};
   lossless keeps {0, 2}, where tier 2 forces small groups and
   byte-aliases `group_size_shift = 0`).
5. **Mode-invariant knob**: lossless `EncoderMode::Experimental` is
   currently profile-invariant — removed from the curated lossless
   axes until an experimental lossless divergence ships.

Also confirmed by the same run: the calibration-plateau and
quality-vs-distance alias pairs are byte-identical on real encodes,
`gather_dedup_phase3` and the `tree_parallel_*` knobs are byte-neutral
(sequential build), `smart_fanout` is byte-neutral at e8, and lossless
e7/e8 round-trip exactly on all seven corpus images.

## Known limits / open items

- **jxl-encoder#68**: e9+ lossless output is undecodable on photo
  content — sweeps over lossless e9/e10 cells produce garbage training
  rows until fixed (sizes are real, decodes fail loudly).
- **jxl-encoder#69**: lz77 / lz77_method / patches / palette /
  tree_sample_fraction are config surface without effect; they rejoin
  the axes when plumbed.
- **No exact trials shipped.** The audit above identifies the
  candidates (entropy-stage knobs, lossless candidate sets); the
  adoption order in the zenjpeg doc puts trials last for exactly the
  reason observed here — the fingerprint work is what surfaced which
  knobs are byte-only. The cheap-shared-state versions belong
  upstream.
- **Fingerprint under-merge** on override-equals-default spellings;
  fixable if jxl-encoder exposes per-effort resolved defaults under
  `__expert`.
- **resolve_plan is `__expert`-gated.** Promoting it (and a
  `LosslessConfig`-side introspection of the effort profile) to stable
  needs an API review; nothing blocks it technically.
- **chroma_subsampling** is rejected by the encoder for non-444 today
  (jxl-encoder issue #47 chunk 4); it becomes a lossy axis the day it
  lands.
- **LeanFaster soft-exemption** should be retired by adding a
  confirmed gate-tripping image (screenshot-class) to the harness
  corpus.
- **Alpha axes** (`alpha_distance`, `alpha_squeeze`,
  `simplify_invisible`) need an RGBA corpus and an alpha-aware metric
  before they can be swept honestly.
- The harness pins threads=1 and runs the non-`parallel` build; a
  `parallel`-build determinism pass (same fingerprint, threads 1 vs N,
  byte-identical) is a cheap future hardening step.
