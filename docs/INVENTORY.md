# Structural inventory (`DecodeJob::inventory`)

`JxlDecoderConfig` declares `DecodeCapabilities::inventory()`. The walker is
`src/inventory.rs`; the contract is imazen/zencodec `docs/inventory.md`.

## What is enumerated

- **Bare codestream:** `FF0A` (header, structure), the codestream (`ImageData`), and bytes after the
  last frame (`Trailing`). The end comes from a header-only `scan_frames_only` pass of
  zenjxl-decoder (frame headers and TOCs, no pixels). If the scan fails (truncated or malformed
  input) the codestream part runs to the end of the input and its `detail` says so.
- **Container:** the 12-byte signature box, then every box with absolute offsets. Size field 0
  (to end of file) and 64-bit sizes are handled. A size-0 `jxlc` is cut at the codestream end found
  by the scan. Bytes that do not frame as a box after the codestream are `Trailing`; before it,
  `Malformed`.
- **Dispositions follow the one-shot `Decode` path.** Exif/XMP follow the decode policy, `jhgm`
  follows `extract_gain_map` / `GainMapRender`, the last Exif and the last `xml ` box win, and
  `brob` is `Skipped` (zenjxl-decoder is built without its `jpeg` feature; the label is the inner
  box type).

## Boxes after the codestream

zenjxl takes Exif/XMP/`jhgm` right after the pixels are decoded (`decode.rs:935-940`,
`codec.rs:3069-3073`). The decoder parses boxes after the last codestream box only if the
codestream parser polls for more input at its end. Measured (zenjxl-decoder 0.4.0, test
`trailing_box_model_matches_decoder`): with a single `jxlc` they are read for codestreams up to and
including 4096 bytes and missed from 4097 bytes on, whatever the image content. With `jxlp`
codestreams the outcome depends on how the parts align (1000+2938 bytes read them, 3900+38 did not),
so the inventory calls them `Skipped` and says so. When the decoder bug is fixed, the model test
fails and this section and `TRAILING_POLL_LIMIT` change.

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
