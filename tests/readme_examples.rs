//! Compile-checks for the code examples in `README.md`.
//!
//! The README's "Quick start" snippets reach across crate boundaries
//! (`zenpixels` for `PixelBuf`, `zenpixels-convert` for `convert_to_buf_with_peak`,
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
//! `zenpixels-convert` conversion API (pulled in by `zencodec`) and the encode
//! snippet uses the encoder re-exports. CI exercises it via `cargo test
//! --all-features`.
#![cfg(all(feature = "decode", feature = "encode", feature = "zencodec"))]
#![allow(unused_variables, dead_code, clippy::needless_doctest_main)]

// ── Decode → packed RGBA8 (README "Decode") ─────────────────────────────────
fn decode_to_rgba8() {
    use zenjxl::{JxlLimits, decode, probe};
    use zenpixels::PixelFormat;

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

    // Convert the actual output color and sample layout to packed RGBA8.
    use zenpixels::PeakSource;
    use zenpixels_convert::{ConvertPolicy, convert_to_buf_with_peak};
    let converted = convert_to_buf_with_peak(
        &output.pixels.as_view(),
        &PixelFormat::RGBA8_SRGB,
        &ConvertPolicy::permissive(),
        PeakSource::Measured,
    )
    .unwrap();
    let rgba: Vec<u8> = converted.into_vec().unwrap();

    // Native layout straight off the PixelBuf.
    let (w, h) = (output.pixels.width(), output.pixels.height());
    let desc = output.pixels.format();
    let _bpp = desc.layout.bytes_per_pixel();
    let view = output.pixels.as_view();
    let borrowed: Option<&[u8]> = view.as_contiguous_bytes();
    let _ = borrowed;
    let native = output.pixels.into_parts();
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
