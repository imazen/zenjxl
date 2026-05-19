# zenjxl picker training data — pointer

The training pareto sweeps + feature parquets from 2026-05-01/04/06
(v04, v05c, v06 zenjxl picker training) **live in block storage**,
not in this repo. They exceed the 30 KB git-commit limit (one TSV is
109 MB; total ~232 MB across 11 files).

## Location

Block storage: `/mnt/v/zen/picker-training/zenjxl-2026-05-04/`

R2 mirror (TODO): `s3://zentrain/picker-training-zenjxl-2026-05-04/`

Tower backup (TODO): `/mnt/tower/output/zensim-archive-2026-05-20/picker-training-zenjxl-2026-05-04/`

## Files + sha256

```
ccff70ac6dae8ca1dc5630eaf5edb08488e65049eca02cb207ff72f5f58d49e9  zenjxl_features_v05c_2026-05-04.tsv          1.2M
4a8bd03ced8689019d9327b4c882f8b268ed47be69e0e854357252c232729e54  zenjxl_lossless_pareto_2026-05-01.parquet    2.3M
020f5f6c2c02d8615ed4c4c621cd362ebd6ff71dda43e664461d65634acf1f21  zenjxl_lossless_pareto_2026-05-01.tsv         29M
9f836166ceec427a804b5b11a22d512386477a87b941161a8878c93eb8cf0fbd  zenjxl_lossy_pareto_2026-05-01.parquet       6.3M
7a7002835ddfcb2eaec2bf5f2426baf15d75f2ed65895b0c17156b97e898c75e  zenjxl_lossy_pareto_2026-05-01.tsv           109M
8e55716aade12e645df228b49ab619a8b4267b78e3b9d6c3f60067dcb2f55530  zenjxl_pareto_2026-05-04_extended.tsv        1.7M
0e18138d8155473620d7be84310197d18365f4621d8261928dc2ad818b984deb  zenjxl_pareto_v04_2026-05-04.tsv             1.7M
d1d1a0f9ffb380f0d46a1e5df7b3e72dd12200d9caec6909db59e5bff4e81f93  zenjxl_pareto_v04_2026-05-04_adapted.tsv     1.3M
11b57de3b14284b4bbc2b8a955aa4c4f2eed57961bb08a00c0949b067d199e88  zenjxl_pareto_v05c_2026-05-04.tsv             45M
69174855c7d7cb45fce1c75b7fe7164d2f6a81159b26c9612f1fadfe25a5e18e  zenjxl_pareto_v05c_2026-05-04_adapted.tsv     31M
8fe6b07ca4e50c6ad4d1261fa135b91a59073b21e33b1a87db30eb463d0ea67e  zenjxl_pareto_v06_2026-05-06_adapted.tsv     3.3M
```

## Provenance

- Training configs: `~/work/zen/zenanalyze/zentrain/examples/zenjxl_picker_config_v04*.py`
- Sweep runs: pareto generation dates 2026-05-01 / 2026-05-04 / 2026-05-06
- Codec: zenjxl (libjxl-backed)
- Schema: pareto rows + feature parquets per the picker-training canonical format

## Why moved here

Per `~/.claude/CLAUDE.md` "ML Data Pipeline Discipline" §6 + the global
"NEVER commit images or large files >30kb without explicit user
confirmation" rule, training data lives in block storage. This pointer
documents where the bytes went.

See `~/work/zen/_ml-inventory-2026-05-20/00-MASTER-SYNTHESIS.md` for
the full canonical-data layout.
