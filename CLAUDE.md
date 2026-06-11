# zenjxl

Thin Rust wrapper around `jxl-encoder` (pure-Rust JPEG XL encoder) and `zenjxl-decoder` (pure-Rust decoder, lib name `jxl`). Most knob selection delegates to jxl-encoder internals via the `__expert` feature; this crate exposes a Rust-friendly API surface on top.

## Variant generation / sweep infrastructure (added 2026-06-11)

The zenjpeg variant-generation patterns are adopted here — **read
`docs/VARIANT_GENERATION.md` first** (jxl-specific adoption + the
dominance/trial/metric audit); the codec-neutral playbook (16 patterns)
lives in `zenjpeg/docs/VARIANT_GENERATION.md`.

- `src/sweep.rs` (gated `encode` + `__expert`): mode-discriminated
  `LossyVariant`/`LosslessVariant`, `SweepAxes × QualityGrid` planner with
  budget ladder, FNV byte-identity `fingerprint` over resolved state
  (generic-q calibration plateau q≤20 dedupes), curated axes with
  provenance table in the module docs.
- `JxlEncoderConfig::resolve_plan()` (`zencodec`+`encode`+`__expert`):
  reads the same stored upstream config the encode consumes; lossless
  plans report dead knobs. `validate()` rejects noise×lossless.
- `examples/sweep_validate.rs` — the empirical harness (inert steps,
  fingerprint contracts, **lossless roundtrip exactness**, ordering, ssim2
  floors). Run via `just sweep-validate`; needs
  `CODEC_CORPUS_DIR=$HOME/work/codec-eval/codec-corpus`. Re-run whenever
  axes, the fingerprint, or jxl-encoder bumps change — upstream
  `*InternalParams` are `#[non_exhaustive]`, so new knobs don't enter the
  fingerprint automatically. Results: `benchmarks/sweep_validate_jxl_*.tsv`
  (committed, git commit in header).
- The harness's maiden runs found jxl-encoder#68 (two independent e9+
  lossless bitstream-corruption causes, both fixed upstream 2026-06-10/11:
  `5eefe5f7` + `329f207d`) and #69 (lossless lz77/palette/patches knobs
  not consumed — those axes stay out until wired). Follow-ups tracked in
  **imazen/zenjxl#8**.
- jxl-encoder's #68 fixes are **not yet in a crates.io release** — local
  builds get them via the `[patch.crates-io]` path dep; published-dep
  consumers (CI clones siblings, so it's covered) need the next
  jxl-encoder release. zenjxl-decoder 0.3.9 is published (`^0.3.7`
  resolves to it).

## Canonical training data + indexes (added 2026-05-20)

**The canonical index for all ML data lives at `~/work/zen/DATA_PROVENANCE.md`.**

Quick paths:
- Trainer input: `/mnt/v/zen/zensim-training/canonical-2026-05-21/`
- Master inventory: `~/work/zen/_ml-inventory-2026-05-20/00-MASTER-SYNTHESIS.md`
- Per-codec picker audit: `~/work/zen/_ml-inventory-2026-05-20/05-per-codec-pickers.md`

## ML/picker status (2026-05-20)

zenjxl ships **no internal picker.** Knob selection is delegated to `jxl-encoder` internals.

Training data for any future jxl picker lives in `benchmarks/zenjxl_*` (pareto sweeps + feature CSVs); the sweep planner above is the cell-generation side of that pipeline. For reference picker wiring see `~/work/zen/zenavif/src/auto_tune.rs` (the only production-shipped zen-codec picker).
