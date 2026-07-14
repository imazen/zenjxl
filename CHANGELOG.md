# Changelog

## [Unreleased]

### QUEUED BREAKING CHANGES
- zencodec trait impls' associated `Error` type changed from `At<JxlError>`
  to `At<zencodec::CodecError>` (the envelope, Pattern B) — breaking for code
  that names the trait impls' associated `Error` type. See *Changed* below
  for the full description (merged as `9b948f4`, PR #16). The package
  version was already bumped 0.2.1 → 0.3.0 in-tree ahead of release.
- `JxlError::LimitExceeded` gained a second field: was `LimitExceeded(String)`,
  now `LimitExceeded(String, zencodec::LimitKind)` — breaking for code that
  pattern-matches the old 1-tuple shape. Carries the real cap that was hit
  (`Width`/`Height`/`Pixels`/`Memory`/`InputSize`/`OutputSize`/`Frames`)
  instead of a single hardcoded `LimitKind::Memory` value. See *Fixed* below.
- Two additive `JxlError` variants (`MalformedImage`, `InvalidState`) were
  added alongside the taxonomy reshape — additive on the already
  `#[non_exhaustive]` enum, but batched here since they land in the same
  0.3.0 release as the other breaks. See *Fixed* below.

**Release gate cleared:** zenjxl 0.3.0's `CategorizedError` taxonomy adoption
(zencodec#103, reshaped into a two-level origin-first taxonomy —
`Image`/`Request`/`Resource`/`Policy`/`Stopped`/`Io`/`Internal` — by #116, and
including the `Lifecycle` -> `Stopped` rename) now resolves directly against
the published zencodec 0.1.26. The temporary `[patch.crates-io]` git pin
(`caterr-reshape` branch) has been dropped and the declared `zencodec`
requirement bumped to `"0.1.26"`. `zencodec-testkit` (the dev-only
conformance-harness dependency providing `check_decode_truncation_series`)
remains git-pinned to the same rev, since that companion crate is still
unpublished. `ErrorCategory` was never published prior to 0.1.26, so
adopting the reshape was not a break of released API.

### Added
- Wired the zencodec-testkit `check_decode_truncation_series` EOF/truncation
  conformance check into the decode test suite (`tests/truncation_series.rs`),
  gated `#![cfg(all(feature = "zencodec", feature = "encode", feature = "decode"))]`
  and run by CI's `--all-features` job. `zencodec-testkit` is git-pinned (still
  unpublished) to the same rev the `zencodec` patch used to carry.

### Changed
- Adopted zencodec's two-level origin-first `ErrorCategory` taxonomy
  end-to-end: `CategorizedError::category()` now introspects the *foreign*
  decode/encode errors' own sub-variants instead of blanket-folding them —
  `JxlError::Decode` matches on `jxl::api::Error::kind()`'s `ErrorClass`
  (`InvalidBitstream`/`Unsupported`/`LimitExceeded`/`OutOfMemory`/
  `Cancelled`/`Io`/`OutputConfiguration`/`Internal`) instead of always
  reporting `Malformed`, and `JxlError::Encode` matches on
  `jxl_encoder::EncodeError`'s own variants instead of always reporting an
  internal bug. A caller-request-origin `EncodeError` (bad config, an
  unsupported pixel layout) previously read as a producer-side `Internal`
  fault; it now reports `Request(Invalid(Parameters))` /
  `Request(Unsupported(PixelFormat))` as appropriate.

### Fixed
- Truncated/incomplete JXL input was mis-categorized as
  `ErrorCategory::Request(RequestError::Invalid(InvalidKind::Parameters))`
  (a caller-fault 4xx) instead of
  `ErrorCategory::Image(ImageError::UnexpectedEof)`. The decoder's
  `ProcessingResult::NeedsMoreInput` (definitionally "needs more bytes") was
  mapped to `JxlError::InvalidInput` at six one-shot decode sites — four in
  `decode.rs` (header/frame/pixels) plus two in `codec.rs`'s streaming
  animation decoder that the first pass missed (same audit, same bug, found
  while wiring the taxonomy reshape). Added a dedicated `JxlError::UnexpectedEof`
  variant (additive — the enum is `#[non_exhaustive]`), routed all six
  `NeedsMoreInput` arms to it, and mapped it to
  `ErrorCategory::Image(ImageError::UnexpectedEof)`. Surfaced by the
  truncation-series check.
- `JxlError::OutOfMemory` split out from `LimitExceeded` (bug #21, `e9a14bb`):
  allocation failures and size-computation overflows (`alloc_util`'s
  `try_reserve_exact` OOM path, `decode::checked_buf_size`'s `checked_mul`
  overflow, `codec.rs`'s RGB-capacity-overflow check) now report
  `ErrorCategory::Resource(ResourceError::OutOfMemory)` instead of being
  folded into `Resource(ResourceError::Limits(LimitKind::Memory))`, matching
  the zenjpeg `AllocationFailed`/`SizeOverflow` precedent. Genuine
  caller-configured caps (`zencodec::ResourceLimits` checks,
  `JxlLimits::validate`) are unchanged and still report `Resource(Limits(_))`.
- `JxlError::LimitExceeded`'s category hardcoded `LimitKind::Memory` for
  every one of the ~15 `zencodec::ResourceLimits` call sites, regardless of
  which cap (dimensions/frames/input size/output size/memory) actually
  fired. `LimitExceeded` now carries the real `LimitKind`, read from the
  triggering `zencodec::LimitExceeded::kind()` at each construction site (or
  set directly for zenjxl's own `JxlLimits::validate` pixel/memory checks).
- `finish()` called before any rows or frames were pushed (both the
  one-shot streaming encoder and the animation encoder) categorized as
  `Request(Invalid(Parameters))` (`JxlError::InvalidInput`) — a caller-fault
  parameter issue — when it is a call-sequencing violation (the operation
  was invoked out of sequence, not given a bad value). Added
  `JxlError::InvalidState` (additive) and rerouted both sites; now
  categorizes as `Request(Invalid(InvalidKind::State))`.
- A header-reported JXL dimension overflowing `u32` (`decode::dim_to_u32`)
  and a `ReconstructHdr` gain-map bundle's unparsable embedded `jhgm`
  ISO 21496-1 metadata both categorized as `Request(Invalid(Parameters))`
  (`JxlError::InvalidInput`) even though the values came from decoding the
  bitstream/container, not from a caller parameter. Added
  `JxlError::MalformedImage` (additive) and rerouted both sites; now
  categorizes as `Image(ImageError::Malformed)`. Two adjacent
  `ReconstructHdr` sites (an unsupported gain-map codestream pixel format,
  and `ultrahdr-core`'s `apply_gainmap` failure) were reviewed but left as
  `InvalidInput` — reclassifying them needs a third new variant plus
  `ultrahdr-core` error-semantics research, out of scope for this pass.

### Documentation
- README overhaul: full badge row (CI/crates.io/lib.rs/docs.rs/MSRV/license),
  documented the `jpeg-lossy`, `reconstruct-hdr`, and `__expert` features,
  refreshed the crosslink footer, and split a badge-free `README.crates.md` for
  crates.io (`readme = "README.crates.md"`).

### Changed
- **zencodec trait impls return `At<CodecError>` — the envelope (Pattern B)**
  (PR #16, error-taxonomy, merged as `9b948f4`). All eight `zencodec` trait impls behind the
  `zencodec` feature (`EncoderConfig` / `EncodeJob` / `Encoder` /
  `AnimationFrameEncoder` / `DecoderConfig` / `DecodeJob` / `Decode` /
  `AnimationFrameDecoder`) now use `type Error = At<zencodec::CodecError>`
  instead of `At<JxlError>`, so a generic consumer recovers the `ErrorCategory`
  *and* the codec name (`"zenjxl"`) **through `Dyn*` dispatch + `Box<dyn Error>`
  erasure** — `None` under the old typed error, `Some` now (forcing test
  `codec::tests::dyn_dispatch_preserves_category_and_codec_through_erasure`).
  `JxlError` (+ its `CategorizedError` impl) is kept as the recoverable detail
  and category source; the new `From<JxlError> for At<CodecError>` bridge wraps
  bare native values, and already-located `At<JxlError>` internals convert once
  at each trait boundary via `CodecError::of` (internal `?` sites untouched).
  The native `decode` / `probe` / `encode*` API is **unchanged** — it keeps the
  typed `At<JxlError>`. Breaking for code that named the trait impls' associated
  `Error` type.
- deps: migrate to published zencodec 0.1.24 estimate API; drop the temporary
  `[patch.crates-io] zencodec = { git, rev = "0f71295" }` pin (the `estimate`
  API is now on crates.io). The `estimate_encode_resources` mapping in
  `src/codec.rs` follows the refined `ResourceEstimate` API:
  `ResourceEstimate::new(peak, time_ms as u64)` (wall_ms is now `u64`),
  `.with_peak_max(max)` (replaces the dropped `.with_peak_range(min, max)`),
  the `.with_output_bytes(..)` call is gone, and
  `ThreadingInformation::parallel(max_efficient_threads)` is now 1-arg.

### Added
- **`zencodec::CategorizedError` adopted on `JxlError`** (error-taxonomy, PR
  zencodec#103). `codec_name()` returns `Some("zenjxl")`; `category()` totally
  maps every variant to one `ErrorCategory`: `Decode` → `MalformedImage`,
  `ProgressiveRejected` → `PolicyRejected`, `Encode` → `Internal`,
  `InvalidInput` → `InvalidParameters`, `LimitExceeded` →
  `LimitsExceeded(LimitKind::Memory)`, `OutOfMemory` → `OutOfMemory` (split
  from `LimitExceeded`, see *Fixed* above), `Sink` → `Io(opaque)`, with
  `Cancelled` and `UnsupportedOperation` delegating to the zencodec cause
  type's own `category()` (cancelled-vs-timed-out,
  operation-vs-pixel-format). New `error::categorized_error_tests` covers the
  per-variant mapping. **TEMP**:
  builds against `[patch.crates-io] zencodec = { git, branch =
  "cancellation-classification-99" }` until zencodec 0.1.26 ships #103; drop the
  patch and bump the `zencodec` dep at that point.
- **`AllocPreference` honored at untrusted decode allocations** (3-mode,
  per-site). The wrapper-owned output buffers — the single-image output buffer
  (`src/decode.rs`), the per-animation-frame buffer, and the recursive
  gain-map sub-image buffers (`src/codec.rs`) — are all sized from the
  untrusted header dimensions, so they default to the *fallible* `try_reserve`
  path (graceful `JxlError::OutOfMemory` on a forged header, see *Fixed*
  above) and honor
  `zencodec::AllocPreference` (`Fallible`/`Infallible` override; `CodecDefault`
  keeps the site default). Threaded from
  `ResourceLimits::prefer_fallible_allocations` at the zencodec decode boundary;
  the direct `decode*` API is unchanged (passes `CodecDefault`). New
  `src/alloc_util.rs` (the `resolve_fallible`/`alloc_zeroed` 3-mode helpers).
  The heavy VarDCT/modular pass buffers live in the `zenjxl-decoder`
  dependency and are out of this preference's reach (deferred follow-up).
- **`JxlDecoderConfig::estimate_decode_resources`** — overrides the
  `zencodec::DecoderConfig` default with a JXL-shaped heuristic (output buffer
  + VarDCT/modular working set + fixed entropy/context overhead), reported
  SERIAL and core-adjusted via `ResourceEstimate::at_cores`. Mirrors the
  existing `estimate_encode_resources`.
- vCPU-aware resource estimation via zencodec's unified `estimate` API:
  `JxlEncoderConfig::estimate_encode_resources(&ImageCharacteristics, &ComputeEnvironment)`
  (overrides the `zencodec::EncoderConfig` default, behind the `zencodec`
  feature) returns a core-adjusted `ResourceEstimate`. Maps `jxl-encoder`'s
  native `heuristics::estimate_encode` + `encode_threading_info` onto the
  shared `zencodec::estimate::{ResourceEstimate, ThreadingInformation}` at the
  boundary — `jxl-encoder` core stays standalone (keeps its own threading
  model; no zencodec coupling).
- **Sweep: `scalar_dense` preset + compute-budget constraint** (`src/sweep.rs`,
  `encode`+`__expert`; VARIANT_GENERATION patterns 17–18). `SweepAxes::scalar_dense()`
  emits dense isolated single-axis ladders for trained scalar heads: the **full
  effort ladder e1–e10** (filling the `e4`/`e6`/`e8` that `modes_full` skips), an
  8-point `k_ac_quant` ladder, and `fine_grained_step`. New
  `SweepBuilder::with_max_deviations(1)` keeps main-effects only.
  `compute_tier(&SweepVariant)` returns the effort level and
  `SweepBuilder::with_compute_limit(max)` caps a sweep by effort
  (`with_compute_limit(5)` = "sweep e ≤ 5"), with dropped cells reported in
  `SweepPlan::compute_tier_skipped`. All additive, behind `__expert`.

### Fixed
- **Preserve the decoder/encoder `At` trace across the codec boundary** and fix
  the `E0308` compile error from the `zenjxl-decoder` `At<Error>` bump (decoder
  `b1be322`: `process()` now returns `Result<_, At<jxl::api::Error>>`). The 7
  decode sites and 4 encode sites flattened the inner error (`JxlError::Decode(e)`
  with `e: At<Error>`, or `JxlError::Encode(e.decompose().0)` which dropped the
  trace). They now use `whereat`'s trace-preserving conversions: the direct
  decode site and all encode sites use `.map_err_at(JxlError::from)` /
  `.map_err_at(JxlError::Encode)`; the 6 sites routed through the
  `ProgressiveRejected`-special-casing `map_err` helper use
  `.map_err(|e| e.map_error(map_err))` (runs the helper on the inner error while
  keeping the `At` trace frames). The callee's location frames now survive into
  `At<JxlError>` instead of being discarded.

### Documentation
- README: fixed the decode/error snippets so they compile against the real API
  (closes #9) — replaced the non-existent `err.location()` with `err.full_trace()`
  and the non-existent `PixelBuffer::contiguous_bytes()` with
  `as_contiguous_bytes()`, and documented that `max_memory_bytes` also caps the
  decoder's live allocations (not just the output-size estimate). Added
  `tests/readme_examples.rs`, a CI-exercised compile-check of every README
  snippet so they can't silently rot again.
- README: documented how to read pixels back out of the decoded `PixelBuffer`
  (the `zenpixels-convert` `to_rgba8().copy_to_contiguous_bytes()` chain for
  packed RGBA8, plus `into_vec()`/`contiguous_bytes()`/`descriptor()` for the
  native layout); added an **Install** section (`zenjxl = "0.2.1"` — the crate
  is published, the old "not published to crates.io" note was wrong); fixed the
  **Encode** example, which never compiled (`encode_rgb8(rgb, w, h, distance)`
  is not a real signature — the convenience fns take an `imgref::ImgRef`) by
  switching it to the raw-bytes `LossyConfig`/`LosslessConfig` `encode(pixels,
  w, h, PixelLayout)` path; and stated the JXL **distance** convention
  (LOWER = better) with the `calibrated_jxl_quality` → `quality_to_distance`
  mapping. Found via an insulated external-developer usability test of the
  published 0.2.1 README.

### Added

- **SCALAR sweep-axis ladders** for the dense-sweep program (zenmetrics
  `docs/PLAN_SWEEPS.md` §5 gaps; `zenpicker-train --scalar-axes` heads), as
  eight new entries in the `__expert` internal-probe registry (d83b1afc):
  `k_ac_quant` SCALAR ladder `{0.575, 0.65, 0.88, 1.0}` around the 0.765
  libjxl default (0.65 = the jxl-encoder#25 measured value; the axis is the
  sanctioned follow-on C learned-dispatch route); `fine_grained_step` SCALAR
  probes `{1, 3}` (4/8 proven structurally dead — the non-aligned 32×32
  pass's `(cy|cx) % 4 == 0` skip makes multiple-of-4 steps a no-op, pinned
  by test); `entropy_mul_table` presets `screenshot_suppressed()` +
  `high_d_photo_smooth_suppressed()` alongside `experimental()`. All three
  knobs were already fingerprint-hashed; ids/parser/budget-ladder pick the
  values up via the registry. Harness re-run fully green, all probes live
  (`benchmarks/sweep_validate_jxl_2026-06-12.tsv`).

- **`DecodePolicy::allow_progressive` now gates JXL during decode** (zencodec
  adapter). The decode path wires the caller's policy into the decoder's
  `JxlDecoderOptions::reject_progressive`: with `allow_progressive ==
  Some(false)` a progressive codestream (multi-pass or LF frame) is rejected at
  the first frame header with the new `JxlError::ProgressiveRejected`; `None`
  and `Some(true)` decode as before. Both the single-image and animation decode
  paths honor it; `probe`/`JxlBasicInfo` are header-only and intentionally
  untouched. Depends on the unreleased `zenjxl-decoder` `reject_progressive`
  option (path-patched until 0.3.11). Tests in `tests/progressive_policy.rs`.

- Pattern 7 (cell ids as durable identity): `sweep::variant_from_cell_id`
  reconstructs the exact `SweepVariant` from a cell id alone — both
  grammars (vd-/mod-), `_q` tokens resolved through the same calibrated
  distance chain the planner used, internal-params labels via the new
  registry lookups (`lossy_params_by_label` / `lossless_params_by_label`;
  `"def"` + every curated probe). Content-hashed `custom#…` bundles and
  unknown labels error (not self-describing). Grammar-totality test:
  every planner-emitted id (canonical + alias, q- and d-grids) parses
  back fingerprint-identical. Unblocks checklist step 8 (zenmetrics
  executor wiring).


- **Native `ReconstructHdr` behind the new `reconstruct-hdr` feature**
  (zencodec adapter): `GainMapRender::ReconstructHdr` now applies jhgm gain
  maps natively instead of downgrading to `Components`. ISO 21496-1
  headrooms decide the direction — an HDR-base bundle (JXL-typical) returns
  the base, which already carries its own HDR signaling; an SDR-base bundle
  gets the gain map applied via `ultrahdr_core::gainmap::apply_gainmap` into
  linear f32 (or f16 when preferred) RGBA, with `target_headroom: None`
  reconstructing at the gain map's encoded maximum and the output
  `ImageInfo` carrying a derived content-light-level + mastering-display
  envelope. `DecodeCapabilities::reconstructs_hdr()` flips to `true` under
  the feature; without it the downgrade-to-Components behavior is unchanged.
  Malformed jhgm metadata or an unsupported gain-map form errors — never a
  silent SDR fallback. New optional dep: `ultrahdr-core 0.5.0`
  (registry; jhgm parameter parsing stays in zencodec). Tests
  `reconstruct_*` in `tests/gain_map_render.rs` cover apply, headroom clamp,
  HDR-base passthrough, and the feature-off downgrade.
- **Variant generation (`__expert`): sweep planner + fingerprints + plan
  introspection**, porting zenjpeg's `VARIANT_GENERATION.md` patterns
  (see `docs/VARIANT_GENERATION.md` for the jxl adoption + the
  dominance/trial/metric audit). `zenjxl::sweep`: mode-discriminated
  `LossyVariant`/`LosslessVariant` (knobs live on the mode that uses
  them), `SweepAxes` × `QualityGrid` → deduplicated, main-effects-first
  `SweepPlan` with a budget ladder and no-silent-caps drop reporting,
  and an FNV-1a byte-identity `fingerprint` over resolved state (the
  generic-quality calibration plateau q ≤ 20 → distance 8.5 dedupes five
  step-5 grid points per stratum). `JxlEncoderConfig::resolve_plan()` →
  `JxlEncodePlan::{Lossy,Lossless}` reads the same stored upstream
  config the encode consumes (no second resolution implementation);
  lossless plans report dead knobs. Empirical harness
  `examples/sweep_validate.rs` (inert-step / fingerprint-contract /
  lossless-roundtrip-exactness / ordering hard gates), results in
  `benchmarks/sweep_validate_jxl_2026-06-10.tsv`. New `__expert`
  re-exports: `EncoderStrategy`, `ProgressiveMode`, `RctType`,
  `ANSHistogramStrategy`. The maiden harness run found jxl-encoder#68
  (e9+ lossless emitted undecodable bitstreams — TWO independent
  causes, both root-caused via the harness's bisect trail and fixed
  upstream same-day: mid-group ref-property stride truncation in
  `5eefe5f7`, and spec-divergent group_id stream numbering in
  `329f207d`, the latter exposed by the harness re-run after the first
  fix; final harness run fully green against the stock published
  decoder) and jxl-encoder#69 (lossless
  lz77/lz77_method deliberately dropped by the multi-group section
  writer, fraction stride-quantized, palette/patches unimplemented on
  the lossless path — setter docs truthed upstream, issue rescoped to
  the wiring work), plus five mis-curated probes fixed from the run's
  evidence (see docs/VARIANT_GENERATION.md §6).

- Versioned public-API surface snapshot at `docs/public-api/zenjxl.txt`,
  regenerated on every `cargo test` by `tests/public_api_doc.rs`
  (`ZEN_API_DOC=check` verifies in CI's clippy job, `=off` skips); justfile
  recipes `api-doc` / `api-doc-check`.
- `zencodec::GainMapRender` wired through the decode trait path (job builder
  `with_gain_map_render` + trait/dyn parity): `BaseOnly` (default) attaches
  nothing; `Components` recursively decodes the jhgm gain-map codestream
  (same resource limits as the base decode) and surfaces BOTH
  `zencodec::decode::DecodedGainMap` and the raw `GainMapSource`;
  `ReconstructHdr` downgrades to Components per the zencodec contract
  (zenjxl surfaces, it does not apply — `reconstructs_hdr()` stays false).
  Unknown future modes error. Tests `tests/gain_map_render.rs` build the
  jhgm fixture in-test via jxl-encoder's `hdr-gainmap` (new dev-dep
  feature).
- zencodec 0.1.21 color-emit + metadata-policy adoption: `resolve_jxl_color` drives the ICC-vs-enum-color decision through `resolve_color_emit`; resolved CICP lowers to the codestream enum `ColorEncoding`; JPEG→JXL `Reencode` recompression preserves the source ICC instead of relabeling sRGB. Deps bumped to published zencodec 0.1.21 / zenpixels 0.2.11; butteraugli lock 0.9.0→0.9.3 (780d45eb).
- Native HDR decode signaling: decode-side output descriptors (probe `output_info` and full decode) carry the transfer function and primaries from the codestream CICP — a BT.2100-PQ JXL decodes as a PQ/BT.2020-tagged buffer. This also corrects the blanket `_LINEAR` claim on f32 output: when CICP is present the decoder renders into the signaled encoding for every depth (linear-sRGB float fallback only applies to XYB images with ICC-only profiles). Test `decode_descriptor_carries_cicp_pq_hdr`.

### Changed
- `JxlEncoderConfig::validate()` now rejects `with_noise(true)` combined
  with lossless mode (`ValidationError::NoiseInLosslessMode`, additive
  `#[non_exhaustive]` variant): noise synthesis is a lossy-VarDCT
  feature and was a silent no-op under the modular path. Generic
  quality knobs under lossless remain tolerated (zencodec pipelines set
  quality before toggling lossless) and are reported via
  `resolve_plan()`'s `inert_knobs` instead.

### Fixed
- CI: clone the `[patch.crates-io]` siblings (jxl-encoder, zenjpeg) at the paths the patch section names; the old workflow cloned to `../jxl-encoder--expert` and perl-stripped inline path deps, so every job failed manifest resolution since the patch section landed (d630212a). The expert-forwarding red that note originally carried (`lossless_expert_override_propagates_through_zenjxl`, imazen/jxl-encoder#67) was fixed upstream and the test passes — verified locally 2026-06-11 across multiple full-suite runs.

- **`jpeg-lossy` feature: lossy JPEG → JXL recompression closed loop**
  (`zenjxl::jpeg_lossy`). Drives a perceptual-quality **target** by bisecting a
  quality knob and scoring each candidate **in-process** (encode → decode →
  score) — zenjxl is the natural home because it deps both the encoder and the
  decoder. Two paths + a router via `JpegRecompressMethod`:
  - `Coarsen` — PreserveJxl coefficient-domain coarsening (jxl-encoder); best at
    gentle / near-lossless targets.
  - `Reencode` — full VarDCT pixel re-encode (reuses the lossless-transcode
    pixels as input, so both paths score against the same reference); best at
    medium / aggressive targets.
  - `Auto` (default) — the **router**: run both to the target and keep the
    smaller; beats either single path (content/target-dependent crossover).

  Entry points: `recompress_jpeg_lossy(jpeg, method, target, higher_is_better,
  scorer, effort)` (main), `recompress_jpeg_lossy_relative` (Coarsen
  convenience), `recompress_jpeg_coarsen` (explicit-scale, no loop).
  Metric-agnostic: the caller supplies a scorer callback over decoded RGB8, so
  the same loop hits a zensim-A / cvvdp / butteraugli target.

  Relative vs inferred targets via `QualityTarget` +
  `recompress_jpeg_lossy_target`:
  - `Relative` — distortion vs the source's own pixels (precise, measured).
  - `Inferred` — quality vs the unknown original, with the **achievability
    clamp**: an absolute target *better* than the source's floor is unreachable
    (you can't recover discarded detail), so the lossless transcode (the floor)
    ships — the dominant inferred byte win. Reachable targets aim at the
    caller-supplied `relative_target`.

  Preliminary helpers (clearly marked, NOT production-calibrated — N=5 CID22
  starting table pending a proper sweep): `predict_inferred_floor` (reads source
  IJG quality via `zenjpeg::detect` and interpolates the floor table per
  `InferredMetric`) and `QualityTarget::inferred_preliminary` (wires
  detect → floor → additive relative_target). Unreachable targets (and any path
  that can't reach the target) fall back to the lossless-transcode floor.
  Validated by `tests/jpeg_lossy.rs` (8/8). See the RD strategy in jxl-encoder's
  `docs/JPEG_LOSSY_RECOMPRESSION.md`.

### Changed
- Bump `jxl-encoder` dep to 0.3.2 (lossy-JPEG recompression API) + add `zenjpeg`
  0.8.7 (optional, `jpeg-lossy` only — source-quality detection for the inferred
  floor predictor). 0.3.2 and `zenjpeg` 0.8.7 are not yet on crates.io, so
  `[patch.crates-io]` redirects both to the local siblings (the same pattern
  jxl-encoder uses for its unpublished `zenjpeg` dep). Builds now require the
  sibling checkouts until the chain is published.

## [0.2.1] - 2026-05-02

### Changed
- Bump minimum `jxl-encoder` dependency to 0.3.1 (published with the
  `__expert` cargo feature + segmented `LossyInternalParams` /
  `LosslessInternalParams`). Drops the local `[patch.crates-io]`
  override.

### Added
- `validate()` methods on zenjxl-owned Config types (`JxlEncoderConfig`,
  `JxlDecoderConfig`) and a new `ValidationError` enum re-exported from
  the crate root. Setters keep clamping out-of-range inputs as before;
  `validate()` is an opt-in fail-fast for batch jobs that want to
  refuse silently-clamped values. Catches `generic_quality` outside
  `0.0..=100.0` (or NaN) — the only zenjxl-side knob whose setter does
  not clamp. A `JxlEncoder` variant on `ValidationError` is reserved
  behind `__expert` for forwarding `jxl-encoder::ValidationError` once
  upstream lands its own `validate()` methods. New `tests/validate.rs`
  covers happy-path, out-of-range, NaN, and clamped-setter cases.
- New `__expert` cargo feature forwards `jxl-encoder/__expert` for
  picker training and codec calibration sweeps. Re-exports the
  segmented `LossyInternalParams` and `LosslessInternalParams` types
  plus `EncoderMode` and `EntropyMulTable` at the crate root (gated
  behind `__expert`); the
  `LossyConfig::with_internal_params(LossyInternalParams)` /
  `LosslessConfig::with_internal_params(LosslessInternalParams)`
  builders on the already-re-exported `LossyConfig` / `LosslessConfig`
  do the work. Double-underscore prefix signals "private — do not
  depend on this in production code." Anything in the underlying
  escape hatch may change without semver bumps. Pulls jxl-encoder's
  `__expert` feature; see jxl-encoder feat/expert-internal-params
  branch.
- `tests/expert_forwarding.rs` smoke test (gated on `__expert`)
  verifying that `LossyInternalParams` (`try_dct16` /
  `try_dct32 = Some(false)`) and `LosslessInternalParams`
  (`nb_rcts_to_try = Some(0)`) overrides propagate through the
  re-exports and change the produced JXL bitstream. Exhaustive
  per-knob coverage lives upstream in jxl-encoder's
  `effort_expert_tests`.

### Changed
- Tracks jxl-encoder's segmentation refactor
  (imazen/jxl-encoder#26): the previously re-exported `EffortProfile`
  is now `#[doc(hidden)]` upstream and the
  `with_effort_profile_override` builders are removed. zenjxl now
  re-exports the per-mode `LossyInternalParams` /
  `LosslessInternalParams` (`#[non_exhaustive]`, `Default`, all fields
  `Option<T>`) and forwards them via the new `with_internal_params`
  builders. The escape hatch is still gated behind `__expert`; the
  surface area is just narrower per mode (lossy knobs cannot be handed
  to the lossless encoder and vice versa).

### Internal
- While jxl-encoder's `feat/expert-internal-params` branch is
  unmerged, `[patch.crates-io] jxl-encoder` points at the sibling
  worktree `../jxl-encoder--expert/jxl-encoder`. Revert to
  `../jxl-encoder/jxl-encoder` once the branch lands, and drop the
  patch entirely once the rename publishes. Both must happen before
  any zenjxl release.

## [0.2.0] - 2026-04-17

### BREAKING CHANGES
- Re-exports from `jxl-encoder` which bumped 0.2 to 0.3

### Added
- `parallel` feature forwards to `jxl-encoder/parallel`, enabling rayon-based
  per-frame parallelism inside the encoder (c3c1d1e). Previously, callers
  could only parallelize across encode calls — now a single high-effort
  encode can saturate multiple cores. Thanks to the `jxl-encoder` crate for
  providing the underlying parallelism.
- Accept `RGBX8_SRGB` and `BGRX8_SRGB` descriptors in `supported_descriptors()`
  and encode dispatch, stripping the undefined padding byte and routing as
  `PixelLayout::Rgb8` (b384cbd). Applied across `Encoder::encode`, `push_rows`,
  and `AnimationFrameEncoder::push_frame`; round-trips verified against the
  `zenjxl-decoder` path.
- Surface JXL `ToneMapping.intensity_target` as zencodec
  `ContentLightLevel.max_content_light_level` for HDR-aware downstream code
  (04556d7, #3). Only reports values above the 255-nit SDR default; leaves
  MaxFALL at 0 since JXL has no equivalent signal.
- Set `ColorAuthority::Cicp` when JXL carries enum (CICP) color encoding with
  `want_icc=false`, so CMS code trusts the structured signaling over the
  synthesized ICC copy (e03c185, #2).

### Changed
- Migrate `ThreadingPolicy` plumbing to the `is_parallel()` helper from
  `zencodec` 0.1.18 (06b8eb3). Uses `Sequential`/`Parallel` in place of the
  deprecated `SingleThread`/`Unlimited` variants.
- Bump `zencodec` to 0.1.13, picking up JP2/DNG/RAW/SVG format detection and
  the `max_total_pixels` resource limit (6783447).

### Fixed
- Update `policy_to_threads` doc comment to match `jxl-encoder`'s ambient
  rayon pool semantics: `threads=0` means "use the ambient pool" rather than
  "create a default pool" (778b192). Simplified the `Balanced`/`Unlimited`
  match arms accordingly.

### Documentation
- README: add the `parallel` feature row to the feature table, drop the
  stale `zennode`/`EncodeJxl`/`DecodeJxl` references, and switch shields.io
  badges to `?style=flat-square` for repo-wide consistency (1caed1d).

### Internal
- Gitignore tooling noise (`.superwork/`, `.claude/`, `.zenbench/`,
  `copter-report/`, profraw/profdata, fuzz logs, `Cargo.toml` backups) and
  exclude dev-only files (`CLAUDE.md`, `CONTEXT-HANDOFF.md`, `.github/`,
  `justfile`, `fuzz/`) from the published crate package (31a1a6a).
- `cargo fmt` pass (c4994cf).

## 0.1.1 (2026-04-01)

- Bump `jxl-encoder` 0.1.4 -> 0.2.0, `zenjxl-decoder` 0.3.4 -> 0.3.5
- Bump `zencodec` 0.1.11 -> 0.1.12 (fixes `parse_iso21496_fmt` API change)
- Re-enable 13 tests previously blocked by zenjxl-decoder 0.3.4 panic

## 0.1.0 (2026-04-01)

Initial release.

- JPEG XL encoding via `jxl-encoder` 0.2.0 with effort/quality/distance controls
- JPEG XL decoding via `zenjxl-decoder` 0.3.5 with streaming support
- `zencodec` trait integration (feature-gated)
- Gain map (HDR) support with ISO 21496-1 metadata parsing
- Probe API for extracting JXL image info without full decode
- Butteraugli-loop encoding (feature-gated)
- Multi-threaded decoding (feature-gated)
