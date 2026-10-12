# Structural inventory (`DecodeJob::inventory`)

With the default-off `inventory` feature, `JxlDecoderConfig` declares
`DecodeCapabilities::inventory()`. Without it the trait operation returns typed
unsupported. Ordinary EXIF/XMP/color decode remains available. The walker is
`src/inventory.rs`; the contract is imazen/zencodec `docs/inventory.md`.

Source: [PR24](https://github.com/imazen/zenjxl/pull/24), `3fa2837f`.
Measurements below are retained from that PR unless explicitly revalidated in
the integration section.

## What is enumerated

- **Bare codestream:** `FF0A` (header, structure), the codestream (`ImageData`), and bytes after the
  last frame (`Trailing`). The end comes from the header-only scan. If the scan fails (truncated or
  malformed input) the codestream part runs to the end of the input and its `detail` says so.
- **Container:** the 12-byte signature box, then every box with absolute offsets. Size field 0
  (to end of file) and 64-bit sizes are handled. A 64-bit size of 0 also means "to the end of the input".
  A codestream box that holds the codestream end is split: an `ImageData` child up to the end and
  an `Unreferenced` child for the rest. Codestream boxes after the end are `Unreferenced`. Bytes
  that do not frame as a box after the codestream are `Trailing`; before it, `Malformed`.
- **Dispositions follow the one-shot `Decode` path.** Exif/XMP follow the decode policy, `jhgm`
  follows `extract_gain_map` / `GainMapRender`, the last Exif and the last `xml ` box win, and
  `brob` is `Skipped` or `Metadata` by decoder build (the label is the inner box type). `jhgm` carries a
  detail: only the gain-map codestream and the parsed parameters reach the caller.

## Reading model and boxes after the codestream

The walker mirrors zenjxl-decoder's box parser (`box_parser.rs`) and puts each box in one of three
phases, using the codestream length from a header-only scan (`JxlDecoder::codestream_length()` after
`scan_frames_only`, which allocates no per-frame pixel buffers and honours the job's
`max_memory_bytes`):

1. **Main:** every box up to and including the codestream box that holds the last codestream byte.
   A rejected box (second `ftyp`, `jxlc` after `jxlp`, bad `jxlp` order, size-0 `jhgm`/`jxli`,
   unparseable `jhgm`/`jxli`, a box running past the input) fails the decode. That box is
   `Malformed`, every earlier `Metadata`/`ImageData` claim becomes `Skipped` ("decode fails at
   offset N"), and later boxes are `Skipped`.
2. **Drain:** zenjxl polls the box parser once more after the last frame (`decode.rs`, zenjxl#22),
   so for a still image the boxes after the codestream are read until the end of the input, a
   rejected box or the next non-empty codestream box (`Unreferenced`).
3. **Dead:** everything after a stop is `Skipped` with a detail saying where parsing stopped.

Not exact, reported as not read (under-claims, never over-claims; `trailing_boxes_match_real_decode`
asserts that direction):

- **Animations.** A one-shot decode stops after the first frame; boxes after the codestream are
  `Skipped` ("only AnimationFrameDecoder surfaces them; the one-shot decode reads them only when its
  pixel pass happens to reach them"). Small animations do surface them: 20 of 356 wrapped corpus
  animations of 0.8-3.9 KB, and zenjxl's own 884 B and 1,316 B animations (zenjxl#25).
- **Junk after the codestream inside the codestream box.** The decoder parses the boxes after it
  only for some layouts (a 1-byte junk tail: read for `spot.jxl`, missed for `bitdepth_8.jxl`;
  zenjxl-decoder#64, section 2).
- **A rejected box after a small codestream** can still fail the decode (the decoder also polls
  trailing boxes while reading the last sections of small codestreams); the box detail says so
  (zenjxl-decoder#64, section 1).
- **Memory:** the decoder's `hf_sections` table is allocated outside the memory tracker during the
  header scan, bounded by `max_pixels` (35 MiB peak measured on a 1,517-byte fuzz input under an
  8 MiB limit; zenjxl-decoder#64, section 3).

A gain-map bundle is checked against the job's render mode: `Components` probes the gain-map
codestream's headers (a decode of a broken one fails), native `ReconstructHdr` also parses the
ISO 21496-1 metadata. A failing bundle is `Malformed` and the decode counts as failed. Failures
past the headers (corrupt gain-map sections) are not modelled.

`brob` and `jbrd` follow the linked decoder build: Cargo unifies the decoder's `jpeg` feature across
the dependency graph, so the walker probes the decoder once with a tiny container (a brotli-wrapped
`Exif` box before the codestream) instead of assuming the feature is off. With it on, `brob` Exif/XMP
is `Metadata` (not decompressed by the inventory, so a corrupt stream would really be dropped).

Memory limits: the job's `max_memory_bytes` is passed to the scan. Only a refused memory budget
becomes a `LimitExceeded` error; every other scan failure (bad ICC, too many pixels, malformed
header) leaves the codestream end "not located" and the inventory still comes back.

## Next chunk: codestream-internal strings

Not walked yet: frame names (including invisible frames), extra-channel names, header extension
bits, the ICC stream and the image-header fields. Plan:

1. Expose from zenjxl-decoder a read-only header walker (`JxlDecoder` already parses
   `FileHeader`, `FrameHeader`, `name`, `extra_channel_info[i].name`, `extensions`): record byte
   ranges as bit offsets rounded to the enclosing section, or return the headers' bit spans.
2. Emit them as `Attribute` children of the codestream part (needs a `body`, so the codestream
   becomes a container: header bits, TOC, per-section `ImageData`).
3. Dispositions: extra-channel names and frame names are `Dropped` (parsed, never surfaced through
   zencodec; extra-channel names are not in `ImageInfo`); extension bits `Dropped`; ICC
   `Metadata(Icc)`.
4. Oracle: `jxlinfo -v` prints extra-channel names; frame names via `jxl-inspect` (`~/work/jxl-inspect`).

## Cross-checks

`just inventory-oracle` runs the corpus sweep (`check_inventory` on every file under
`~/work/codec-corpus/jxl`) and compares the box lists with `jxlinfo -v -v` (type and length of every
box) and `exiftool -v3` (payload offset and length of every box it lists).

## Memory

The walk allocates per recorded part (about 200 bytes each, measured: 2M `jxll` boxes hit the 1 Mi
part cap at 198 MiB peak heap, `tests/inventory_alloc.rs`) and nothing proportional to the image: a
two-frame 2048x2048 animation peaks at 58 KiB. `ResourceLimits::max_memory_bytes` is passed to the
scan, which then returns `LimitExceeded` if it cannot fit.

## Integration validation (2026-10-12)

`just metadata-check` passes the reused inventory/regression/allocation tests,
strict clippy, default-off behavior, and new one-shot timeout cases. The complete
184-file corpus passes `check_inventory`. Independent readers agree on 551 boxes
in 97 containers (jxlinfo), and 454 listed boxes in 184 files (ExifTool).
`just metadata-portability` passes i686 inventory regression/timeout tests and
regenerates the public API snapshot. These results do not cover the codestream
attributes and decoder limitations listed above.

Local recipes run Cargo from the parent directory because this checkout has an
ignored legacy `paths` override that otherwise replaces pinned dependencies.
The ignored override is preserved. CI has no such local configuration.

## Published API audit

The forced patch comparison against crates.io zenjxl 0.2.1 reports 222 checks
passed, one failed, and 31 skipped. The failure is the second tuple field of
`JxlError::LimitExceeded`, already present on main before this integration.
Manual source inspection also finds eight trait associated error types changed
from `At<JxlError>` to `At<CodecError>`; the tool does not detect these. Both
breaks remain in the existing queued-breaking changelog. The local 0.3.0 bump
predates metadata inventory; no additional version bump was made here.

`just metadata-semver` builds both rustdoc JSON files explicitly because the
semver tool's temporary consumer does not inherit root dependency patches.
Its nonzero result is intentional while these documented main-branch breaks
are queued; it is not a compatibility pass.
