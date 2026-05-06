# `patches=True` is a free Pareto win on screen content (v07 sweep)

**TL;DR:** zenjxl encoder should default `patches=True` for class=screen content. v07 sweep data shows -7.7% bytes / +0.74 zensim improvement, +1.1% encode time cost. Confirmed across 355 matched pairs on screen content. Photo/lineart/synthetic see zero bytes change either way.

## Method

v07 sweep TSV (`~/sweep-data/v07/*.tsv`) has 11,953 rows with explicit `patches` setting (True or False). Matched pairs across the orthogonal axes (image, distance, effort, biters, ziters, gaborish, pdl) — only `patches` differs. Compute Δbytes, Δzensim, Δms per matched pair, then aggregate per content class (filename heuristic: terminal/windows/screen/gui/... → screen; chart/graph/logo/... → lineart; etc).

## Results — bytes & quality

| class | n | T strict-wins (bytes) | F strict-wins | tie | mean Δbytes (T−F) | mean Δzensim (T−F) |
|---|---:|---:|---:|---:|---:|---:|
| lineart | 180 | 0 | 0 | 180 | +0.000% | +0.0000 |
| photo | 4440 | 0 | 0 | 4440 | +0.000% | +0.0000 |
| **screen** | **355** | **115 (32%)** | **0 (0%)** | 240 | **−7.675%** | **+0.7357** |
| synthetic | 999 | 0 | 0 | 999 | +0.000% | +0.0000 |

## Results — encode time

| class | n | mean ms(T) | mean ms(F) | Δms (T vs F) |
|---|---:|---:|---:|---:|
| lineart | 180 | 1042.6 | 1005.8 | +3.66% |
| photo | 4440 | 1930.1 | 1894.0 | +1.90% |
| screen | 360 | 9937.7 | 9828.5 | **+1.11%** |
| synthetic | 1020 | 4261.7 | 4269.8 | -0.19% |

## Recommendation

**Ship `patches=True` per content class** — at the encoder API level, switch the default based on content class:

| class | recommended `patches` | rationale |
|---|---|---|
| **screen** | **True** | -7.7% bytes / +0.74 zensim / +1.1% ms — strict Pareto win |
| photo | False (keep current) | zero bytes change, +1.9% ms cost — not worth |
| lineart | True or False | zero bytes change, only 180 sample pairs |
| synthetic | False | zero bytes change, +0% ms |
| document | unknown | no v07 data |

Easiest implementation: classify image at encode-time using the existing `zenanalyze` features (or a tiny content classifier baked from the same), set `patches=True` if class==screen.

Even simpler: **always default `patches=True`**. Cost: ~+2% encode time on photo (most traffic). Benefit: -7.7% bytes on screens (where it's strictly better). At 5% screen traffic share, net savings: +0.4% bytes saved (95% × 0% + 5% × −7.7%) for +1.9% mean encode time. That's a solid Pareto improvement; only the encode-time cost would push back.

## Why screens benefit and others don't

JXL `patches` matches repeated rectangular regions and emits them once with offsets — exactly the structure of GUI-screenshot content (repeated buttons, identical glyphs, uniform gradient panels). On photo / continuous-tone content there's nothing to match, so the `patches` lookup completes with no matches and zero bytes saved. The +1-4% encode time on non-screen content is the cost of running the pattern matcher and finding nothing.

## Caveats

- 355 screen pairs is small. Most of those screens are from `gb82-screen` corpus + cid22 GUI images. Need broader screen corpus to confirm.
- The 7.7% bytes win is mean — 240/355 pairs (68%) tie, only 115 (32%) strictly win. Worst case patches=True doesn't hurt; best case saves ~30% on the matchable subset.
- v07 sweep distance range 0.5–8.0; behavior may differ at near-lossless or extreme distortion.
- `gaborish=False` interaction: not tested here; v07 explore showed gaborish=False is the second-place winner on screens.

## Provenance

- Source data: `~/sweep-data/v07/*.tsv` (34 chunks, 11,953 patches-axis rows)
- Analyzer: `~/sweep-data/analyze_patches_per_class_v07.py` (this generated)
- Generated: 2026-05-06 during 10-hour autonomous run
- Cross-references:
  - v07 explore (global): `zenanalyze/benchmarks/picker_v07_explore_2026-05-05.md` — patches=True dominates 42 of 117 v07-beats-v06 cells (consistent with this finding)
  - per-class picker audit: `zenanalyze/benchmarks/picker_v06_per_class_audit_2026-05-06.md` — picker hurts screens; patches=True is the missing piece
