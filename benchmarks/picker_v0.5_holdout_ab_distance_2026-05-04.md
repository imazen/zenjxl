# zenjxl held-out A/B (distance-banded)

**Verdict: HOLD**

- Holdout: 248 images (frac=0.2, seed=7); 4683 (image, distance) cells.
- Default cell: effort7
- Picker cell preference: {'effort3': 2812, 'effort5': 855, 'effort9': 598, 'effort7': 418}

## Per-band results

| band (distance) | n | mean Δbytes % | median Δbytes % | win rate | mean Δzensim_pp |
|---|---:|---:|---:|---:|---:|
| tight (0.05..1.0) | 1717 | -0.70 | +0.00 | 43.1% | +0.15 |
| mid (1.0..3.0) | 1235 | +2.14 | +0.15 | 39.4% | +1.07 |
| loose (3.0..15) | 1731 | +7.58 | +2.19 | 29.2% | +3.46 |
| overall | 4683 | +3.11 | +0.41 | 37.0% | +1.62 |
