# zenjxl

JPEG XL encoding and decoding with `zencodec-types` trait integration.

Wraps [jxl-rs](https://github.com/libjxl/jxl-rs) for decoding and `jxl-encoder` for encoding. Both are feature-gated (`decode` and `encode`).

## Features

- `decode` — JPEG XL decoding via jxl-rs
- `encode` — JPEG XL encoding via jxl-encoder
- `zencodec` — Enable zencodec-types trait integration
- `butteraugli-loop` — Perceptual quality tuning (requires `encode`)

## zencodec-types integration

`JxlEncoderConfig` implements `zencodec_types::EncoderConfig` and `JxlDecoderConfig` implements `zencodec_types::DecoderConfig`, enabling use in the unified zen* codec pipeline.

## License

Sustainable, large-scale open source work requires a funding model, and I have been doing this full-time for 15 years. If you are using this for closed-source development AND make over $1 million per year, you'll need to buy a commercial license at https://www.imazen.io/pricing

Commercial licenses are similar to the Apache 2 license but company-specific, and on a sliding scale. You can also use this under the AGPL v3.
