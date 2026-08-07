//! Compile-checks for the code examples in `README.md`.
//!
//! The README's "Quick start" snippets reach across crate boundaries
//! (`zenpixels` for `PixelBuffer`, `zenpixels-convert` for `.to_rgba8()`,
//! `whereat` for the `At<JxlError>` wrapper, `almost-enough` for a concrete
//! `Stop` token). Those types aren't visible from a plain `cargo test --doc`
//! run, so the README examples used to drift out of sync with the real API
//! (see issue #9). This test mirrors them so CI fails the moment a snippet
//! stops compiling.
//!
//! Keep these bodies in lockstep with `README.md`. They only need to compile —
//! the file paths and empty byte slices mean nothing is actually decoded at
//! runtime, so the functions are never called.
//!
//! Gated on `decode + encode + zencodec` because the decode snippet uses the
//! `zenpixels-convert` extension trait (pulled in by `zencodec`) and the encode
//! snippet uses the encoder re-exports. CI exercises it via `cargo test
//! --all-features`.
#![cfg(all(feature = "decode", feature = "encode", feature = "zencodec"))]
#![allow(unused_variables, dead_code, clippy::needless_doctest_main)]

// ── Decode → packed RGBA8 (README "Decode") ─────────────────────────────────
fn decode_to_rgba8() {
    use zenjxl::{JxlLimits, decode, probe};
    use zenpixels::PixelDescriptor;

    let jxl_bytes: &[u8] = &[];

    // Metadata-only probe (no pixel decode).
    let info = probe(jxl_bytes).unwrap();
    let _ = (info.width, info.height, info.has_alpha, info.is_gray);

    // Full decode with resource limits. `&[]` lets the decoder pick natively.
    let limits = JxlLimits {
        max_pixels: Some(120_000_000),
        max_memory_bytes: Some(2 * 1024 * 1024 * 1024),
    };
    let output = decode(jxl_bytes, Some(&limits), &[]).unwrap();

    // Normalize to packed RGBA8 bytes via the `zenpixels-convert` trait.
    use zenpixels_convert::PixelBufferConvertTypedExt;
    let rgba: Vec<u8> = output.pixels.to_rgba8().copy_to_contiguous_bytes();

    // Native layout straight off the PixelBuffer.
    let (w, h) = (output.pixels.width(), output.pixels.height());
    let desc: PixelDescriptor = output.pixels.descriptor();
    let _bpp = desc.bytes_per_pixel();
    {
        // Borrowing native bytes (Some only when rows are unpadded).
        let borrowed: Option<&[u8]> = output.pixels.as_contiguous_bytes();
        let _ = borrowed;
    }
    let native: Vec<u8> = output.pixels.into_vec();
    let _ = (rgba, w, h, native);
}

// ── Error handling (README "Dependencies & errors") ─────────────────────────
fn match_decode_error() {
    use zenjxl::{JxlError, decode};
    let jxl_bytes: &[u8] = &[];
    match decode(jxl_bytes, None, &[]) {
        Ok(_) => {}
        Err(err) => {
            // Print the error with its captured trace frames.
            let _ = format!("{}", err.full_trace());
            // Borrow / own the underlying JxlError.
            let _borrowed: &JxlError = err.error();
            let _owned: JxlError = err.decompose().0;
        }
    }
}

// ── Cancellation (README "Cancellation") ────────────────────────────────────
fn decode_with_cancellation() -> Result<(), whereat::At<zenjxl::JxlError>> {
    use std::sync::Arc;
    use zenjxl::{JxlLimits, decode_with_options};

    let jxl_bytes: &[u8] = &[];

    let stopper = almost_enough::Stopper::new();
    let watcher = stopper.clone(); // Stopper is Clone; shares the cancel flag
    std::thread::spawn(move || watcher.cancel());

    let limits = JxlLimits {
        max_pixels: Some(120_000_000),
        max_memory_bytes: Some(2 * 1024 * 1024 * 1024),
    };
    let stop: Arc<dyn enough::Stop> = Arc::new(stopper);
    let output = decode_with_options(jxl_bytes, Some(&limits), &[], None, Some(stop))?;
    let _ = output;
    Ok(())
}

// ── Encode (README "Encode") ────────────────────────────────────────────────
fn encode_lossy_and_lossless() {
    use zenjxl::{
        LosslessConfig, LossyConfig, PixelLayout, calibrated_jxl_quality, quality_to_distance,
    };

    let rgb: &[u8] = &[0u8; 256 * 256 * 3]; // packed RGB8 pixels

    let distance = quality_to_distance(calibrated_jxl_quality(85.0));
    let lossy = LossyConfig::new(distance)
        .encode(rgb, 256, 256, PixelLayout::Rgb8)
        .unwrap();

    let lossless = LosslessConfig::new()
        .encode(rgb, 256, 256, PixelLayout::Rgb8)
        .unwrap();

    let _ = (lossy, lossless);
}

// ── One-shot Quick start (README lead) ──────────────────────────────────────
fn quick_start_one_shot() -> Result<(), whereat::At<zenjxl::JxlError>> {
    use zenjxl::{decode_rgba8, encode, encode_lossless};
    use zenpixels::{PixelDescriptor, PixelSlice};

    // 2×2 RGBA, tightly packed — dims + stride + format ride with the pixels.
    let (width, height) = (2u32, 2u32);
    let rgba = vec![
        255, 0, 0, 255, 0, 255, 0, 255, //
        0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let stride = width as usize * 4; // bytes per row (tightly packed)
    let img = PixelSlice::new(&rgba, width, height, stride, PixelDescriptor::RGBA8_SRGB)
        .expect("valid 2x2 RGBA8 slice");

    // Lossy at the default butteraugli distance 1.0 (≈ visually lossless).
    let jxl = encode(img)?; // no separate width/height arguments
    let (pixels, w, h) = decode_rgba8(&jxl)?;
    assert_eq!((w, h), (width, height));
    assert_eq!(pixels.len(), (width * height * 4) as usize);

    // Lossless round-trips the bytes exactly.
    let img2 = PixelSlice::new(&rgba, width, height, stride, PixelDescriptor::RGBA8_SRGB)
        .expect("valid 2x2 RGBA8 slice");
    let jxl_lossless = encode_lossless(img2)?;
    let (pixels_lossless, _, _) = decode_rgba8(&jxl_lossless)?;
    assert_eq!(pixels_lossless, rgba);
    Ok(())
}
