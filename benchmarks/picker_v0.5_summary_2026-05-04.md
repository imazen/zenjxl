# zenjxl v0.5 picker — HOLD (real measurement)

**Status:** HOLD. Distance-banded A/B confirms picker doesn't beat static defaults at matched-distance bytes.

## Trained on v05c sweep data

- Sources: 1241 images
- Sweep grid: effort ∈ {1,3,5,7,9} × distance ∈ {0.05..15} × noise ∈ {F,T}
- Sweep rows: 266,636
- Cells: 5 efforts; scalar head: distance
- MLP: 112 → 128 → 128 → 15, 32911 params, 40 KB i8

## Picker quality
- Teacher argmin acc: 48.3% / Student val: 51.0%
- Train→val gap: +6.0pp (overfit)
- Picker cell preference: 60% effort3, 18% effort5, 13% effort9, 9% effort7

## Distance-banded A/B (248 imgs, 4683 cells, seed=7)

vs default `effort=7`:

| band | n | mean Δbytes | win rate | Δzensim_pp |
|---|---:|---:|---:|---:|
| tight (0.05..1.0) | 1717 | -0.70% | 43.1% | +0.15 |
| mid (1.0..3.0) | 1235 | +2.14% | 39.4% | +1.07 |
| loose (3.0..15) | 1731 | +7.58% | 29.2% | +3.46 |
| **overall** | **4683** | **+3.11%** | **37.0%** | **+1.62** |

vs default `effort=3` (picker's most-preferred): -0.46% bytes / -0.99pp zensim — quality regress, not clean win.

## Diagnosis

Picker has strong prior toward effort=3 (60% of decisions). When it picks differently it's roughly random — overall worse than just always picking effort=7 or effort=3 statically.

The sweep is doing its job: same-distance comparisons show effort=7 dominates effort=3 on bytes-at-quality (smaller files for same butteraugli/zensim). The picker hasn't learned this bias correction.

## Next steps

1. **Time-aware objective**: include encode_ms in the picker loss; effort=3 wins on speed but loses on bytes-per-quality. Picker should reflect that cost.
2. **More features**: compressibility prediction needs orientation/edge features the v04full feature set may lack.
3. **Better corpus balance**: 60% picker→effort3 may be data bias from synthetic/screen content.

## Artifacts
- `benchmarks/zenjxl_picker_v0.5_2026-05-04.bin` (40 KB i8)
- `benchmarks/zenjxl_picker_v0.5_2026-05-04.manifest.json`
- `benchmarks/picker_v0.5_holdout_ab_distance_2026-05-04.{md,tsv}` (proper distance-banded measurement)
- `s3://zentrain/zenjxl/pickers/zenjxl_picker_v0.5_2026-05-04.{bin,manifest.json}`
- Pareto TSV: `s3://zentrain/sweep-v05c-2026-05-04/zenjxl_pareto_concat.tsv`
