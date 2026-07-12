# Proxy-policy sweep + time/RD policy optimization — 2026-07-12

Question: for an IMAGE PROXY doing lossless JXL, what per-image policy
(which encode configs to try, gated on what) is time- and RD-optimal?

## Method
- Corpus: 1,000 imazen-26 origins (the prior 414 clean-picker origins +
  586 new, stratified proportionally across all 21 content classes),
  6,841 renditions (prior 11-size ladders + 4-size ladders maxdim
  {64,192,512,1024} for the expansion, Lanczos, no upscales).
  Expansion renditions: /mnt/v/output/clean-picker-corpus-2026-07-12/.
- Grid: 32 lossless cells — def e6..e10 ±pal0, wp5 e6 ±pal0, and every
  curated __expert internal-params probe at its non-default-aliasing
  efforts (lloyd e7/e9/e10 ±pal0, ycocg/seeds2 e7/e9/e10, buckets256 +
  props16 e7/e8, threshold30 + maxsamples8192 e7/e10). Old renditions'
  def/wp5 cells reused from the 2026-07-02 fleet sweep after proving
  byte-identical output: ZERO mismatches across 53,964 overlapping
  cells re-encoded locally.
- Timing: per-cell wall ms, single-threaded (RAYON_NUM_THREADS=1),
  process-per-image, 7950X, run via examples/proxy_policy_sweep.rs.
  ~215 cpu-hours local. Raw per-cell TSVs + matrix:
  /mnt/v/output/proxy-policy-sweep-2026-07-12/ (mirrored to Tower).
  Encoded bytes not archived: lossless cells are deterministically
  re-derivable at the recorded commits (drift-checked above), unlike
  fleet sweeps where recovery costs real money.
- Split: origin-id parity (origin_split.py) — train 3,627 / val 2,014 /
  test 1,200 renditions. Menus greedy-built on train, gate/menu variants
  tuned on val, shipped policy evaluated ONCE on test.

## Shipped policy (see src/lossless_verify.rs)
- Gate (one cheap local pixel pass): distinct colors <= 256 OR >= 99%
  near-gray pixels.
- Not gated (~72-80%): single encode mod-e9_lloyd-pal0.
- Gated: also mod-e9_seeds2, mod-e10_lloyd-pal0, mod-e10_maxsamples8192;
  keep smallest.

## Test-split result (evaluated once)
| policy | avg ms | mean oh | p99 oh | max oh | >20% |
|---|---|---|---|---|---|
| single e10_def (old default) | 5677 | 3.84% | 57.4% | 224% | 80 |
| prev shipped (palette->e10/e6 def±pal0) | 6182 | 1.48% | 40.1% | 76% | 16 |
| **B10 shipped** | **4060** | **0.79%** | **8.6%** | 80% | **7** |

34% faster and 1.9x better mean overhead than the previous policy; the
one residual worst-case family (fine-grid synthetic plots, ~80%) is
shared with the previous policy (76%) — a wash, documented, not hidden.

Key discoveries: e9 beats e10 as default (cheaper AND more often
optimal; the effort ladder is non-monotonic); lloyd_max_buckets is a
consistent net win; palette detection OFF is the better default;
maxsamples8192 loses on average but is the oracle on the pathological
low-color family (+30-86% rescued there).

Commits: zenjxl 697ca51, jxl-encoder eeb52735. Host: lilith.

## Confirmation set (added same day, after shipping)
246 additional fresh test-parity (7/9) origins — every remaining unused
one in imazen-26 — 984 renditions, swept on the same 32-cell grid, with
ALL policies FROZEN before this data existed (no selection, menu, or
gate decision ever saw these origins). Files: `testexp_confirmation.tsv`
+ `confirm_testexp.py`. Result (n=984):

| policy | avg ms | mean oh | p99 oh | max oh | >20% |
|---|---|---|---|---|---|
| single e10_def | 7607 | 6.64% | 67.7% | 94% | 129 |
| prev shipped | 8361 | 1.84% | 53.6% | 86% | 19 |
| **B10 shipped** | **5589** | **0.45%** | **3.5%** | **5.7%** | **0** |

Stronger than the original test split on every axis — the residual
~80% worst-case family there does not recur in fresh origins (it is one
specific origin cluster), and the previous policy's weakness (86% max,
19 outliers) replicates on fresh data, confirming it was structural,
not corpus-specific.
