# zenjxl Public API Ablation Report

**Date:** 2026-06-11
**Snapshot base commit:** 49ab9db0 (main, prior to current WIP)
**Snapshot counts:** 182 default / 432 all-except-_* (user-stated 181/431 — 1-off likely from snapshot regeneration)
**Mode:** CONSERVATIVE — default KEEP; flag only clear mistakes
**Commit status:** READ-ONLY (active work marker from `claude-zenjxl-variantgen` at 00:01:34Z; jj @ had uncommitted changes to Cargo.toml + examples/e9_diag.rs not authored by this session — marker stale >5min but uncommitted changes present, so no claim made per CLAUDE.md).

**Grep template (external consumers, excluding this repo):**
```
ugrep -rn "<symbol>" /home/lilith/work/ --include="*.rs" \
  --exclude-dir=target --exclude-dir=.jj --exclude-dir=zenjxl
```

---

## Summary

| Scope | Total items | Flagged A | Flagged B | Total flagged | % of total |
|-------|------------|-----------|-----------|---------------|------------|
| Default (182) | 182 | 0 | 3 | 3 | 1.6% |
| All-features delta (250 additional) | 250 | 0 | 1 | 1 | 0.4% |
| **Grand total** | **432** | **0** | **4** | **4** | **0.9%** |

---

## Scan Evidence

### Consumer map (as of 2026-06-11)

| Consumer | What it uses |
|----------|-------------|
| `zenpipe/zencodecs` | `JxlDecoderConfig`, `JxlEncoderConfig`, `JxlDecodeJob`, `JxlEncodeJob`, `GainMapBundle`, `LosslessConfig`, `encode_gray8_lossless`, `encode_rgb8_lossless`, `jpeg_lossy::{JpegRecompressMethod, QualityTarget, recompress_jpeg_lossy_target}`, `calibrated_jxl_quality` (doc comment ref) |
| `imageflow/imageflow_core` | `JxlDecoderConfig`, `JxlEncoderConfig` |
| `zenmetrics` | `JxlEncoderConfig`, `zenjxl::decode` (free fn), `JxlDecodeOutput` (implicit via decode return) |
| `zensim / zensim-picker-prep` | `JxlEncoderConfig`, `zenjxl::decode` |
| `_zensim-pu-panel` (parked) | `JxlEncoderConfig`, `zenjxl::decode` |

**Not consumed externally (zero `zenjxl::` prefixed hits):**
- `zenjxl::encode_bgra8` / `encode_bgra8_lossless`
- `zenjxl::encode_rgba8` / `encode_rgba8_lossless`
- `zenjxl::encode_rgb8` (lossy version) / `encode_gray8` (lossy)
- `zenjxl::decode_with_options` / `decode_with_parallel`
- `zenjxl::probe`
- `zenjxl::is_bare_codestream` / `is_container`
- `zenjxl::calibrated_jxl_quality` / `quality_to_distance` (from `zenjxl::` namespace)
- `zenjxl::append_gain_map_box`
- `zenjxl::PixelLayout`
- `zenjxl::JxlDecodeOutput` / `JxlInfo` / `JxlExtraChannelInfo` / `JxlExtraChannelType` (by name in callers)
- `zenjxl::JxlLimits`
- `zenjxl::GainMapData` struct
- `zenjxl::JxlAnimationFrameDecoder` / `JxlAnimationFrameEncoder` (by name in callers)

---

## Module Tables

### Default features — flagged items

#### Free functions: `decode_with_parallel` and `decode_with_options`

`decode` is used by `zenmetrics` and `zenpipe/zencodecs`. The two more-detailed variants `decode_with_parallel` and `decode_with_options` have zero external callers — callers that need parallelism or stop tokens go through the `JxlDecodeJob` + `JxlDecoderConfig` zencodec-trait path.

| Item | Flag | Rationale |
|------|------|-----------|
| `pub fn zenjxl::decode_with_parallel(…)` | **B** | No external callers. `decode` + the `JxlDecodeJob::with_stop` path cover the use cases. Queued as B (pub(crate) or remove); requires zenmetrics/zenpipe to use `decode` or the job path. |
| `pub fn zenjxl::decode_with_options(…)` | **B** | Same rationale as `decode_with_parallel`. The only caller of `decode_with_options` is `decode_with_parallel` itself (internal). Zero external callers. |

#### Re-exports: `LossyConfig`, `LosslessConfig`, `PixelLayout` (from `jxl_encoder`)

These three types from `jxl_encoder` are re-exported at the `zenjxl` root behind the `encode` feature. `LosslessConfig` is used by `zencodecs/jxl_enc.rs` — it calls `zenjxl::LosslessConfig::default()` and passes it to `encode_gray8_lossless` / `encode_rgb8_lossless`. `LossyConfig` and `PixelLayout` are used only by `jxl_encoder`'s direct dependents (the `jxl-encoder` crate itself; zero `zenjxl::LossyConfig` or `zenjxl::PixelLayout` hits).

| Item | Flag | Rationale |
|------|------|-----------|
| `pub use jxl_encoder::LossyConfig` (re-export) | **B** | Zero external callers via `zenjxl::LossyConfig`. Callers who need `LossyConfig` import it from `jxl_encoder` directly. This re-export only makes sense if it's needed for `encode_rgb8` / `encode_gray8` / `encode_bgra8` (which take `&LossyConfig`) — but those free functions also have zero external callers (see below). Queued B to drop when the lossy free functions are dropped. |
| `pub use jxl_encoder::PixelLayout` (re-export) | **B** | Zero external callers via `zenjxl::PixelLayout`. `PixelLayout` is consumed directly from `jxl_encoder` by its own tests and `gainmap-roundtrip`. This re-export is unnecessary at the `zenjxl` layer. Queued B. |

---

### Default features — borderline items (KEPT)

#### Free functions: `encode_bgra8`, `encode_rgba8`, `encode_rgb8`, `encode_gray8` (lossy); `encode_bgra8_lossless`, `encode_rgba8_lossless`

The `encode_gray8_lossless` and `encode_rgb8_lossless` variants have live consumers (zencodecs). The four lossy free functions (`encode_bgra8`, `encode_rgba8`, `encode_rgb8`, `encode_gray8`) and the two remaining lossless variants (`encode_bgra8_lossless`, `encode_rgba8_lossless`) have zero external callers via `zenjxl::`.

However: these are part of the legacy "simple function" API layer that exists for callers who don't want the `JxlEncoderConfig`/`JxlEncodeJob` trait machinery. The `encode_gray8_lossless` / `encode_rgb8_lossless` usage in zencodecs proves the API is alive and useful. Flagging only `encode_bgra8*` and `encode_rgba8*` (BGRA and RGBA lossless variants) would be premature — they complete the format-coverage set and any new consumer would need them. **KEEP** as a set.

#### `is_bare_codestream`, `is_container`, `append_gain_map_box`, `calibrated_jxl_quality`, `quality_to_distance`

Zero external callers via `zenjxl::`. All five are re-exports from `jxl_encoder`. They are referenced in zencodecs/transcode.rs documentation comments (confirming intent to be available). These are utility functions any JXL-adjacent consumer may need (detecting container vs codestream, appending gain maps, quality mapping). **KEEP** — useful utility surface; the zero-hit scan is because current callers import them from `jxl_encoder` directly or don't need them yet.

#### `zenjxl::probe`

Zero external callers via `zenjxl::probe`. But it's the natural companion to `zenjxl::decode` for format detection without full decode. Callers using `JxlDecoderConfig::probe_header` cover this use case via the trait path; `zenjxl::probe` is the non-trait shortcut. **KEEP** — symmetric with `decode`; would be confusing to remove one.

#### `decode` free function

Used by `zenmetrics` (decode.rs), `zenpipe/zencodecs` (decode.rs gain map path), and `zensim` bench tools. **KEEP**.

---

### All-features delta — flagged items

#### `pub struct zenjxl::GainMapData` — pub field `jhgm_payload`

`GainMapData` is the holder for the gain map payload inside `JxlEncoderConfig`. It has one field: `pub jhgm_payload: Vec<u8>`. External consumers use `JxlEncoderConfig::with_gain_map(jhgm_payload: Vec<u8>)` to set the gain map — they never need to construct or inspect `GainMapData` directly. The struct is only created inside `codec.rs` (`Arc::new(GainMapData { jhgm_payload })`).

| Item | Flag | Rationale |
|------|------|-----------|
| `pub struct zenjxl::GainMapData` — the `jhgm_payload` field | **B** | Make `GainMapData` non-constructible from outside (`pub(crate)` struct or private field + accessor). External callers use `with_gain_map(bytes: Vec<u8>)`. The `gain_map() -> Option<&GainMapData>` accessor is the read path; if anyone needs the raw bytes they can add `pub fn jhgm_payload(&self) -> &[u8]`. Zero external direct field accesses found. Queue for next 0.x minor. |

---

### All-features delta — no flags on remainder

- `jpeg_lossy` module: Used live by `zencodecs/transcode.rs`. KEEP.
- `JxlEncoderConfig` / `JxlDecoderConfig` (zencodec trait impls): Live consumers. KEEP.
- `JxlDecodeJob` / `JxlEncodeJob`: Re-exported by `zencodecs/lib.rs`, used throughout. KEEP.
- `JxlAnimationFrameDecoder` / `JxlAnimationFrameEncoder`: Animation codec surface; zencodec contracts. KEEP.
- `JxlDecoder<'a>` / `JxlEncoder`: zencodec trait structs. KEEP.
- `JxlDecodeOutput` / `JxlInfo` / `JxlExtraChannelInfo` / `JxlExtraChannelType`: Return types from `decode` and `probe`. No external name-usage hits, but callers receive them through the return type of live APIs. KEEP.
- `JxlLimits`: Parameter type for legacy `decode` / `decode_with_*`. Used implicitly (callers pass `None` today). KEEP.
- `ValidationError` / `JxlError`: error types. KEEP.
- `__expert` items (`LossyInternalParams`, `LosslessInternalParams`, etc.): private by convention, feature-gated. KEEP.

---

## Top-3 Findings

1. **`decode_with_options` / `decode_with_parallel`** (B): Both free functions have zero external callers. `decode_with_options` is only called by `decode_with_parallel` internally. The zencodec-trait path (`JxlDecodeJob`) is the externally-used way to configure parallelism and stop tokens. Queue as B for next minor.

2. **`pub use jxl_encoder::LossyConfig` and `PixelLayout`** (B): Re-exports with zero external `zenjxl::` prefixed usage. `LossyConfig` only makes sense alongside the lossy free functions (`encode_rgb8` etc.); if those are ever deprecated, these re-exports go with them. `PixelLayout` is a direct `jxl_encoder` type not needed at the `zenjxl` layer. Queue as B.

3. **`GainMapData::jhgm_payload` pub field** (B, all-features): Struct is created only inside `codec.rs`; external callers use `with_gain_map(bytes)`. The pub field is an accidental leak. Privatize with an accessor.

---

**Note on commit:** This report was NOT committed to the zenjxl repo. The repo had a live work marker from `claude-zenjxl-variantgen` and uncommitted changes to Cargo.toml + examples/e9_diag.rs (not authored by this session). Report saved to `/mnt/v/output/api-ablation/zenjxl--zenjxl.md` only. To commit: claim repo after marker clears + @ is clean, copy from block storage, commit under a fresh jj change.
