# Structural inventory (`DecodeJob::inventory`)

`JxlDecoderConfig` declares `DecodeCapabilities::inventory()`. The walker is
`src/inventory.rs`; the contract is imazen/zencodec `docs/inventory.md`.

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
   unparseable `jhgm`/`jxli`, a box running past the input) fails the decode **unless the last
   codestream box (`jxlc`, or the `jxlp` flagged last) has already been entered**: the decoder
   defers box errors after that point. A failing box is `Malformed`, every earlier
   `Metadata`/`ImageData` claim becomes `Skipped` ("decode fails at offset N"), and later boxes are
   `Skipped`.
2. **Drain:** after the last frame the decoder skips what is left of the current codestream box
   (junk after the codestream) and parses the boxes after it until the end of the input or a
   rejected box (the walk stops; the decode is not affected). Codestream boxes there are
   `Unreferenced`. A one-shot decode of an animation, which stops after its first frame, reaches
   these boxes through a header-only scan, so the same rule holds for it.
3. **Dead:** everything after a stop is `Skipped` with a detail saying where parsing stopped.

A gain-map bundle is checked against the job's render mode: `Components` probes the gain-map
codestream's headers (a decode of a broken one fails), native `ReconstructHdr` also parses the
ISO 21496-1 metadata. A failing bundle is `Malformed` and the decode counts as failed.

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
