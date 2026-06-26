# zenjxl lossy picker v0.1 (INTERIM — dense-r6, even/odd-by-origin split)

72 KB ZNPR (f16; n_inputs=110, n_outputs=21, n_layers=3). Predicts the lossy
7-cell knob choice; pairs with a top-3 encode-verify for the ≤1% operating point.

- **Split (clean):** canonical even/odd-by-origin — {0,2,4,6,8}=train, {1,3,5}=val,
  {7,9}=test (zenmetrics `scripts/picker/origin_split.py`). No derivative leaks.
- **Held-out numbers** (zensim): val argmin 2.25% / top-3-verify 0.52% (oracle-in-top-3
  84.4%); **TEST (7/9 origins) argmin 2.33% / top-3-verify 0.42%** (85.6%), val→test
  gap +0.08pp — generalizes.
- **INTERIM caveat:** trained on the dense-r6 corpus, which is **train-biased**
  (built from `K500_even` representatives). **Supersede with v0.2 trained on the
  full imazen-26 corpus** (1082/657/418 origins) once the clean re-sweep lands.
  Also missing `output_bounds` (OOD-on-output check is a no-op) — emit per-output
  p01/p99 in the v0.2 bake.
- Provenance + program: zenmetrics `docs/CLEAN_PICKER_PROGRAM.md`.
