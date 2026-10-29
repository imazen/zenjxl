# JPEG XL declared rows

The shared adapter's row path buffers a full raw raster and returns
`OutputTiming::AT_FINISH`. It is not a low-memory streaming encoder.

```rust,ignore
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
let mut encoder = config.job().encoder()?;
encoder.begin(width, height, descriptor)?;
for strip in strips {
    encoder.push_rows(strip)?;
}
let encoded = encoder.finish()?;
```

The declaration is retained. Begin checks nonempty geometry, supported input,
configured dimensions and retained Pulse before changing state. Refusal permits
retry; after successful admission, another begin, one-shot encode, or the
specialized `encode_srgba8` path refuses. Strips must retain width and descriptor,
cannot exceed the declared height, and finish requires all rows. Legacy row
input without begin remains supported, with consistent-strip checks.

Rows are accumulated fallibly. Configured dimensions and raw payload admission
precede conversion/append, but vector capacity, conversion storage and native
workspace are not a proven whole-operation memory bound. No new workspace
evidence is published. Checked planning/color admission remains a separate native
integration; this repair implements the existing Encoder row protocol.

Retained Pulse is checked at begin, push and finish, preserving its original
reason. Native code remains responsible for checkpoints within encoding. Source
color fallback changes only after a strip is accepted, so a rejected strip cannot
replace the prior interpretation.

Six focused tests and strict Clippy pass on ARM64; the baseline reproduces the
valid-begin refusal. See [the receipt and dependency lock](../benchmarks/declared_rows_2026-10-29.md)
and `just declared-rows-check`. In a clean checkout, provide the documented
`.zencodec` revision and saved lock. No crate release or new public signature is
introduced here.
