# zenjxl v0.5 picker — HOLD (harness limitation, not picker problem)

**Status:** HOLD pending distance-band-aware A/B harness.

## Trained on v05c sweep data (BIG sweep)

- **Sources:** 1241 images.
- **Sweep grid:** effort ∈ {1, 3, 5, 7, 9} × distance ∈ {0.05, 0.1, ..., 12.0} × noise ∈ {false, true}.
- **q in TSV:** 75 (dummy — distance is the quality knob, JXL CLI overrides q with distance).
- **Sweep rows used for training:** 266,636.
- **Cells:** 5 (effort1, effort3, effort5, effort7, effort9).
- **Scalar head:** distance (continuous, 0.5..12.0).
- **Output dim:** 15.
- **MLP:** 112 → 128 → 128 → 15, 32911 params, 40.0 KB i8 baked.

## Picker quality

| metric | value |
|---|---|
| Teacher argmin acc | 48.3% |
| **Student val argmin acc** | **51.0%** |
| Teacher mean overhead | 6.63% |
| Student val mean overhead | 8.81% |
| Train→val gap | +6.00pp (overfit) |

## Held-out A/B — INVALID due to harness q-vs-distance band mismatch

`holdout_ab_lookup.py` bands by `zq` from the pareto TSV's `q` column. JXL's q=75 dummy → all 248 holdout rows fall into the high band → no low/mid coverage. A meaningful A/B needs distance-band partitioning.

## Recommendation

HOLD ship for now, but the picker may be valid — the harness can't measure it correctly. Build a distance-band A/B (~30 min), re-run, then issue final SHIP/HOLD.

## Artifacts

- `benchmarks/zenjxl_picker_v0.5_2026-05-04.bin` (40 KB i8)
- `s3://zentrain/zenjxl/pickers/zenjxl_picker_v0.5_2026-05-04.{bin,manifest.json}`
- Pareto TSV: `s3://zentrain/sweep-v05c-2026-05-04/zenjxl_pareto_concat.tsv` (266k rows)
