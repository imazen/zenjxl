"""Assemble the master per-(rendition, cell) matrix for proxy-policy optimization.

Sources:
  A. old parquet  : bytes for old renditions x 120 old-grid cells (byte-identical
                    to local binary, verified) -- ORACLE widening only, no timing.
  B. cells/       : local sweep pass 1 -- new probes on old renditions (20 cells),
                    full 32 cells on new renditions. bytes + LOCAL ms.
  C. cells_base/  : local sweep pass 2 -- 12 base cells on old renditions.
                    bytes (cross-check vs A) + LOCAL ms.
Outputs: matrix.parquet (long: basename, cell, bytes, ms_local, src) and
         crosscheck report (A vs C byte equality).
"""
import glob, sys
import pandas as pd, pyarrow.parquet as pq

SC = "/tmp/claude-1000/-home-lilith-work-zen-zenmetrics/49ed8dcc-486d-4a67-81d5-adc41676d2c6/scratchpad"
OUT = "/mnt/v/output/proxy-policy-sweep-2026-07-12"

def read_cells(d, src):
    rows = []
    for f in glob.glob(f"{d}/*.tsv"):
        try:
            rows.append(pd.read_csv(f, sep="\t", names=["basename","cell","bytes","ms"]))
        except Exception as e:
            print(f"WARN unreadable {f}: {e}", file=sys.stderr)
    if not rows:
        return pd.DataFrame(columns=["basename","cell","bytes","ms"])
    df = pd.concat(rows, ignore_index=True)
    df["src"] = src
    return df

local1 = read_cells(f"{OUT}/cells", "local_p1")
local2 = read_cells(f"{OUT}/cells_base", "local_p2")
print(f"pass1: {len(local1)} cells over {local1['basename'].nunique()} images")
print(f"pass2: {len(local2)} cells over {local2['basename'].nunique()} images")

old = pq.read_table('/mnt/v/zen/zensim-training/2026-07-02-jxl-modular/zenjxl_lossless_pareto_2026-07-03.parquet',
                    columns=['image_path','bytes','config_name']).to_pandas()
old["basename"] = old["image_path"].str.rsplit("/", n=1).str[-1]
old = old.rename(columns={"config_name":"cell"})[["basename","cell","bytes"]]
old["ms"] = float("nan")
old["src"] = "hetzner_old"
print(f"old parquet: {len(old)} cells over {old['basename'].nunique()} images")

# Cross-check: pass2 bytes vs old parquet bytes on identical (basename, cell)
if len(local2):
    xc = local2.merge(old[["basename","cell","bytes"]], on=["basename","cell"], suffixes=("_local","_old"))
    mism = xc[xc["bytes_local"] != xc["bytes_old"]]
    print(f"CROSSCHECK: {len(xc)} overlapping cells, {len(mism)} byte mismatches")
    if len(mism):
        print(mism.head(20).to_string())

matrix = pd.concat([local1, local2, old], ignore_index=True)
# Prefer local rows on duplicates (they carry ms); keep hetzner only where no local row.
matrix["pref"] = (matrix["src"] == "hetzner_old").astype(int)
matrix = matrix.sort_values("pref").drop_duplicates(subset=["basename","cell"], keep="first").drop(columns="pref")
matrix.to_parquet(f"{SC}/matrix.parquet")
print(f"matrix: {len(matrix)} rows, {matrix['basename'].nunique()} images, {matrix['cell'].nunique()} distinct cells")
