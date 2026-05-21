# zenjxl

Thin Rust wrapper around `jxl-encoder` (C++ libjxl binding). Most knob selection delegates to libjxl internals via the `__expert` feature; this crate exposes a Rust-friendly API surface on top.

## Canonical training data + indexes (added 2026-05-20)

**The canonical index for all ML data lives at `~/work/zen/DATA_PROVENANCE.md`.**

Quick paths:
- Trainer input: `/mnt/v/zen/zensim-training/canonical-2026-05-21/`
- Master inventory: `~/work/zen/_ml-inventory-2026-05-20/00-MASTER-SYNTHESIS.md`
- Per-codec picker audit: `~/work/zen/_ml-inventory-2026-05-20/05-per-codec-pickers.md`

## ML/picker status (2026-05-20)

zenjxl ships **no internal picker.** Knob selection is delegated to `jxl-encoder` internals.

Training data for any future jxl picker lives in `benchmarks/zenjxl_*` (pareto sweeps + feature CSVs). For reference picker wiring see `~/work/zen/zenavif/src/auto_tune.rs` (the only production-shipped zen-codec picker).
