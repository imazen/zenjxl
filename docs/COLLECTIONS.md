# JPEG XL collections

The optional `collections` feature implements the shared image-collection
contract from [zencodec PR168](https://github.com/imazen/zencodec/pull/168).
This is review code, not a released API. It uses the qualified native selection
and F32 layer path from [decoder PR68](https://github.com/imazen/zenjxl-decoder/pull/68)
and explicit CMYK polarity from [zenpixels PR93](https://github.com/imazen/zenpixels/pull/93).

The root integration build also needs zencodec revision
`ffedb4f0d4e2bc89fa450e947251945a15d61c18` checked out at `.zencodec`.
Its root patches replace both registry and Git dependencies with that checkout.
This keeps HDR reconstruction, JPEG metadata and the collection adapter on one
shared type identity without changing codec math. `just contract-identity`
checks the resolved graph. Cargo applies patches only at the consuming workspace
root, so another application must carry its own temporary overrides until these
dependencies are released. The standalone collection consumer has its own pins
and does not need this checkout.

```rust
use zenjxl::collection::JxlCollectionConfig;
use zencodec::{CodecSet, ImageFormat, StopToken, Unstoppable};
use zencodec::collection::{Composition, ImageRequest, OpenRequest};
use zencodec::operation::PolicySet;
use zencodec::source::{ReadAtShim, SliceReadAt};

# fn read(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
let codecs = CodecSet::new()
    .with_collection(JxlCollectionConfig::new(64, 256));
let mut source = ReadAtShim(SliceReadAt::new(bytes));
let mut images = codecs.open_collection(
    ImageFormat::Jxl,
    &mut source,
    OpenRequest::new(Composition::Uncomposited, 512),
    PolicySet::new(),
    StopToken::new(Unstoppable),
)?;
let image = images.first_image(&ImageRequest::new())?;
# Ok(())
# }
```

The two codec allowances bound retained identities and native frame-header work.
Hidden frames consume header work even when there is one visible presentation.
Each independent selected decode gets its own native decoder and header bound.
The shared allowance in `OpenRequest` bounds discovery calls, including cached
calls. None is a byte budget or a guarantee about repeated decode cost.

To browse before decoding, keep the opened operation, create an `images()`
cursor, call `next()` and then `describe(&id)`. Descriptions retain stored
geometry, orientation and raw-layer placement without reconstructing pixels.
They remain available after discovery reaches the end. Later decode still
checks requirements and runs publication policy on the exact retained output.

`Composited` returns visible presentations and renders spot colors. `Uncomposited`
returns local raw samples, unrendered spots and native layer facts. It replays
dependencies from the start for selection. Both preserve stored orientation.
Raw-layer reference slots are local to the opened source, not collection IDs.
An identity or cursor from another opened operation is rejected.

`ImageOutput` owns its planes and actual output ICC interpretation. F32 samples
are strided. CMYK uses `PixelFormat::CmykF32Inverted`; conversion must understand
that polarity or refuse. `JxlSourceFacts` and `JxlRawLayer` are available through
`image.interpretation().source_details.get::<T>()`. They may contain names and
other identifying information. Retaining these facts is not permission to pass
opaque metadata to an encoder, and is not a complete PII scan.

Current limits are explicit:

- Only resident input is accepted. A positioned source is not silently staged.
- Regions and children return unsupported errors, never a full-image fallback
  or successful empty result.
- No nontrivial shared decode resource cap or `DecodePolicy` flag is qualified
  on this route. They fail before source I/O. No-op defaults are accepted;
  `max_output_bytes` is encode-only and does not apply to opening a decoder.
- Native decoder defaults remain in force, but do not jointly bound retained
  discovery state, replay workspace and independently retained image owners.
- Source inspection does not enumerate all metadata, gain-map reconstruction,
  layer roles or every color-conversion route.

Use `zencodec::find_cause::<CollectionOpenError>(&error)` to inspect unhandled
settings. The cause retains its `whereat::At` trace. Callback errors also retain
their original type and trace. Safe outer formatting excludes arbitrary payloads.
For cancellation, use `CollectionOperationError::stop_reason()` independently
of its stage: a stop inside publication can retain the `Policy` stage. A stopped
operation is terminal; a normal request refusal does not prevent valid selection.

The [standalone consumer](../validation/collections) builds away from local
workspace patches and pins the same contract versions used by native spatial
qualification. `just collection-validation` runs caller-facing checks and
strict linting; `just collection-portability` checks WASI and i686 compilation.
`just collection-cross-validation` executes the consumer on i686 using `cross`.
Run it from the isolated export with `ZENJXL_COLLECTION_SCRATCH` and `TMPDIR`
pointing at the same disk-backed directory, and `CODEC_CORPUS_CACHE` pointing
at the base cache directory. The latter contains `codec-corpus/v1`. Both paths
are mounted by `validation/collections/Cross.toml`. Running from this nested
workspace avoids a duplicate project mount. The seven consumer cases passed
on i686; [the validation receipt](../benchmarks/collection_portability_2026-10-26.json)
records the exact configuration and limits of resource measurement.
Native/reference cross-path and independent libjxl validation live in
[zencodec's spatial harness](https://github.com/imazen/zencodec/tree/ideal-api/perf/spatial-validation).
No performance improvement is claimed by this packaging change.

The existing [timing/incremental PR20](https://github.com/imazen/zenjxl/pull/20)
and [inventory-behavior PR30](https://github.com/imazen/zenjxl/pull/30) cover
different paths. This collection implementation does not merge or replace those
changes; integration still has to preserve their contracts.
