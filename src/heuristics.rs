//! Resource estimates for a zenjxl encode — peak memory, time, and the
//! thread count an encode will actually use.
//!
//! This is a thin, calibration-free forwarder to **jxl-encoder's** own
//! `heuristics` module, which already models JXL's encoder working set
//! (lossy vs lossless, effort-banded — lossless e9 tree-learning is ~300×
//! the per-pixel cost of e1) *and* its internal threading
//! (`encode_threading_info`: time ÷ speedup, plus a per-thread working-set
//! term added to peak memory). zenjxl re-exposes it so a fleet's
//! resource-estimate dispatch can treat JXL uniformly with the other codecs
//! (`zenavif::heuristics`, `zenpng::heuristics`, `zenwebp::heuristics` each
//! expose an `estimate_encode`).
//!
//! Why this matters for scheduling: a single JXL-modular/high-effort encode
//! can need multiple GB of working set and saturate many cores, while a small
//! lossy encode needs tens of MB and one thread — so a box's safe concurrency
//! is `Σ peak_memory ≤ RAM` and `Σ threads ≤ cores`, not a fixed per-core
//! fan-out. These estimates are the inputs to that admission control.

pub use jxl_encoder::heuristics::EncodeEstimate;

/// Memory + time estimate for a zenjxl encode, thread-aware.
///
/// - `input_bpp` — bytes per input pixel (3 = RGB8, 4 = RGBA8, 6 = RGB16).
/// - `has_alpha` — alpha plane present (lossless adds a per-pixel term for it).
/// - `lossless`, `effort` — select the calibration stratum (effort 1–9).
/// - `cores` — available CPU cores. `time_ms` is divided by JXL's speedup at
///   this effort, and JXL's per-thread working set is added to peak memory, so
///   the returned numbers are the *threaded* footprint on a box with `cores`.
///
/// Returns `None` only on dimension overflow. Mirrors the shape of
/// `zenavif::heuristics::estimate_encode` (same `EncodeEstimate` fields).
#[must_use]
pub fn estimate_encode(
    width: u32,
    height: u32,
    input_bpp: u8,
    has_alpha: bool,
    lossless: bool,
    effort: u8,
    cores: usize,
) -> Option<EncodeEstimate> {
    jxl_encoder::heuristics::estimate_encode_threaded(
        width, height, input_bpp, has_alpha, lossless, effort, cores,
    )
}

/// How many CPU threads a JXL encode at this (`lossless`, `effort`) will
/// actually use given `cores`. This is the thread term a fleet admission
/// controller needs to keep `Σ threads ≤ cores` when packing concurrent
/// encodes onto one box (a JXL encode that self-threads to 16 cores leaves no
/// room for 16 more single-threaded encodes alongside it).
#[must_use]
pub fn encode_threads(lossless: bool, effort: u8, cores: usize) -> u64 {
    jxl_encoder::heuristics::encode_threading_info(lossless, effort).effective_threads(cores)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_scales_and_holds_invariants() {
        let small = estimate_encode(1000, 1000, 3, false, false, 7, 8).unwrap();
        let big = estimate_encode(4000, 3000, 3, false, false, 7, 8).unwrap();
        // peak memory grows with pixel count.
        assert!(big.peak_memory_bytes > small.peak_memory_bytes);
        // min ≤ typical ≤ max for any estimate.
        assert!(big.peak_memory_bytes_min <= big.peak_memory_bytes);
        assert!(big.peak_memory_bytes <= big.peak_memory_bytes_max);
        // lossless is at least as memory-hungry as lossy at the same size/effort.
        let lossy = estimate_encode(4000, 3000, 3, false, false, 7, 8).unwrap();
        let lossless = estimate_encode(4000, 3000, 3, false, true, 7, 8).unwrap();
        assert!(lossless.peak_memory_bytes >= lossy.peak_memory_bytes);
    }

    #[test]
    fn threads_never_exceed_cores() {
        assert!(encode_threads(false, 7, 8) <= 8);
        assert!(encode_threads(true, 9, 16) <= 16);
        // 1 core → 1 thread, always.
        assert_eq!(encode_threads(true, 9, 1), 1);
    }
}
