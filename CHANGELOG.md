# Changelog

## Unreleased

### Added
- `parallel` feature forwards to `jxl-encoder/parallel`, enabling rayon-based
  per-frame parallelism inside the encoder (c3c1d1e). Previously, callers
  could only parallelize across encode calls — now a single high-effort
  encode can saturate multiple cores. Thanks to the `jxl-encoder` crate for
  providing the underlying parallelism.
- Accept `RGBX8_SRGB` and `BGRX8_SRGB` descriptors in `supported_descriptors()`
  and encode dispatch, stripping the undefined padding byte and routing as
  `PixelLayout::Rgb8` (b384cbd). Applied across `Encoder::encode`, `push_rows`,
  and `AnimationFrameEncoder::push_frame`; round-trips verified against the
  `zenjxl-decoder` path.
- Surface JXL `ToneMapping.intensity_target` as zencodec
  `ContentLightLevel.max_content_light_level` for HDR-aware downstream code
  (04556d7, #3). Only reports values above the 255-nit SDR default; leaves
  MaxFALL at 0 since JXL has no equivalent signal.
- Set `ColorAuthority::Cicp` when JXL carries enum (CICP) color encoding with
  `want_icc=false`, so CMS code trusts the structured signaling over the
  synthesized ICC copy (e03c185, #2).

### Changed
- Migrate `ThreadingPolicy` plumbing to the `is_parallel()` helper from
  `zencodec` 0.1.18 (06b8eb3). Uses `Sequential`/`Parallel` in place of the
  deprecated `SingleThread`/`Unlimited` variants.
- Bump `zencodec` to 0.1.13, picking up JP2/DNG/RAW/SVG format detection and
  the `max_total_pixels` resource limit (6783447).

### Fixed
- Update `policy_to_threads` doc comment to match `jxl-encoder`'s ambient
  rayon pool semantics: `threads=0` means "use the ambient pool" rather than
  "create a default pool" (778b192). Simplified the `Balanced`/`Unlimited`
  match arms accordingly.

### Documentation
- README: add the `parallel` feature row to the feature table, drop the
  stale `zennode`/`EncodeJxl`/`DecodeJxl` references, and switch shields.io
  badges to `?style=flat-square` for repo-wide consistency (1caed1d).

### Internal
- Gitignore tooling noise (`.superwork/`, `.claude/`, `.zenbench/`,
  `copter-report/`, profraw/profdata, fuzz logs, `Cargo.toml` backups) and
  exclude dev-only files (`CLAUDE.md`, `CONTEXT-HANDOFF.md`, `.github/`,
  `justfile`, `fuzz/`) from the published crate package (31a1a6a).
- `cargo fmt` pass (c4994cf).

## 0.1.1 (2026-04-01)

- Bump `jxl-encoder` 0.1.4 -> 0.2.0, `zenjxl-decoder` 0.3.4 -> 0.3.5
- Bump `zencodec` 0.1.11 -> 0.1.12 (fixes `parse_iso21496_fmt` API change)
- Re-enable 13 tests previously blocked by zenjxl-decoder 0.3.4 panic

## 0.1.0 (2026-04-01)

Initial release.

- JPEG XL encoding via `jxl-encoder` 0.2.0 with effort/quality/distance controls
- JPEG XL decoding via `zenjxl-decoder` 0.3.5 with streaming support
- `zencodec` trait integration (feature-gated)
- Gain map (HDR) support with ISO 21496-1 metadata parsing
- Probe API for extracting JXL image info without full decode
- Butteraugli-loop encoding (feature-gated)
- Multi-threaded decoding (feature-gated)
