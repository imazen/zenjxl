# zenjxl

JPEG XL encoding and decoding with `zencodec-types` trait integration.

Wraps [jxl-rs](https://github.com/libjxl/jxl-rs) for decoding and `jxl-encoder` for encoding. Both are feature-gated (`decode` and `encode`).

## Features

- `decode` — JPEG XL decoding via jxl-rs
- `encode` — JPEG XL encoding via jxl-encoder
- `std` — Enable std support (default)

## zencodec-types integration

`JxlEncoderConfig` implements `zencodec_types::EncoderConfig` and `JxlDecoderConfig` implements `zencodec_types::DecoderConfig`, enabling use in the unified zen* codec pipeline.

## License

AGPL-3.0-or-later
