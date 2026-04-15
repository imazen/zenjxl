# zenjxl [![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenjxl/ci.yml?style=flat-square)](https://github.com/imazen/zenjxl/actions/workflows/ci.yml) [![MSRV](https://img.shields.io/badge/MSRV-1.93-blue?style=flat-square)](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field) [![License](https://img.shields.io/badge/license-AGPL--3.0--only%20OR%20Commercial-blue?style=flat-square)](https://github.com/imazen/zenjxl#license)

zenjxl is a JPEG XL encoding and decoding library combining [zenjxl-decoder](https://github.com/imazen/zenjxl-decoder) and [jxl-encoder](https://github.com/imazen/jxl-encoder) with resource limits, cancellation, and gain map support.

zenjxl-decoder is Imazen's fork of jxl-rs with additional metadata extraction, gain map parsing, and resource limiting. jxl-encoder is a pure Rust JPEG XL encoder supporting both lossless (modular) and lossy (VarDCT) modes. zenjxl wraps both behind a unified API and provides zencodec integration for use in [zenpipe](https://github.com/imazen/zenpipe) pipelines.

`#![forbid(unsafe_code)]`, `no_std + alloc`, edition 2024.

## Quick start

### Decode

```rust
use zenjxl::{decode, probe, JxlLimits};
use zenpixels::PixelDescriptor;

let jxl_bytes: &[u8] = &std::fs::read("photo.jxl").unwrap();

// Metadata-only probe (no pixel decode).
let info = probe(jxl_bytes).unwrap();
println!("{}x{}, alpha={}, gray={}", info.width, info.height, info.has_alpha, info.is_gray);

// Full decode with resource limits.
let limits = JxlLimits {
    max_pixels: Some(100_000_000),
    max_memory_bytes: Some(2 * 1024 * 1024 * 1024),
};
let output = decode(jxl_bytes, Some(&limits), &[]).unwrap();
let pixels = output.pixels; // PixelBuffer (zenpixels)
```

### Encode

```rust
use zenjxl::{encode_rgb8, encode_rgb8_lossless, calibrated_jxl_quality};

let rgb: &[u8] = &[0u8; 256 * 256 * 3]; // RGB pixels

// Lossy encode -- calibrated_jxl_quality maps 0..=100 to JXL distance.
let distance = calibrated_jxl_quality(85);
let lossy = encode_rgb8(rgb, 256, 256, distance).unwrap();

// Lossless encode.
let lossless = encode_rgb8_lossless(rgb, 256, 256).unwrap();
```

## Features

**Decode** -- `probe()` returns `JxlInfo` with dimensions, bit depth, ICC profile, CICP signaling, EXIF orientation, raw EXIF/XMP bytes, extra channel metadata, HDR tone mapping fields, preview size, and gain map bundles. `decode()` returns a `PixelBuffer` with automatic format negotiation. `decode_with_parallel()` enables multithreaded decoding; `decode_with_options()` adds cancellation via `enough::Stop`.

**Encode** -- Convenience functions for RGB, RGBA, BGRA, and grayscale u8 data in both lossy and lossless modes. `calibrated_jxl_quality()` maps a 0--100 quality scale to JXL distance. Container utilities (`append_gain_map_box`, `is_bare_codestream`) for gain map authoring.

**Gain maps** -- Decode extracts `GainMapBundle` from `jhgm` container boxes (ISO 21496-1). Encode can append gain map boxes to existing codestreams.

**HDR metadata** -- `JxlInfo` exposes `intensity_target`, `min_nits`, `relative_to_max_display`, and `linear_below` from the JXL tone mapping header.

## Feature flags

| Flag | Default | Description |
|------|---------|-------------|
| `decode` | yes | JPEG XL decoding via [zenjxl-decoder](https://github.com/imazen/zenjxl-decoder) |
| `encode` | yes | JPEG XL encoding via [jxl-encoder](https://github.com/imazen/jxl-encoder) |
| `threads` | no | Multithreaded decoding via rayon (requires `decode`) |
| `parallel` | no | Per-frame parallelism inside the encoder via rayon (requires `encode`) |
| `butteraugli-loop` | no | Perceptual quality tuning (requires `encode`) |
| `zencodec` | no | Config/Job/Executor trait integration for zen codec pipelines |

## Limitations

- Not published to crates.io. Depend on it via git or path.
- Encoder is u8-only for the convenience API. The zencodec path supports wider bit depths.
- zenjxl-decoder does not yet support all JPEG XL features (e.g., some edge cases in progressive decoding).

## Image tech I maintain

| | |
|:--|:--|
| State of the art codecs* | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] ([rav1d-safe] · [zenrav1e] · [zenavif-parse] · [zenavif-serialize]) · **zenjxl** ([jxl-encoder] · [zenjxl-decoder]) · [zentiff] · [zenbitmaps] · [heic] · [zenraw] · [zenpdf] · [ultrahdr] · [mozjpeg-rs] · [webpx] |
| Compression | [zenflate] · [zenzop] |
| Processing | [zenresize] · [zenfilters] · [zenquant] · [zenblend] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [resamplescope-rs] · [codec-eval] · [codec-corpus] |
| Pixel types & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] |
| ImageResizer | [ImageResizer] (C#) — 24M+ NuGet downloads across all packages |
| [Imageflow][] | Image optimization engine (Rust) — [.NET][imageflow-dotnet] · [node][imageflow-node] · [go][imageflow-go] — 9M+ NuGet downloads across all packages |
| [Imageflow Server][] | [The fast, safe image server](https://www.imazen.io/) (Rust+C#) — 552K+ NuGet downloads, deployed by Fortune 500s and major brands |

<sub>* as of 2026</sub>

### General Rust awesomeness

[archmage] · [magetypes] · [enough] · [whereat] · [zenbench] · [cargo-copter]

[And other projects](https://www.imazen.io/open-source) · [GitHub @imazen](https://github.com/imazen) · [GitHub @lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith) · [NuGet](https://www.nuget.org/profiles/imazen) (over 30 million downloads / 87 packages)

## License

Dual-licensed: [AGPL-3.0](LICENSE-AGPL3) or [commercial](LICENSE-COMMERCIAL).

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

See [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.

Upstream code from [libjxl/libjxl](https://github.com/libjxl/libjxl) is licensed under BSD-3-Clause.
Our additions and improvements are dual-licensed (AGPL-3.0 or commercial) as above.

### Upstream contribution

We are willing to release our improvements under the original BSD-3-Clause
license if upstream takes over maintenance of those improvements. We'd rather
contribute back than maintain a parallel codebase. Open an issue or reach out.

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zentiff]: https://github.com/imazen/zentiff
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic-decoder-rs
[zenraw]: https://github.com/imazen/zenraw
[zenpdf]: https://github.com/imazen/zenpdf
[ultrahdr]: https://github.com/imazen/ultrahdr
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenrav1e]: https://github.com/imazen/zenrav1e
[mozjpeg-rs]: https://github.com/imazen/mozjpeg-rs
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenavif-serialize]: https://github.com/imazen/zenavif-serialize
[webpx]: https://github.com/imazen/webpx
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenresize]: https://github.com/imazen/zenresize
[zenfilters]: https://github.com/imazen/zenfilters
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zencodecs
[zenlayout]: https://github.com/imazen/zenlayout
[zennode]: https://github.com/imazen/zennode
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-server
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
[ImageResizer]: https://github.com/imazen/resizer
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[zenbench]: https://github.com/imazen/zenbench
[cargo-copter]: https://github.com/imazen/cargo-copter
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[codec-eval]: https://github.com/imazen/codec-eval
[codec-corpus]: https://github.com/imazen/codec-corpus
