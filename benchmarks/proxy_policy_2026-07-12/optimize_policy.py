"""Time+RD policy optimization for the zenjxl lossless image-proxy use case.

Policy = (gate over cheap per-image features) -> ordered candidate menu of
sweep cells; proxy encodes every candidate in the menu and keeps the
smallest. Cost = sum of LOCAL per-cell encode ms; quality = bytes overhead
vs the widest available oracle. Train/val/test by origin-id parity
(origin_split.py, the ONE canonical rule).
"""
import sys
sys.path.insert(0, "/home/lilith/work/zen/zenmetrics/scripts/picker")
import numpy as np
import pandas as pd
from origin_split import split_of

SC = "/tmp/claude-1000/-home-lilith-work-zen-zenmetrics/49ed8dcc-486d-4a67-81d5-adc41676d2c6/scratchpad"

POLICY_CELLS = [
    "mod-e6_def","mod-e6_def-pal0","mod-e7_def","mod-e7_def-pal0","mod-e8_def","mod-e8_def-pal0",
    "mod-e9_def","mod-e9_def-pal0","mod-e10_def","mod-e10_def-pal0","mod-e6_wp5","mod-e6_wp5-pal0",
    "mod-e7_lloyd","mod-e7_lloyd-pal0","mod-e9_lloyd","mod-e9_lloyd-pal0","mod-e10_lloyd","mod-e10_lloyd-pal0",
    "mod-e7_ycocg","mod-e9_ycocg","mod-e10_ycocg","mod-e7_seeds2","mod-e9_seeds2","mod-e10_seeds2",
    "mod-e7_buckets256","mod-e8_buckets256","mod-e7_props16","mod-e8_props16",
    "mod-e7_threshold30","mod-e10_threshold30","mod-e7_maxsamples8192","mod-e10_maxsamples8192",
]

def load_all():
    m = pd.read_parquet(f"{SC}/matrix.parquet")
    oracle = m.groupby("basename")["bytes"].min().rename("oracle_bytes")
    pol = m[m["cell"].isin(POLICY_CELLS)]
    B = pol.pivot_table(index="basename", columns="cell", values="bytes")
    T = pol.pivot_table(index="basename", columns="cell", values="ms")
    complete = B.dropna().index.intersection(T.dropna().index)
    B, T = B.loc[complete], T.loc[complete]
    print(f"complete renditions (all {len(POLICY_CELLS)} policy cells w/ local ms): {len(B)}")

    fo = pd.read_csv(f"{SC}/old_features_qualified_2026-07-12.tsv", sep="\t")
    fo["basename"] = fo["image_path"].str.rsplit("/", n=1).str[-1]
    fn = pd.read_csv(f"{SC}/expansion_features_qualified_2026-07-12.tsv", sep="\t")
    fn["basename"] = fn["image_path"].str.rsplit("/", n=1).str[-1]
    fcols = sorted(set(c for c in fo.columns if "@" in c) & set(c for c in fn.columns if "@" in c))
    feats = pd.concat([fo[["basename"]+fcols], fn[["basename"]+fcols]], ignore_index=True).drop_duplicates("basename").set_index("basename")

    idx = B.index.intersection(feats.index)
    B, T, feats = B.loc[idx], T.loc[idx], feats.loc[idx]
    orc = oracle.loc[idx]
    split = pd.Series([split_of(b) for b in idx], index=idx, name="split")
    print(f"evaluable renditions: {len(idx)}  split: {split.value_counts().to_dict()}")
    return B, T, feats, orc, split, fcols

def menu_stats(B, T, orc, menu):
    best = B[list(menu)].min(axis=1)
    oh = (best / orc - 1) * 100
    ms = T[list(menu)].sum(axis=1)
    return oh, ms

def greedy_menus(B, T, orc, max_k=5, mode="rd"):
    """Nested menu family. mode=rd: each step minimizes mean overhead.
    mode=rd_per_ms: minimizes mean overhead per added ms (time-aware)."""
    menus, menu = [], []
    remaining = list(B.columns)
    while len(menu) < max_k:
        best_c, best_score = None, None
        cur_oh = menu_stats(B, T, orc, menu)[0].mean() if menu else None
        for c in remaining:
            oh, ms = menu_stats(B, T, orc, menu + [c])
            if mode == "rd":
                score = (oh.mean(), ms.mean())
            else:
                gain = (cur_oh - oh.mean()) if menu else (100 - oh.mean())
                score = (-(gain / max(T[c].mean(), 1e-9)), oh.mean())
            if best_score is None or score < best_score:
                best_score, best_c = score, c
        menu.append(best_c); remaining.remove(best_c)
        menus.append(list(menu))
    return menus

def eval_policy(B, T, orc, choose_menu, idx):
    """choose_menu: basename -> menu (list of cells). Returns stats dict."""
    ohs, mss = [], []
    for b in idx:
        menu = choose_menu(b)
        row_b, row_t = B.loc[b, menu], T.loc[b, menu]
        ohs.append((row_b.min() / orc.loc[b] - 1) * 100)
        mss.append(row_t.sum())
    oh = pd.Series(ohs, index=idx); ms = pd.Series(mss, index=idx)
    return dict(ms=ms.mean(), oh_mean=oh.mean(), oh_p99=np.percentile(oh, 99),
                oh_max=oh.max(), n_over20=int((oh > 20).sum()), worst=oh.idxmax())

if __name__ == "__main__":
    B, T, feats, orc, split, fcols = load_all()
    tr, va = split == "train", split == "val"
    Btr, Ttr, otr = B[tr], T[tr], orc[tr]

    print("\n=== single-cell baselines (train) ===")
    rows = []
    for c in B.columns:
        oh, ms = menu_stats(Btr, Ttr, otr, [c])
        rows.append((c, ms.mean(), oh.mean(), oh.max()))
    base = pd.DataFrame(rows, columns=["cell","ms","oh_mean","oh_max"]).sort_values("oh_mean")
    print(base.head(12).to_string(index=False))

    print("\n=== greedy nested menus (train, rd mode) ===")
    for menu in greedy_menus(Btr, Ttr, otr, 5, "rd"):
        oh, ms = menu_stats(Btr, Ttr, otr, menu)
        print(f"  K={len(menu)} {menu}  ms={ms.mean():.0f}  oh_mean={oh.mean():.3f}%  oh_max={oh.max():.1f}%")
    print("=== greedy nested menus (train, rd-per-ms mode) ===")
    for menu in greedy_menus(Btr, Ttr, otr, 5, "rd_per_ms"):
        oh, ms = menu_stats(Btr, Ttr, otr, menu)
        print(f"  K={len(menu)} {menu}  ms={ms.mean():.0f}  oh_mean={oh.mean():.3f}%  oh_max={oh.max():.1f}%")

# ── Gated-policy search (appended; run via optimize_policy_full.py driver) ──
from sklearn.tree import DecisionTreeClassifier

def gate_col(feats, prefix):
    cols = [c for c in feats.columns if c.startswith(prefix)]
    assert cols, f"no feature col starting {prefix}"
    return cols[0]

def eval_gated(B, T, orc, idx, mask_rich, menu_lean, menu_rich):
    """mask_rich: bool Series over idx -> use rich menu."""
    lean_oh, lean_ms = menu_stats(B.loc[idx], T.loc[idx], orc.loc[idx], menu_lean)
    rich_oh, rich_ms = menu_stats(B.loc[idx], T.loc[idx], orc.loc[idx], menu_rich)
    oh = lean_oh.where(~mask_rich.loc[idx], rich_oh)
    ms = lean_ms.where(~mask_rich.loc[idx], rich_ms)
    return dict(ms=ms.mean(), oh_mean=oh.mean(), oh_p99=float(np.percentile(oh, 99)),
                oh_max=oh.max(), n_over20=int((oh > 20).sum()),
                frac_rich=float(mask_rich.loc[idx].mean()), worst=oh.idxmax())

def fmt(name, s):
    return (f"  {name:52} ms={s['ms']:7.0f} oh_mean={s['oh_mean']:6.3f}% "
            f"p99={s['oh_p99']:5.2f}% max={s['oh_max']:6.1f}% >20%:{s['n_over20']:3d} "
            f"rich%={s.get('frac_rich', 1.0)*100:5.1f}  worst={s['worst']}")

def full_search():
    B, T, feats, orc, split, fcols = load_all()
    tr = split.index[split == "train"]; va = split.index[split == "val"]; te = split.index[split == "test"]
    Btr, Ttr, otr = B.loc[tr], T.loc[tr], orc.loc[tr]

    fam_rd = greedy_menus(Btr, Ttr, otr, 5, "rd")
    fam_ms = greedy_menus(Btr, Ttr, otr, 5, "rd_per_ms")
    print("\ngreedy families (train):")
    for tag, fam in [("rd", fam_rd), ("rd/ms", fam_ms)]:
        for m in fam:
            oh, ms = menu_stats(Btr, Ttr, otr, m)
            print(f"  [{tag}] K={len(m)} {m} ms={ms.mean():.0f} oh={oh.mean():.3f}% max={oh.max():.1f}%")

    pf = gate_col(feats, "palette_fits_in_256")
    mask_palette = feats[pf] >= 0.5

    # Learned gate: does the lean menu leave >1% on the table vs the rich union?
    SHIP_RICH = ["mod-e10_def","mod-e10_def-pal0","mod-e6_def","mod-e6_def-pal0"]
    candidates = {}
    lean_opts = {f"ms-K{len(m)}": m for m in fam_ms[:2]} | {f"rd-K1": fam_rd[0]}
    rich_opts = {f"rd-K{len(m)}": m for m in fam_rd[1:5]} | {"ship4": SHIP_RICH}

    # train learned gates per (lean, rich)
    results = []
    for ln, lm in lean_opts.items():
        for rn, rm in rich_opts.items():
            if set(rm) <= set(lm):
                continue
            lean_oh_tr, _ = menu_stats(Btr, Ttr, otr, lm)
            union_oh_tr, _ = menu_stats(Btr, Ttr, otr, list(dict.fromkeys(lm + rm)))
            y = ((lean_oh_tr - union_oh_tr) > 1.0).astype(int)
            gates = {"always": pd.Series(True, index=feats.index),
                     "palette": mask_palette}
            if y.sum() >= 20:
                t2 = DecisionTreeClassifier(max_depth=2, class_weight={0: 1, 1: 25}, random_state=0)
                t2.fit(feats.loc[tr, fcols], y)
                gates["tree2"] = pd.Series(t2.predict(feats[fcols]).astype(bool), index=feats.index)
            for gn, gm in gates.items():
                s_tr = eval_gated(B, T, orc, tr, gm, lm, rm)
                results.append((f"lean={ln} rich={rn} gate={gn}", lm, rm, gm, s_tr))

    print("\n=== TRAIN shortlist (oh_max<=100, sorted by ms then oh) ===")
    ok = [r for r in results if r[4]["oh_max"] <= 100.0]
    ok.sort(key=lambda r: (r[4]["ms"], r[4]["oh_mean"]))
    for name, lm, rm, gm, s in ok[:14]:
        print(fmt(name, s))

    # Baselines on val + shortlist on val
    print("\n=== VAL: baselines vs shortlist ===")
    ship_gate = mask_palette
    print(fmt("BASELINE single e10_def", eval_gated(B,T,orc,va, pd.Series(False,index=feats.index), ["mod-e10_def"], ["mod-e10_def"])))
    print(fmt("BASELINE single e9_def", eval_gated(B,T,orc,va, pd.Series(False,index=feats.index), ["mod-e9_def"], ["mod-e9_def"])))
    print(fmt("BASELINE SHIPPED (palette->ship4 else e10_def)", eval_gated(B,T,orc,va, ship_gate, ["mod-e10_def"], SHIP_RICH)))
    val_rows = []
    for name, lm, rm, gm, _ in ok[:14]:
        s_va = eval_gated(B, T, orc, va, gm, lm, rm)
        val_rows.append((name, lm, rm, gm, s_va))
        print(fmt(name, s_va))
    return B, T, feats, orc, split, fcols, val_rows, te
