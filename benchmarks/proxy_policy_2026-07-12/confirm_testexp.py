"""FROZEN-policy confirmation on the 2026-07-12 test-expansion corpus.

246 fresh test-parity (7/9) origins / 984 renditions that NO selection,
menu construction, or gate tuning ever saw. The shipped policy and all
baselines are evaluated exactly as frozen in src/lossless_verify.rs
(commit deabeb7e). Oracle = min over the same 32-cell grid the main
sweep used (the old 120-cell widening isn't available for these
renditions; the main analysis measured that widening changes the oracle
by only ~0.075% mean, see benchmarks README).
"""
import glob, sys
sys.path.insert(0, "/home/lilith/work/zen/zenmetrics/scripts/picker")
import numpy as np
import pandas as pd

SC = "/tmp/claude-1000/-home-lilith-work-zen-zenmetrics/49ed8dcc-486d-4a67-81d5-adc41676d2c6/scratchpad"
OUT = "/mnt/v/output/proxy-policy-sweep-2026-07-12"

rows = [pd.read_csv(f, sep="\t", names=["basename","cell","bytes","ms"])
        for f in glob.glob(f"{OUT}/cells_testexp/*.tsv")]
m = pd.concat(rows, ignore_index=True)
B = m.pivot_table(index="basename", columns="cell", values="bytes")
T = m.pivot_table(index="basename", columns="cell", values="ms")
complete = B.dropna().index.intersection(T.dropna().index)
B, T = B.loc[complete], T.loc[complete]
orc = B.min(axis=1)
print(f"test-expansion: {len(B)} complete renditions, {B.shape[1]} cells")

feats = pd.read_csv(f"{SC}/testexp_features_qualified_2026-07-12.tsv", sep="\t")
feats["basename"] = feats["image_path"].str.rsplit("/", n=1).str[-1]
feats = feats.drop_duplicates("basename").set_index("basename")
idx = B.index.intersection(feats.index)
B, T, orc, feats = B.loc[idx], T.loc[idx], orc.loc[idx], feats.loc[idx]
print(f"with features: {len(idx)}")

fits = feats[[c for c in feats.columns if c.startswith("palette_fits_in_256")][0]] >= 0.5
gs = feats[[c for c in feats.columns if c.startswith("grayscale_score")][0]]
gate_b10 = fits | (gs >= 0.99)
never = pd.Series(False, index=idx)

def menu_stats(menu):
    best = B[list(menu)].min(axis=1)
    return (best / orc - 1) * 100, T[list(menu)].sum(axis=1)

def eval_gated(mask, lm, rm):
    lo, lms = menu_stats(lm); ro, rms = menu_stats(rm)
    oh = lo.where(~mask, ro); ms = lms.where(~mask, rms)
    return dict(ms=ms.mean(), oh_mean=oh.mean(), oh_p99=float(np.percentile(oh,99)),
                oh_max=oh.max(), n_over20=int((oh>20).sum()), frac_rich=float(mask.mean()),
                worst=oh.idxmax())

def fmt(name, s):
    return (f"  {name:44} ms={s['ms']:7.0f} oh_mean={s['oh_mean']:6.3f}% "
            f"p99={s['oh_p99']:5.2f}% max={s['oh_max']:6.1f}% >20%:{s['n_over20']:3d} "
            f"rich%={s['frac_rich']*100:5.1f}  worst={s['worst']}")

LM = ["mod-e9_lloyd-pal0"]
RM = ["mod-e9_lloyd-pal0","mod-e9_seeds2","mod-e10_lloyd-pal0","mod-e10_maxsamples8192"]
SHIP_RICH = ["mod-e10_def","mod-e10_def-pal0","mod-e6_def","mod-e6_def-pal0"]
print("\n=== CONFIRMATION SET (fresh 246 test-parity origins, frozen policies) ===")
res = {}
for name, mask, l, r in [
    ("single_e10_def", never, ["mod-e10_def"], ["mod-e10_def"]),
    ("prev_shipped", fits, ["mod-e10_def"], SHIP_RICH),
    ("B10_shipped", gate_b10, LM, RM),
]:
    res[name] = eval_gated(mask, l, r)
    print(fmt(name, res[name]))

pd.DataFrame([dict(policy=k, n=len(idx), avg_encode_ms=round(v["ms"],1),
                   oh_mean_pct=round(v["oh_mean"],4), oh_p99_pct=round(v["oh_p99"],3),
                   oh_max_pct=round(v["oh_max"],2), n_over20pct=v["n_over20"],
                   rich_frac=round(v["frac_rich"],4), worst_image=v["worst"]) for k,v in res.items()]
) .to_csv(f"{SC}/testexp_confirmation.tsv", sep="\t", index=False)
print("\nwrote testexp_confirmation.tsv")
