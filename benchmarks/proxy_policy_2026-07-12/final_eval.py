"""Final: full search on complete data, winner selection on val, ONE test eval."""
import sys
SC = "/tmp/claude-1000/-home-lilith-work-zen-zenmetrics/49ed8dcc-486d-4a67-81d5-adc41676d2c6/scratchpad"
exec(open(f"{SC}/optimize_policy.py").read().replace('if __name__ == "__main__":', 'if False:'))

B, T, feats, orc, split, fcols = load_all()
tr = split.index[split == "train"]; va = split.index[split == "val"]; te = split.index[split == "test"]
Btr, Ttr, otr = B.loc[tr], T.loc[tr], orc.loc[tr]

fam_rd = greedy_menus(Btr, Ttr, otr, 5, "rd")
fam_ms = greedy_menus(Btr, Ttr, otr, 5, "rd_per_ms")
print("greedy families (train):")
for tag, fam in [("rd", fam_rd), ("rd/ms", fam_ms)]:
    for m in fam:
        oh, ms = menu_stats(Btr, Ttr, otr, m)
        print(f"  [{tag}] K={len(m)} {m} ms={ms.mean():.0f} oh={oh.mean():.3f}% max={oh.max():.1f}%")

pf = gate_col(feats, "palette_fits_in_256")
mask_palette = feats[pf] >= 0.5
SHIP_RICH = ["mod-e10_def","mod-e10_def-pal0","mod-e6_def","mod-e6_def-pal0"]
never = pd.Series(False, index=feats.index)

print("\n=== VAL: baselines ===")
print(fmt("single e10_def", eval_gated(B,T,orc,va, never, ["mod-e10_def"], ["mod-e10_def"])))
print(fmt("single e9_def", eval_gated(B,T,orc,va, never, ["mod-e9_def"], ["mod-e9_def"])))
print(fmt("single e9_lloyd-pal0", eval_gated(B,T,orc,va, never, ["mod-e9_lloyd-pal0"], ["mod-e9_lloyd-pal0"])))
print(fmt("SHIPPED (palette->ship4 else e10_def)", eval_gated(B,T,orc,va, mask_palette, ["mod-e10_def"], SHIP_RICH)))

print("\n=== VAL: candidate policies ===")
cands = []
for ln, lm in {"rdK1": fam_rd[0], "msK2": fam_ms[1]}.items():
    for rn, rm in {"rdK2": fam_rd[1], "rdK3": fam_rd[2], "rdK4": fam_rd[3], "rdK5": fam_rd[4]}.items():
        if set(rm) <= set(lm): continue
        s = eval_gated(B, T, orc, va, mask_palette, lm, rm)
        cands.append((f"lean={ln}({lm[0]}..) rich={rn} gate=palette", lm, rm, mask_palette, s))
        print(fmt(cands[-1][0], s))
# also gate=always variants of rd menus (no gate, all images same menu)
for rn, rm in {"rdK2": fam_rd[1], "rdK3": fam_rd[2]}.items():
    s = eval_gated(B, T, orc, va, never, rm, rm)
    cands.append((f"flat {rn}", rm, rm, never, s))
    print(fmt(cands[-1][0], s))

# WINNER: dominate SHIPPED on ms AND oh_mean AND oh_max, then min ms; tiebreak oh_mean
ship_va = eval_gated(B,T,orc,va, mask_palette, ["mod-e10_def"], SHIP_RICH)
dom = [c for c in cands if c[4]["ms"] < ship_va["ms"] and c[4]["oh_mean"] < ship_va["oh_mean"] and c[4]["oh_max"] <= min(100.0, ship_va["oh_max"])]
dom.sort(key=lambda c: (c[4]["oh_mean"], c[4]["ms"]))
print(f"\n{len(dom)} policies dominate SHIPPED on val. Winner = best oh_mean among dominators:")
name, lm, rm, gm, s = dom[0]
print(fmt("WINNER " + name, s))
print(f"  lean menu: {lm}\n  rich menu: {rm}")

print("\n=== TEST (evaluated ONCE) ===")
print(fmt("single e10_def", eval_gated(B,T,orc,te, never, ["mod-e10_def"], ["mod-e10_def"])))
print(fmt("SHIPPED", eval_gated(B,T,orc,te, mask_palette, ["mod-e10_def"], SHIP_RICH)))
print(fmt("WINNER " + name, eval_gated(B,T,orc,te, gm, lm, rm)))

# persist per-rendition results for the benchmarks record
rows = []
for setname, idx in [("train", tr), ("val", va), ("test", te)]:
    for polname, l, r, g in [("shipped", ["mod-e10_def"], SHIP_RICH, mask_palette), ("winner", lm, rm, gm)]:
        lean_oh, lean_ms = menu_stats(B.loc[idx], T.loc[idx], orc.loc[idx], l)
        rich_oh, rich_ms = menu_stats(B.loc[idx], T.loc[idx], orc.loc[idx], r)
        oh = lean_oh.where(~g.loc[idx], rich_oh); ms = lean_ms.where(~g.loc[idx], rich_ms)
        for b in idx:
            rows.append((setname, polname, b, oh.loc[b], ms.loc[b]))
pd.DataFrame(rows, columns=["split","policy","basename","overhead_pct","ms"]).to_csv(f"{SC}/final_policy_rows.tsv", sep="\t", index=False)
print("\npersisted per-rendition rows")
