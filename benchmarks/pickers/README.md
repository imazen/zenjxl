# Algorithmic (decision-tree) picker code-heuristics — 2026-06-28

CART → Rust code-heuristics distilled from the clean-split dual-model program
(`zenmetrics/docs/CLEAN_PICKER_PROGRAM.md`). Each `pick_*` fn maps zenanalyze
features + target-q to a config cell, no model load. Byte-exact verified vs the
fitted tree. Companion to the MLP pickers (the neural alternative).

Measured val overhead (origin split, train=even / val=1,3,5):
- `zenjxl_lossy_cart_zensim`  : 6.9%  (MLP peer: 0.55%)
- `zenjxl_lossy_cart_ssim2`   : 7.9%  (MLP peer: 0.61%)
- `zenjxl_lossless_cart`      : 0.73% (bytes-target; lossless = metric-insensitive)

The lossy MLP (`train_hybrid`, ~0.55–0.61% overhead, generalizes val→test ≤0.13pp)
is excellent but its clean ZNPR `.bin` bake is gated on genuine high-q sparsity
(tiny/zq92–94 have 2–25 rows) + two 1-config cells — not shipped via --allow-unsafe.
