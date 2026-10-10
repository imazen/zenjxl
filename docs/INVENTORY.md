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
   unparseable `jhgm`/`jxli`, a box running past the input) fails the decode. That box is
   `Malformed`, every earlier `Metadata`/`ImageData` claim becomes `Skipped` ("decode fails at
   offset N"), and later boxes are `Skipped`.
2. **Drain:** zenjxl polls the box parser once more after the last frame (`decode.rs`, fix for
   zenjxl#22), so boxes after the codestream are read until the end of the input, a rejected box
   (the walk stops; the image has decoded) or the next non-empty codestream box (`Unreferenced`).
3. **Dead:** everything after the stop is `Skipped` with a detail saying where parsing stopped.

Junk after the codestream inside a codestream box is a child `Unreferenced` part. The boxes after
such a box are reported `Skipped` ("not reliably read"): measured on the corpus, the decoder reads
them for some layouts and not others (a 1-byte junk tail reads them for `spot.jxl` and misses them
for `bitdepth_8.jxl`), so no disposition is exact there. The oracle test only requires that the
inventory never claims a read the decode lacks.

A rejection reached in the drain phase can still fail the decode of a small file, because the
decoder also polls trailing boxes while reading the last sections of small codestreams. The detail
of such a box says so.

`brob` and `jbrd` follow the linked decoder build: Cargo unifies the decoder's `jpeg` feature across
the dependency graph, so the walker probes the decoder once with a tiny container (a brotli-wrapped
`Exif` box before the codestream) instead of assuming the feature is off. With it on, `brob` Exif/XMP
is `Metadata` (not decompressed by the inventory, so a corrupt stream would really be dropped).

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
