# JXL wrapper ARM audit, 2026-09-06

This wrapper has no independent ARM/arcane/magetypes kernels in `src/`. Encoding delegates to `jxl-encoder`; decoding delegates to `zenjxl-decoder` (imported as `jxl`). Backend performance measurements live in [the encoder audit](https://github.com/imazen/jxl-encoder/tree/main/benchmarks/arm_audit_2026-09-06) and [the decoder audit](https://github.com/imazen/zenjxl-decoder/tree/main/benchmarks/arm_audit_2026-09-06). They are backend measurements, not wrapper throughput measurements.

Validation host: Apple M4 Pro, macOS, Rust 1.98 / LLVM 22. Wrapper source `681021da`; local path patches resolved encoder `10df82e8` and decoder `17df9a31` production code (decoder investigation `5777c5b7` adds only profiling documentation). No target-cpu=native. Commands ran under nice -n19 with four build/Rayon/OMP threads.

- Default encode/decode configuration: 25 tests passed. Most integration binaries contain zero enabled tests under these features. [Full log](zenjxl-wrapper-tests.log).
- `--features zencodec,__expert`: 139 tests passed, including lossless verification, alpha-axis contracts, variant generation and zencodec adapter tests. [Full log](zenjxl-wrapper-feature-tests.log).

Commands: `cargo test --locked -p zenjxl --lib --tests` and `cargo test --locked -p zenjxl --features zencodec,__expert --lib --tests`. Neither run enables every optional feature; this is not full feature-matrix coverage. The default build reports existing dead-code warnings in the encoder dependency.

The RGBA adapter in `src/codec.rs` processes padded input by row when the source stride differs from width; its packed path passes the contiguous pixels directly. No public API or production wrapper code was changed by this audit.

Strict wrapper-library Clippy passed with `--features zencodec,__expert --no-deps -- -D warnings`; [log](clippy.log). Scoped formatting passed. Reproduce the feature-enabled tests with `just arm-wrapper-tests-macos`.
