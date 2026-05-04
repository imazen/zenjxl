# zenjxl v0.4 picker held-out A/B (table-lookup)

**Verdict: HOLD**

- Holdout: 248 of 1241 images (frac=0.2, seed=7)
- Method: table-lookup over the v0.4 sweep TSV; picker chooses cell, default cell = effort7
- Cells in sweep: {'effort3': 55001, 'effort5': 55001, 'effort7': 55001, 'effort9': 54475, 'effort1': 47158}
- Picker cell preference (held-out): {'effort3': 148, 'effort5': 45, 'effort9': 33, 'effort7': 22}

## Per-band results

| band | n | mean Δbytes % | median Δbytes % | win rate (Δ<-0.1%) | mean Δzensim pp |
|---|---:|---:|---:|---:|---:|
| zq30..49 (low) | 0 | +0.00 | +0.00 | 0.0% | +0.00 |
| zq50..74 (mid) | 0 | +0.00 | +0.00 | 0.0% | +0.00 |
| zq75..95 (high) | 248 | +5.33 | +1.66 | 31.0% | +4.04 |
| overall | 248 | +5.33 | +1.66 | 31.0% | +4.04 |

## Reading
- A HOLD picker should beat the default cell on bytes at matched quality.
- Δbytes < 0 means picker is smaller. Δzensim_pp > 0 means picker is sharper.
- This is a TABLE-LOOKUP A/B: it does not measure closed-loop target_zensim convergence. The closed-loop SHIP gate is a separate harness.
- The v0.4 sweep grid is reduced (2 cells); the binary search space is small. Mean overhead is bounded above by the cell delta at any (img, q).
