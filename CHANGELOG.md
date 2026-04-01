# Changelog

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
