# zenjxl benchmarks — index

Catalog of empirical reports informing encoder defaults, picker design,
and shipped artifacts. Newest at top.

## Encoder defaults — empirical decisions

| Decision | Date | Source | Verdict |
|---|---|---|---|
| **lz77 vs RLE on VarDCT** | 2026-05-06 | [lz77_vs_rle_v07_2026-05-06.md](lz77_vs_rle_v07_2026-05-06.md) | **KEEP `lz77=True`** — RLE matches LZ77 on photo (98.5%) but regresses synthetic gradients 100-389%. Speed: RLE -25% at e9 only. |

## Picker training — sweep + champion lineage

For per-sweep reports see also `~/work/zen/zenanalyze/benchmarks/`.

| Sweep | Date | Champion | Δbytes | Δzensim | Status |
|---|---|---|---|---|---|
| **v06** (multi-target) | 2026-05-05 | `zensim_mask_histgb` | -1.879% | +0.402 | Top — recommended ship target |
| v06 (single-target safety mask) | 2026-05-04 | `zensim_mask_mlp` v0.6 | -2.19% | +0.32 | superseded |
| v0.5 (full-grid retrain) | 2026-05-04 | distance-banded HOLD | +3.11% overall | +1.62 | HOLD — picker no better than static effort=7 |
| v0.4 | 2026-05-04 | HOLD | — | — | superseded |

**Cross-reference:**
- v06 picker variants: `~/work/zen/zenanalyze/benchmarks/picker_v06_multi_99chunks_2026-05-05.md`
- v06+v07 union (patches/gaborish/pdl): `~/work/zen/zenanalyze/benchmarks/picker_v06_v07_union_2026-05-05.md`
- v07 knob exploration: `~/work/zen/zenanalyze/benchmarks/picker_v07_explore_2026-05-05.md`
  - **Key finding: `patches=True` is the dominant Pareto winner** (42 of 117 v07-beats-v06 cells)
  - Secondary: `gaborish=False`
  - v07's best safe-alt saves 8.16% bytes vs v06's 5.26%

## Shipped picker artifacts (in this dir)

| File | Version | Notes |
|---|---|---|
| `zenjxl_picker_v0.4_2026-05-04.bin` | v0.4 | A/B HOLD — superseded |
| `zenjxl_picker_v0.4_2026-05-04.manifest.json` | v0.4 | manifest |
| `zenjxl_picker_v0.5_2026-05-04.bin` | v0.5 | full-grid HOLD — superseded |
| `zenjxl_picker_v0.5_2026-05-04.manifest.json` | v0.5 | manifest |

v0.6 picker (zensim_mask_mlp) lives in `~/work/zen/zenanalyze/benchmarks/zenjxl_picker_v0.6_safety_2026-05-04.bin`.

## Methodology notes

- **Safety mask** during training: only label a cell as "winner" if it
  satisfies `bytes < default × 0.99 AND zensim ≥ default - 0.05 AND ms ≤ default × 1.05`.
  Without the mask, picker chases bytes wins that cost zensim or 5×
  encode time — see `zensim_nomask_mlp` row in v06 multi-target report
  (-8.7% bytes BUT -1.32 zensim AND +291% encode time → unacceptable).
- **HistGradientBoosting beats MLP** on this data; tree models handle
  the (image, distance, knob) heterogeneity better.
- **Multi-target weighted picker** (`multi_mask_mlp`) is more
  conservative (76% default-rate) but only -1.05% bytes — weaker than
  single-target zensim picker.

## Sweep data location

- v06: `~/sweep-data/zenjxl_v06.tsv` (37 MB, 165k cells)
- v07: `~/sweep-data/v07/*.tsv` (34 chunks, 32k cells; lz77/patches/gaborish axes)
- v08-v11: `~/sweep-data/v0X/*.tsv` per sweep (status varies; see `~/sweep-data/NOTES.md`)
- All sweeps mirrored to R2 at `s3://zentrain/sweep-vXX-YYYY-MM-DD/`

## Training recipe (cloud)

See `~/work/zen/zen-train-docker/CLAUDE.md` for end-to-end recipe.
Verified at 2026-05-06: cloud `picker_multi` reproduces local result
byte-exact in ~3 min on a $0.06/hr CPU box.
