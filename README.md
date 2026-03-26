# zenjxl

JPEG XL encoding and decoding with `zencodec` trait integration.

Wraps [jxl-rs](https://github.com/libjxl/jxl-rs) for decoding and `jxl-encoder` for encoding. Both are feature-gated (`decode` and `encode`).

## Features

- `decode` — JPEG XL decoding via jxl-rs
- `encode` — JPEG XL encoding via jxl-encoder
- `threads` — Multithreaded decoding via rayon (requires `decode`)
- `zencodec` — Enable zencodec trait integration
- `butteraugli-loop` — Perceptual quality tuning (requires `encode`)

## zencodec integration

`JxlEncoderConfig` implements `zencodec::EncoderConfig` and `JxlDecoderConfig` implements `zencodec::DecoderConfig`, enabling use in the unified zen* codec pipeline.

## License

Dual-licensed: [AGPL-3.0](LICENSE-AGPL3) or [commercial](LICENSE-COMMERCIAL).

I've maintained and developed open-source image server software — and the 40+
library ecosystem it depends on — full-time since 2011. Fifteen years of
continual maintenance, backwards compatibility, support, and the (very rare)
security patch. That kind of stability requires sustainable funding, and
dual-licensing is how we make it work without venture capital or rug-pulls.
Support sustainable and secure software; swap patch tuesday for patch leap-year.

[Our open-source products](https://www.imazen.io/open-source)

**Your options:**

- **Startup license** — $1 if your company has under $1M revenue and fewer
  than 5 employees. [Get a key →](https://www.imazen.io/pricing)
- **Commercial subscription** — Governed by the Imazen Site-wide Subscription
  License v1.1 or later. Apache 2.0-like terms, no source-sharing requirement.
  Sliding scale by company size.
  [Pricing & 60-day free trial →](https://www.imazen.io/pricing)
- **AGPL v3** — Free and open. Share your source if you distribute.

See [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.

Upstream code from [libjxl/libjxl](https://github.com/libjxl/libjxl) is licensed under BSD-3-Clause.
Our additions and improvements are AGPL-3.0-or-later with commercial licensing as above.

### Upstream Contribution

We are willing to release our improvements under the original BSD-3-Clause
license if upstream takes over maintenance of those improvements. We'd rather
contribute back than maintain a parallel codebase. Open an issue or reach out.
