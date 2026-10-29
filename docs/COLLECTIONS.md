# JPEG XL collections

Emitted auxiliary planes now explicitly carry
`zencodec::image::AuxiliaryGrid::ColorSamples`: their sample centers match the
reconstructed color plane. Source `dim_shift` is retained separately and does
not describe the emitted grid. Shared adoption validates dimensions, and the
declaration survives retained owners and color replacement. This does not
authorize auxiliary color conversion or establish a general resampling method.
The [qualification receipt](../benchmarks/collection_auxiliary_registration_2026-10-27.json)
records native owner/replacement checks, the shared dependency identity, existing
alpha/HDR/JPEG/lossless regressions and WASI/i686 compilation.

The optional `collections` feature implements the shared image-collection
contract from [zencodec PR168](https://github.com/imazen/zencodec/pull/168).
This is review code, not a released API. It uses the qualified native selection
and F32 layer path from [decoder PR68](https://github.com/imazen/zenjxl-decoder/pull/68)
and explicit CMYK polarity from [zenpixels PR93](https://github.com/imazen/zenpixels/pull/93).

The root integration build also needs zencodec revision
`3f4fd14feb9bdfed041e426aaa569b236a201158` checked out at `.zencodec`.
Its root patches replace both registry and Git dependencies with that checkout.
Native build dependencies now use exact Git pins rather than floating sibling
checkouts. Decoder test resources remain a separate CI checkout.
This keeps HDR reconstruction, JPEG metadata and the collection adapter on one
shared type identity without changing codec math. `just contract-identity`
checks the resolved graph. Cargo applies patches only at the consuming workspace
root, so another application must carry its own temporary overrides until these
dependencies are released. The standalone collection consumer has its own pins
and does not need this checkout.
The [earlier integration receipt](../benchmarks/collection_integration_2026-10-26.json)
records strict all-feature lint and the existing gain-map, JPEG-lossy, lossless
round-trip and alpha-mode regression results with its recorded graph. The
[current output-storage receipt](../benchmarks/collection_output_storage_2026-10-26.json)
records the earlier codec-configured path. The [operation-level receipt](../benchmarks/collection_operation_storage_2026-10-26.json)
retains those controls while moving storage into the shared request, and checks
independent pools through one registry plus workspace-limit refusal.

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

To bound retained output payloads, share a `zencodec::storage::StorageBudget`
with `OpenRequest::with_output_storage(storage.clone())` when opening. Each selected
decode admits color and auxiliary buffers, including padding, before native
reconstruction writes them. F32 backing storage is word-aligned in both
allocation modes; mapping does not rely on incidental byte-vector alignment.
The returned `ImageOutput` owns those same buffers;
its last clone releases the charge even if the registry and collection have
already dropped. Cloned requests and operations explicitly given the same pool
share the allowance. Separate requests can use independent pools through one
immutable registry; no codec-specific storage setter or capability query is needed. A `StorageError` is recoverable once enough held images drop;
the adapter does not retry with unaccounted storage.

Replacing packed color now retains JXL auxiliary allocations independently.
`ImageOutput::replace_packed_color` calls the optional owner mechanism; the
consumer needs no additional coordination. The old native color allocation
releases after the last original image clone drops, while auxiliary addresses,
samples and charges remain unchanged. The replacement does not copy auxiliary
planes. Other backends can keep inseparable storage through the same API. Both
source and destination still coexist during conversion, and workspace admission
remains separate.

This setting has a narrow scope: output payloads. It does not admit native
reference/work buffers, source bytes, discovery, profiles, metadata or small
owner allocations, and therefore does not satisfy `max_memory_bytes`. Default
allocation remains available when no output allowance is configured. Both paths
use the same interpretation and publication policy, without a second resolver.

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
- `max_input_bytes` is checked against the actual immutable resident span before
  native parsing. Length hints neither admit an oversized slice nor reject an
  admitted one. This bounds input accepted by the parser; the caller already
  owns that storage, so it is not a memory-admission guarantee.
  [The input-limit receipt](../benchmarks/collection_input_limit_2026-10-26.json)
  covers exact boundaries in both views, misleading hints, typed refusals,
  unsupported mixed requests and unchanged native samples.
- Other nontrivial shared decode resource caps and all `DecodePolicy` flags
  remain unqualified and fail before source I/O. No-op defaults are accepted;
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

## Optional-Pulse rollout validation (2026-10-29)

The standalone all-feature dependency graph resolves exactly one identity each
for zencodec, zenmeta, zencodec-testkit, zenpixels and zenpixels-convert. Shared
metadata and testkit dependencies use version edges so a consuming root can
select that same family. Native dependencies use exact pins; the core checkout
is 3f4fd14f and the paired pixel revision is f23ac753.

The collection-capable decoder remains fe819360. Substituting the ordinary
application pin ce1a28b4 fails compilation: frame/layer inspection, extra-channel
details and frame-header bounds are missing. Keep collection qualification
separate from a successful still-image application build.

With the collection feature enabled, six metadata rewrite tests and three shared
contract tests pass, as does strict Clippy for those tests and the library.
The stopped rewrite fixture now passes Some(pulse), preserving its refusal
assertion. Run `just release-contract-check` from an isolated checkout with the
pinned `.zencodec` directory. Wrapper receipt: rc0, 16 seconds, peak RSS 0.56 GiB,
minimum available 13454 MiB. These are build/test resource figures, not image
processing peak-memory measurements. Independent collection tests, the full
feature matrix and platform qualification have separate receipts.

The independent collection consumer now passes all 15 controls, including every
observed cancellation/publication seam, last-clone storage ownership, unchanged
samples across views and refusal of unproven workspace requirements. Adapter
Clippy passes in that graph (rc0, 47 seconds, peak RSS 0.70 GiB, minimum available
13796 MiB). All-feature/all-target compilation also passes (rc0, 6 seconds,
peak RSS 0.63 GiB, minimum available 13891 MiB). JPEG-lossy explicitly enables
zenjpeg metadata for quality detection; the sole added dependency is the XML
parser required by that feature. The complete runtime/platform matrix is not
implied by these focused checks.

The full runtime matrix exposed a dependency regression that focused metadata
checks missed: encoder 3c1be817 emits a complete codestream before animation
metadata, failing the existing required header/Exif/XMP/frame box order. The
candidate now pins the current-main/animation-stack integration f42a5932
([encoder PR139](https://github.com/imazen/jxl-encoder/pull/139)). Its 92 consumer
library tests pass, including the unchanged box-order check; the remaining
integration suite and encoder oracle gate are still running/unqualified.

The first cross CI run also found obsolete mounts for removed sibling build
dependencies. Cross now mounts only the retained decoder test resources. No
platform or native test is removed. Independent collection CI passed on Linux,
macOS ARM64 and Windows ARM64 at 0dcc0a7b; this does not qualify the new encoder
pin or the repaired cross configuration.

The complete unoptimized suite reached the final lossless thread-determinism
case but exceeded the 900-second wrapper deadline (rc124, peak RSS 1.14 GiB,
minimum available 13400 MiB). Earlier library, alpha-axis, animation, gain-map,
inventory and orientation suites passed; this is not an overall pass. The test
profile now uses optimization level 1 with debug assertions and overflow checks
explicitly enabled. Fixtures, thread counts, efforts and assertions are unchanged;
the optimized full rerun remains required.

The complete optimized rerun passes: 203 tests, zero failures and the one existing
ignored gain-map doctest, across 25 result groups. Both lossless thread-determinism
cases pass with their original data and assertions. Wrapper receipt: rc0,
199 seconds including compilation, peak RSS 1.22 GiB, minimum available
12448 MiB. This run uses portable encoder Git pin f42a5932 (no encoder path
patch), paired pixels f23ac753 and core 3f4fd14f. The prior timeout remains a
separate failed run; these timings are not a codec-performance comparison.

Strict all-feature/all-target Clippy also passes on the portable graph (rc0,
9 seconds, peak RSS 0.64 GiB, minimum available 13666 MiB).
