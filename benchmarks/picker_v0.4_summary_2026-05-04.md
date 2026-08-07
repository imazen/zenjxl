# zenjxl v0.4 picker — summary

**Status:** trained, baked, table-lookup A/B run. **Production verdict: HOLD pending v0.4-extension sweep.**

## Trained model

| metric | value |
|---|---|
| val argmin_acc | **63.3%** (vs v0.3's 11%) |
| val mean overhead | **3.62%** |
| train→val gap | +1.25pp (mild overfit) |
| n_inputs / n_outputs / n_layers | 144 / 6 / 3 |
| .bin size | 43 KB (i8) |
| schema_hash | 0x7a00fae38120d94f |

## Cell taxonomy (REDUCED vs v0.4 spec)

The v0.4 spec asked for 4 effort × distance product. The actual sweep
ran **2 effort cells**, no distance axis (`effort ∈ {3, 7}`, distance
always defaulted to 1.0 placeholder).

| picker dimension | v0.4 spec | actual collected | gap |
|---|---|---|---|
| effort values | broader | {3, 7} | mid only |
| distance values | swept | none | no signal |
| q values per image | 16 (step 5) | 10 (step 10) | coarser |

## Held-out A/B (table-lookup, 117/587 images, seed=7)

| baseline | mean Δbytes | mean Δzensim_pp | n |
|---|---:|---:|---:|
| `effort=7` (more compressive) | +1.41% | **+0.60** | 819 |
| `effort=3` (faster, less compressive) | -4.12% | **-0.76** | 819 |

**Reading:** picker doesn't have a clean win. vs `effort=7` it's
slightly bigger but slightly higher quality (favours `effort=3` for
some images). vs `effort=3` it's smaller but lower quality. Neither is
a strict bytes-at-matched-quality improvement.

## Recommendation

1. **Do not ship as default JXL knob picker.** Static `effort=7` is a
   strict-better choice within this 2-effort grid for byte-at-quality.
2. **Rerun sweep with full v0.4 grid:** `effort ∈ {3, 5, 7, 9}` and the
   `distance` axis at multiple values. Distance is the second-most
   impactful JXL quality knob; the picker has no signal on it from this
   sweep.
3. **Picker quality itself is good** (63.3% val argmin_acc, 3.62% mean
   overhead) — the limitation is data shape, not model capacity.

## Artifacts
- `benchmarks/zenjxl_picker_v0.4_2026-05-04.bin` (43 KB)
- `benchmarks/zenjxl_picker_v0.4_2026-05-04.manifest.json`
- `benchmarks/picker_v0.4_holdout_ab_2026-05-04.md`
- `s3://zentrain/zenjxl/pickers/zenjxl_picker_v0.4_2026-05-04.bin`
- Sweep TSV at `s3://zentrain/sweep-v04-2026-05-04/zenjxl_pareto_concat.tsv` (11,740 rows, 587 imgs × 20 configs)
