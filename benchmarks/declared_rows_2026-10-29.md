# JXL declared rows, 2026-10-29

Base `558e5e02`; CI's core checkout `3f4fd14feb9bdfed041e426aaa569b236a201158`
at `.zencodec`. Isolated ARM64 Neoverse-N1 export, outside workspace overrides.
Native dependency pins unchanged. This repository does not track its root lock;
the qualification resolution is preserved in `declared_rows_2026-10-29.lock`.
Copy it to Cargo.lock in the same source export to reproduce this graph.

Command: `run-heavy --mem 3G --jobs 2 -- timeout 900 just declared-rows-check`,
with external CARGO_TARGET_DIR, CARGO_PROFILE_DEV_DEBUG=0 and
CARGO_NET_GIT_FETCH_WITH_CLI=true. Logs retained at
`~/tmp/jxl-declared-rows-20261029/`.

- First baseline attempt stopped before compilation: absent root Cargo.lock with
  --locked. rc101, 46s, peak RSS 0.17GiB. No native test ran.
- Generated one resolution for both baseline and repair. Baseline row control
  fails on Unsupported(RowLevelEncode) at valid begin. rc101, 235s, peak RSS
  0.77GiB, minimum available 13917MiB.
- Repaired run: all six selected tests pass (two row controls, two existing color
  controls, two existing shared-contract controls). Strict Clippy rejects a
  pre-existing nonminimal Boolean expression in color fallback. rc101, 48s,
  peak RSS 0.57GiB, minimum available 14042MiB.
- Final run after an equivalent Boolean simplification: all six tests and strict
  library/regression Clippy pass. rc0, 5s, peak RSS 0.20GiB, minimum available
  14445MiB, peak load 1.17. Existing assertions unchanged; no lint suppression.

The row parity control covers RGB/RGBA/RGBX, padded strips, lossless/lossy,
rejected-begin retry, duplicate begin, mismatched width, missing/excess rows,
one-shot and encode_srgba8 rejection after begin, plus shared row conformance.
Timeout controls require the original TimedOut category at begin/push/finish.
Parity is exact bytes against the same native one-shot path; this is not a new
independent decoder oracle, exhaustive native-mode matrix or performance study.
This adapter retains a full raster and reports AT_FINISH. No proven workspace
bound or checked planning/color admission is added.

Source SHA256: `7df3e42e1ae4a865383fcea48412a6d3c4f4d8d771b079b945a541d117696595`.
Test SHA256: `43f4bf69839a1d152c8824ff938a2db29463bc17dd3eb15bba05a625a3da4b18`.
Lock SHA256: `e5d97a0c0199ac15eabb50bc04d9ac07770437f279161d38ccd789522a4d3841`.
Wrapper resource figures describe validation, not encoder memory or speed.
