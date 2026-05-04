# zenjxl v0.4 picker held-out A/B (table-lookup)

**Verdict: HOLD**

- Holdout: 117 of 587 images (frac=0.2, seed=7)
- Method: table-lookup over the v0.4 sweep TSV; picker chooses cell, default cell = effort7
- Cells in sweep: {'effort3': 5870, 'effort7': 5870}
- Picker cell preference (held-out): {'effort7': 502, 'effort3': 317}

## Per-band results

| band | n | mean Δbytes % | median Δbytes % | win rate (Δ<-0.1%) | mean Δzensim pp |
|---|---:|---:|---:|---:|---:|
| zq30..49 (low) | 234 | +0.68 | +0.00 | 12.8% | +0.20 |
| zq50..74 (mid) | 234 | +4.28 | +0.00 | 15.4% | +1.30 |
| zq75..95 (high) | 351 | -0.03 | +0.00 | 23.9% | +0.41 |
| overall | 819 | +1.41 | +0.00 | 18.3% | +0.60 |

## Reading
- A HOLD picker should beat the default cell on bytes at matched quality.
- Δbytes < 0 means picker is smaller. Δzensim_pp > 0 means picker is sharper.
- This is a TABLE-LOOKUP A/B: it does not measure closed-loop target_zensim convergence. The closed-loop SHIP gate is a separate harness.
- The v0.4 sweep grid is reduced (2 cells); the binary search space is small. Mean overhead is bounded above by the cell delta at any (img, q).
