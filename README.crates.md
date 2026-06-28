<!-- GENERATED FROM README.md by zenutils gen-readme-crates.sh — DO NOT EDIT. -->

# zenjxl

zenjxl is a JPEG XL encoding and decoding library combining [zenjxl-decoder](https://github.com/imazen/zenjxl-decoder) and [jxl-encoder](https://github.com/imazen/jxl-encoder) with resource limits, cancellation, gain map support, and lossy JPEG → JXL recompression.

zenjxl-decoder is Imazen's fork of [jxl-rs](https://github.com/libjxl/jxl-rs) with additional metadata extraction, gain map parsing, and resource limiting. jxl-encoder is a pure Rust JPEG XL encoder supporting both lossless (modular) and lossy (VarDCT) modes. zenjxl wraps both behind a unified API and provides zencodec integration for use in [zenpipe](https://github.com/imazen/zenpipe) pipelines.

`#![forbid(unsafe_code)]`, `no_std + alloc`, edition 2024.

## Install

```toml
[dependencies]
zenjxl = "0.2.1"
```

To read decoded pixels back out as packed RGBA8 bytes (see the Decode example),
also add the pixel crates and — for cancellation — `almost-enough`:

```toml
zenpixels = "0.2.13"           # PixelBuffer / PixelDescriptor (returned by decode)
zenpixels-convert = "0.2.13"   # the `.to_rgba8()` extension trait
almost-enough = "0.4.4"        # a ready-made cancellation token (optional)
```

## Quick start

### Decode

```rust
use zenjxl::{decode, probe, JxlLimits};
use zenpixels::PixelDescriptor;

let jxl_bytes: &[u8] = &std::fs::read("photo.jxl").unwrap();

// Metadata-only probe (no pixel decode).
let info = probe(jxl_bytes).unwrap();
println!("{}x{}, alpha={}, gray={}", info.width, info.height, info.has_alpha, info.is_gray);

// Full decode with resource limits. The 3rd arg is a pixel-format preference
// list (`&[zenpixels::PixelDescriptor]`); `&[]` lets the decoder pick natively.
let limits = JxlLimits {
    max_pixels: Some(120_000_000),                  // 108 MP photos are common
    max_memory_bytes: Some(2 * 1024 * 1024 * 1024),
};
let output = decode(jxl_bytes, Some(&limits), &[]).unwrap();

// `output.pixels` is a `zenpixels::PixelBuffer` in the image's NATIVE format
// (opaque → RGB8, with alpha → RGBA8). Normalize to packed RGBA8 bytes with the
// `zenpixels-convert` extension trait:
use zenpixels_convert::PixelBufferConvertTypedExt;
let rgba: Vec<u8> = output.pixels.to_rgba8().copy_to_contiguous_bytes(); // w*h*4, R,G,B,A

// Need the bytes in their native layout instead? Read them straight off the
// PixelBuffer — no conversion crate required. `descriptor()` tells you the layout
// (e.g. RGB8 vs RGBA8); `width()`/`height()` give the dimensions.
let (w, h) = (output.pixels.width(), output.pixels.height());
let desc = output.pixels.descriptor();              // PixelDescriptor: channels + bit depth
let native: Vec<u8> = output.pixels.into_vec();      // owned, tightly packed, w*h*desc.bytes_per_pixel()
// (use `output.pixels.as_contiguous_bytes()` — `Option<&[u8]>`, `Some` only when rows
// are unpadded — to borrow instead of taking an owned Vec.)
```

**Limits.** `max_pixels` and `max_memory_bytes` both gate up front: before any
frame is decoded the wrapper rejects the image if `width * height` exceeds
`max_pixels`, or if the `width * height * bytes_per_pixel` output estimate
exceeds `max_memory_bytes`. `max_memory_bytes` is *also* forwarded to the
decoder's internal memory tracker, which bounds allocations during the decode
itself — so it caps both the early estimate and the live decode, not just the
output buffer. Pass `None` for `limits` to use the decoder's built-in defaults.

**Dependencies & errors.** Besides `zenjxl`, add `zenpixels` (`PixelBuffer`/
`PixelDescriptor`), `zenpixels-convert` (the `.to_rgba8()` trait), and `enough`
(cancellation). `decode`/`probe`/`encode_*` return `Result<_, whereat::At<E>>`
(`At<JxlError>`): the `At<…>` adds a build-time source location for logs — print
the error with its captured frames via `err.full_trace()` (a `Display`, e.g.
`println!("{}", err.full_trace())`). Get the underlying error with `err.error()`
(borrow) or `err.decompose().0` (owned — `decompose` also hands back the trace),
then match the `JxlError` enum (it is `#[non_exhaustive]`, so keep a wildcard
arm).

**Cancellation.** `decode_with_options(data, limits, preferred, parallel, stop)`
adds a cancellation token. `parallel: Option<bool>` toggles multithreaded decode
(`None` = default); `stop: Option<Arc<dyn enough::Stop>>` is the token. Build a
real one with `almost_enough::Stopper` (`cargo add almost-enough`):

```rust
use std::sync::Arc;
use zenjxl::{decode_with_options, JxlLimits};

let stopper = almost_enough::Stopper::new();
let watcher = stopper.clone(); // Stopper is Clone; shares the cancel flag
std::thread::spawn(move || watcher.cancel()); // e.g. on a deadline / client disconnect

let limits = JxlLimits { max_pixels: Some(120_000_000), max_memory_bytes: Some(2 * 1024 * 1024 * 1024) };
let stop: Arc<dyn enough::Stop> = Arc::new(stopper);
let output = decode_with_options(jxl_bytes, Some(&limits), &[], None, Some(stop))?;
```

### Encode

```rust
use zenjxl::{LossyConfig, LosslessConfig, PixelLayout, calibrated_jxl_quality, quality_to_distance};

let rgb: &[u8] = &[0u8; 256 * 256 * 3]; // packed RGB8 pixels

// Lossy. JXL is parameterized by butteraugli *distance*, where LOWER = better
// (0.0 = mathematically lossless, ~1.0 = visually lossless, larger = smaller file).
// Map a 0..=100 quality to a distance with the calibrated chain:
//   calibrated_jxl_quality(generic_q) -> native JXL quality (0..=100),
//   quality_to_distance(native_q)     -> butteraugli distance.
let distance = quality_to_distance(calibrated_jxl_quality(85.0));
let lossy = LossyConfig::new(distance)
    .encode(rgb, 256, 256, PixelLayout::Rgb8)
    .unwrap();

// Lossless.
let lossless = LosslessConfig::new()
    .encode(rgb, 256, 256, PixelLayout::Rgb8)
    .unwrap();
```

`PixelLayout` also covers `Rgba8`, `Bgra8`, and `Gray8` (plus 16-bit and
linear-f32 variants). `quality_to_distance` alone maps quality straight to
distance; `calibrated_jxl_quality` first re-maps a libjpeg-turbo-style quality
onto JXL's native quality scale so a given number lands at the same perceptual
level it would in a JPEG encoder. Convenience wrappers
(`encode_rgb8`/`encode_rgba8`/…) exist for the [`imgref`](https://docs.rs/imgref)
`ImgRef<rgb::Rgb<u8>>` pixel types if you already hold those.

## Features

**Decode** -- `probe()` returns `JxlInfo` with dimensions, bit depth, ICC profile, CICP signaling, EXIF orientation, raw EXIF/XMP bytes, extra channel metadata, HDR tone mapping fields, preview size, and gain map bundles. `decode()` returns a `PixelBuffer` with automatic format negotiation. `decode_with_parallel()` enables multithreaded decoding; `decode_with_options()` adds cancellation via `enough::Stop`.

**Encode** -- Convenience functions for RGB, RGBA, BGRA, and grayscale u8 data in both lossy (VarDCT) and lossless (modular) modes. `calibrated_jxl_quality()` maps a 0--100 quality scale to JXL distance. Container utilities (`append_gain_map_box`, `is_bare_codestream`, `is_container`) for gain map authoring.

**JPEG recompression** -- With the `jpeg-lossy` feature, `jpeg_lossy::recompress_jpeg_lossy_target()` recompresses an existing JPEG to JPEG XL toward a perceptual-quality target. Three paths: coefficient-domain `Coarsen` (re-quantize the source's own DCT coefficients — no re-encode), full `Reencode` (decode and re-encode through VarDCT), and `Auto` (run both to the target, keep the smaller). The closed loop is metric-agnostic — you supply the scorer over decoded RGB8 — so it can target zensim, butteraugli, SSIMULACRA2, or any metric.

**Gain maps** -- Decode extracts `GainMapBundle` from `jhgm` container boxes (ISO 21496-1). Encode can append gain map boxes to existing codestreams. With the `reconstruct-hdr` feature, the zencodec decode adapter applies an SDR-base gain map to reconstruct an HDR image (linear f32/f16 output with a CLL/MDCV envelope) via [ultrahdr-core](https://github.com/imazen/ultrahdr).

**HDR metadata** -- `JxlInfo` exposes `intensity_target`, `min_nits`, `relative_to_max_display`, and `linear_below` from the JXL tone mapping header.

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `decode` | yes | JPEG XL decoding via [zenjxl-decoder](https://github.com/imazen/zenjxl-decoder) |
| `encode` | yes | JPEG XL encoding via [jxl-encoder](https://github.com/imazen/jxl-encoder) — lossless (modular) + lossy (VarDCT) |
| `threads` | no | Multithreaded decoding via rayon (requires `decode`) |
| `parallel` | no | Per-frame parallelism inside the encoder via rayon (requires `encode`) |
| `butteraugli-loop` | no | Perceptual quality tuning during encode (requires `encode`) |
| `jpeg-lossy` | no | Lossy JPEG → JXL recompression to a quality target (requires `encode` + `decode`) |
| `zencodec` | no | Config/Job/Executor trait integration for zen codec pipelines |
| `reconstruct-hdr` | no | Native HDR reconstruction from SDR-base gain maps in the zencodec adapter (requires `zencodec` + `decode`) |

> The `__expert` feature forwards jxl-encoder's internal-parameter escape hatch (used for picker training and codec-calibration sweeps). It is private and unstable — anything reachable through it can change without a semver bump, so don't depend on it in production.

## Limitations

- The `encode_rgb8`/`encode_rgba8`/… convenience wrappers are u8-only. The
  `LossyConfig`/`LosslessConfig` `encode()` path and the zencodec adapter support
  wider bit depths (16-bit and linear-f32 layouts via `PixelLayout`).
- zenjxl-decoder does not yet support all JPEG XL features (e.g., some edge cases in progressive decoding).

## License

Dual-licensed: [AGPL-3.0](https://github.com/imazen/zenjxl/blob/main/LICENSE-AGPL3) or [commercial](https://github.com/imazen/zenjxl/blob/main/LICENSE-COMMERCIAL).

I've maintained and developed open-source image server software -- and the 40+
library ecosystem it depends on -- full-time since 2011. Fifteen years of
continual maintenance, backwards compatibility, support, and the (very rare)
security patch. That kind of stability requires sustainable funding, and
dual-licensing is how we make it work without venture capital or rug-pulls.
Support sustainable and secure software; swap patch tuesday for patch leap-year.

[Our open-source products](https://www.imazen.io/open-source)

**Your options:**

- **Startup license** -- $1 if your company has under $1M revenue and fewer
  than 5 employees. [Get a key](https://www.imazen.io/pricing)
- **Commercial subscription** -- Governed by the Imazen Site-wide Subscription
  License v1.1 or later. Apache 2.0-like terms, no source-sharing requirement.
  Sliding scale by company size.
  [Pricing & 60-day free trial](https://www.imazen.io/pricing)
- **AGPL v3** -- Free and open. Share your source if you distribute.

See [LICENSE-COMMERCIAL](https://github.com/imazen/zenjxl/blob/main/LICENSE-COMMERCIAL) for details.

Upstream code from [libjxl/libjxl](https://github.com/libjxl/libjxl) is licensed under BSD-3-Clause.
Our additions and improvements are dual-licensed (AGPL-3.0 or commercial) as above.

### Upstream contribution

We are willing to release our improvements under the original BSD-3-Clause
license if upstream takes over maintenance of those improvements. We'd rather
contribute back than maintain a parallel codebase. Open an issue or reach out.

## Image tech I maintain

| | |
|:--|:--|
| **Codecs** ¹ | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] · **zenjxl** · [zenbitmaps] · [heic] · [zentiff] · [zenpdf] · [zensvg] · [zenjp2] · [zenraw] · [ultrahdr] |
| Codec internals | [zenjxl-decoder] · [jxl-encoder] · [zenrav1e] · [rav1d-safe] · [zenavif-parse] · [zenavif-serialize] |
| Compression | [zenflate] · [zenzop] · [zenzstd] |
| Processing | [zenresize] · [zenquant] · [zenblend] · [zenfilters] · [zensally] · [zentone] |
| Pixels & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline & framework | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] · [zenwasm] · [zentract] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [zenmetrics] · [resamplescope-rs] |
| Pickers & ML | [zenanalyze] · [zenpredict] · [zenpicker] |
| Products | [Imageflow] image engine ([.NET][imageflow-dotnet] · [Node][imageflow-node] · [Go][imageflow-go]) · [Imageflow Server] · [ImageResizer] (C#) |

<sub>¹ pure-Rust, `#![forbid(unsafe_code)]` codecs, as of 2026</sub>

### General Rust awesomeness

[zenbench] · [archmage] · [magetypes] · [enough] · [whereat] · [cargo-copter]

[Open source](https://www.imazen.io/open-source) · [@imazen](https://github.com/imazen) · [@lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith)

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic
[zentiff]: https://github.com/imazen/zentiff
[zenpdf]: https://github.com/imazen/zenpdf
[zensvg]: https://github.com/imazen/zenextras
[zenjp2]: https://github.com/imazen/zenextras
[zenraw]: https://github.com/imazen/zenraw
[ultrahdr]: https://github.com/imazen/ultrahdr
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenrav1e]: https://github.com/imazen/zenrav1e
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenavif-serialize]: https://github.com/imazen/zenavif-serialize
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenzstd]: https://github.com/imazen/zenzstd
[zenresize]: https://github.com/imazen/zenresize
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zenfilters]: https://github.com/imazen/zenfilters
[zensally]: https://github.com/imazen/zensally
[zentone]: https://github.com/imazen/zentone
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zencodecs
[zenlayout]: https://github.com/imazen/zenlayout
[zennode]: https://github.com/imazen/zennode
[zenwasm]: https://github.com/imazen/zenwasm
[zentract]: https://github.com/imazen/zentract
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenmetrics]: https://github.com/imazen/zenmetrics
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[zenanalyze]: https://github.com/imazen/zenanalyze
[zenpredict]: https://github.com/imazen/zenanalyze
[zenpicker]: https://github.com/imazen/zenanalyze
[zenbench]: https://github.com/imazen/zenbench
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[cargo-copter]: https://github.com/imazen/cargo-copter
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-dotnet-server
[ImageResizer]: https://github.com/imazen/resizer
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
