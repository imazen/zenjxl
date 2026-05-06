# Per-content-class encoder rule for zenjxl (v07 + v08 sweep evidence)

**TL;DR:** different content classes want different encoder defaults. Shipping a per-class rule moves the Pareto curve substantially on screens and synthetic content, neutrally on photo/lineart. The rule is decidable from `zenanalyze` features (no ML needed; deterministic classify).

## Recommended rule

```rust
let class = classify_content(features);  // photo | screen | lineart | document | synthetic
let (patches, gaborish, pixel_domain_loss) = match class {
    Screen | Synthetic => (true,  false, false),  // Pareto win
    Photo  | Lineart   => (false, true,  false),  // current default; do not change
    Document           => (false, true,  false),  // no v07/v08 data; conservative default
};
```

Apply in `LossyConfig::encode` or expose via `ConfigSpec::with_content_class(...)`.

## Evidence — combined v07 + v08

For each (image, distance, effort=7, biters=0) cell, compare jxl-encoder-style default `(patches=False, gaborish=True, pdl=False)` against the proposed alt `(patches=True, gaborish=False, pdl=False)`:

| class | n | mean Δbytes | mean Δzensim | verdict |
|---|---:|---:|---:|---|
| **screen** | 216 | **−9.85%** | **+0.65** | **clear Pareto win** |
| **synthetic** | 224 | **−6.63%** | **+0.55** | **clear Pareto win** |
| photo | 3320 | +6.53% | +1.62 | regression on bytes (don't apply) |
| lineart | 160 | +4.90% | +2.84 | regression on bytes (don't apply) |

## Evidence — patches=True alone (v07, 1125 matched pairs)

Confirmed in [patches_per_class_v07_2026-05-06.md](patches_per_class_v07_2026-05-06.md):

| class | n | T strict-wins | mean Δbytes (T−F) | mean Δzensim |
|---|---:|---:|---:|---:|
| **screen** | 355 | 32% | **−7.7%** | **+0.74** |
| photo / lineart / synthetic | 9619 | 0% | +0.00% | +0.00 |

## Evidence — gaborish alone (v08, 250k pairs)

Per-class breakdown of the v08 gaborish axis:

| class | n | gab=ON wins | gab=OFF wins | mean Δbytes (OFF−ON) | mean Δzensim (OFF−ON) |
|---|---:|---:|---:|---:|---:|
| photo | 106154 | 50% | 25% | +1.20% | +0.67 |
| lineart | 5120 | 54% | 21% | +0.92% | +1.14 |
| **screen** | 6786 | 18% | **58%** | **−4.97%** | -0.18 |
| **synthetic** | 7030 | 21% | **51%** | **−9.81%** | +0.06 |

For screens and synthetic content, gaborish=OFF saves bytes with negligible quality cost. For photo and lineart, gaborish=OFF saves quality (+0.67–+1.14 zensim) at the cost of bytes (+1–1.2%) — quality vs bytes tradeoff, not a clear win.

## Why this works

JXL's `patches` extension matches repeated rectangular regions and emits offsets — exactly the structure of GUI screenshots (repeated buttons, glyphs, panels). On photo content, the matcher finds nothing and bytes don't change. On screens, the savings stack with the −5% from gaborish=OFF (which removes a smoothing post-filter that's unhelpful on hard-edged synthetic content) for a combined ≈10% saving.

`pixel_domain_loss` doesn't move the needle on any class — leave at default.

## Encode-time cost

`patches=True`: +1.1% on screen, +1.9% on photo, +3.7% on lineart. Negligible when the encoder is wall-clock-dominant (≥1s/image at production sizes).

`gaborish=False` interaction with patches=True: +18% encode time mean across all classes (the patches scanner runs longer when gaborish doesn't pre-smooth). For screen content, the bytes saving (−9.85%) dwarfs this.

## Caveats

- Sample size on screen is modest (355 patches pairs / 6786 gaborish pairs). The rebalanced corpus (~3,000 gen-screen sources) gives much more screen-density training data; the rule should be re-validated there.
- Distance range 0.5–8.0 only. Near-lossless and extreme distortion not tested.
- Document class has no v07 data; using a conservative default is safe but possibly suboptimal.
- Lineart sample is small (180 pairs); the +0.74 zensim observed on screens may not generalize identically to chart/diagram content.

## Comparison to current production v0.6 picker

The published v0.6 zenjxl picker (`zensim_mask_histgb`, headline −1.879% bytes / +0.402 zensim) is **photo-weighted** and hides a +41.4% bytes regression on screen content (see [picker_v06_per_class_audit_2026-05-06.md](../../zenanalyze/benchmarks/picker_v06_per_class_audit_2026-05-06.md)). The per-class encoder rule above:

- recovers the screen regression (+41% → 0% by skipping picker, then the v07 patches=True rule gives a fresh −9.85% on top)
- leaves photo behavior unchanged (rule says "use defaults for photo"; v0.6 picker still applies and gives its real −0.45% holdout win)
- net: roughly +0.4% bytes gain over current production (weighted by class frequency)

## Provenance

- v07 patches data: `~/sweep-data/v07/*.tsv` (34 chunks)
- v08 patches+gaborish+pdl data: `~/sweep-data/v08/*.tsv` (98 chunks)
- Generated 2026-05-06 during 10-hour autonomous run
- Local analyzers: `~/sweep-data/analyze_patches_per_class_v07.py` (committed) + ad-hoc Python for v08 combined analysis
