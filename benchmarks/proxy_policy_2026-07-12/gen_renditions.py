"""Generate 4 aspect-preserved Lanczos renditions (maxdim 64/192/512/1024)
per selected expansion origin, into the 2026-07-12 corpus dir, plus a
per-rendition manifest (origin sha256 pass-through, split, class, source,
rendition path) for extract_features_for_picker."""
import hashlib, os, sys
from concurrent.futures import ProcessPoolExecutor
import pandas as pd
from PIL import Image

SC = "/tmp/claude-1000/-home-lilith-work-zen-zenmetrics/49ed8dcc-486d-4a67-81d5-adc41676d2c6/scratchpad"
OUT = "/mnt/v/output/clean-picker-corpus-2026-07-12"
SIZES = [64, 192, 512, 1024]
Image.MAX_IMAGE_PIXELS = 300_000_000

os.makedirs(OUT, exist_ok=True)
sel = pd.read_csv(f"{SC}/expansion_selection.tsv", sep="\t", dtype={"id": str})

def process(row):
    oid, src = row["id"], row["path"]
    try:
        sha = hashlib.sha256(open(src, "rb").read()).hexdigest()
        img = Image.open(src).convert("RGB")
        w0, h0 = img.size
        rows = []
        for m in SIZES:
            if max(w0, h0) <= m and m != SIZES[-1]:
                continue  # skip upscales (keep at least the largest slot as-is cap)
            scale = m / max(w0, h0)
            w, h = max(1, round(w0 * scale)), max(1, round(h0 * scale))
            if scale >= 1.0:
                w, h = w0, h0  # never upscale
            name = f"o_{oid}.png.scale{w}x{h}.png"
            dst = f"{OUT}/{name}"
            if not os.path.exists(dst):
                img.resize((w, h), Image.LANCZOS).save(dst, optimize=False)
            rows.append((sha, row["split"], row["content_class"], f"o_{oid}.png", dst))
        return rows
    except Exception as e:
        print(f"FAIL {oid}: {e}", file=sys.stderr)
        return []

all_rows = []
with ProcessPoolExecutor(max_workers=14) as ex:
    for i, rows in enumerate(ex.map(process, [r for _, r in sel.iterrows()])):
        all_rows.extend(rows)
        if (i + 1) % 100 == 0:
            print(f"{i+1}/{len(sel)} origins", flush=True)

mf = pd.DataFrame(all_rows, columns=["sha256", "split", "content_class", "source", "path"])
mf.to_csv(f"{OUT}/_features_manifest.tsv", sep="\t", index=False)
print(f"done: {len(mf)} renditions from {len(sel)} origins -> {OUT}")
