//! Encode peak-memory probe — one JXL encode, report measured peak RSS (VmHWM).
//!
//! The ENCODE counterpart for the heaptrack / VmHWM sweep that calibrates the
//! encode peak-memory model (`jxl_encoder::heuristics::estimate_encode`, which
//! zenjxl's `JxlEncoderConfig::estimate_encode_resources` delegates to —
//! `src/codec.rs:581`) against measured reality, *per effort level*. JXL
//! encode memory is EFFORT-dependent: lossy gains a buttloop band at e>=8,
//! lossless ramps through tree-learning bands (e<=5 / e6 / e7-9 / e>=10).
//!
//!   cargo build -p zenjxl --release --example mem_probe_encode   # needs the `encode` feature (default-on)
//!   GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072 \
//!     ./target/release/examples/mem_probe_encode <rgb8.bin> <w> <h> <lossy|lossless> <effort 1..9> <quality>
//!   heaptrack ./target/release/examples/mem_probe_encode ...   # allocator peak heap
//!
//! One encode per process — peak RSS is a per-process high-water mark, so the
//! input must come from a cheap file read (raw RGB8 bin), never an in-process
//! decode (whose own peak would pollute VmHWM above the encode peak).
//!
//! VERIFY: this drives jxl-encoder's `LossyConfig`/`LosslessConfig` directly
//! (the same configs zenjxl re-exports and `JxlEncoderConfig` wraps) — the
//! exact path `estimate_encode` was calibrated on (see
//! `jxl-encoder/jxl-encoder-cli/examples/mem_probe.rs`). `encode()` takes a
//! raw `&[u8]` packed RGB8 buffer with `PixelLayout::Rgb8` (sRGB→linear is
//! handled internally).
//!
//! THREADS axis (7th positional arg, default 1): `with_threads(N)` selects the
//! pool width the encode runs on. This ONLY engages real parallelism when the
//! probe is built `--features parallel` (forwards `jxl-encoder/parallel`);
//! without it, `run_with_threads` is the no-op variant and every N is
//! sequential. jxl carries per-thread scratch (lossy buttloop/EPF ~2.5 MB/thr,
//! lossless tree-learning SplitWorkspace via thread-local cache), and the
//! production default (`threads=0` → ambient rayon global pool = all cores,
//! useful-capped at 8 lossy / 16 lossless) is multi-threaded, so the TYP must
//! cover the default thread count. We pass N (>=1) here rather than 0 so a
//! dedicated N-thread pool is built deterministically per cell (0 would inherit
//! whatever ambient pool exists, which is non-deterministic for a sweep).
//! NOTE: arg order is `... <quality> [threads] [est]` — `threads` slots BEFORE
//! the optional `est` marker; if you pass `est`, you must pass threads first.
//!
//! TSV row:
//!   w  h  pixels  mode  effort  quality  threads  out_bytes  pre_rss_kb  vmhwm_kb  marginal_kb

use std::hint::black_box;

use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};

/// A `/proc/self/status` field in KiB (e.g. `VmRSS:`, `VmHWM:`).
fn status_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(field))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 7 {
        eprintln!(
            "usage: mem_probe_encode <rgb8.bin> <w> <h> <lossy|lossless> <effort 1..9> <quality> [threads] [est]"
        );
        std::process::exit(2);
    }
    let path = &a[1];
    let w: u32 = a[2].parse().expect("w");
    let h: u32 = a[3].parse().expect("h");
    let mode = match a[4].as_str() {
        "lossy" | "lossless" => a[4].clone(),
        other => panic!("mode must be lossy|lossless, got {other}"),
    };
    let is_lossless = mode == "lossless";
    // effort axis = jxl `with_effort` (u8). jxl-encoder's native ceiling is 9
    // (libjxl kTortoise); e10/e11/e12 are this encoder's extensions. The setter
    // does NOT clamp (zenjxl's JxlEncoderConfig clamps to 1..=10, but the
    // re-exported LossyConfig/LosslessConfig used here pass effort straight
    // through). Representative sweep levels: 1, 4, 7.
    //   - lossy: e<=7 is one base band, e>=8 turns on the butteraugli loop —
    //     but the 2026-06-23 size sweep found the buttloop adds ~ZERO memory
    //     over e7 at equal quality (~80 B/px asymptotic; the working set is
    //     quality-sensitive, ~122 B/px at 1024² q50 dropping to ~65 at 2048²).
    //     e8/e9 at 4096² are SLOW (the buttloop's multi-resolution butteraugli
    //     precompute) — fine under run-heavy caps, but AVOID for quick runs.
    //   - lossless: e<=5 base (~72 B/px), e6 (~215 — heavier than once thought),
    //     e7-9 full MA tree-learning (~360-425 B/px), e>=10 (~620). e7+ at
    //     4096² is the heaviest cell here (tree-learning is ~size-independent
    //     in B/px but the absolute working set at 12 MP is ~5 GB) — AVOID
    //     e>=7 lossless at 4096² for quick runs.
    let effort: u8 = a[5].parse().expect("effort");
    // quality 0..100; lossy maps it to a butteraugli distance. lossless ignores
    // it (the encode is exact). VERIFY: quality_to_distance is the zenjxl/jxl
    // calibration curve; using it here keeps the distance consistent with what
    // JxlEncoderConfig::with_quality would resolve.
    let quality: f32 = a[6].parse().expect("quality");
    let distance = jxl_encoder::quality_to_distance(quality.clamp(0.0, 100.0));

    // threads (7th arg, default 1). `est` may appear as either the 7th arg
    // (no threads given) or the 8th (threads given) for back-compat with the
    // older `... <quality> est` form. Parse the 7th arg: if it's "est" it is
    // the marker (threads stays 1); otherwise it's the thread count.
    let arg7 = a.get(7).map(String::as_str);
    let (threads, est) = match arg7 {
        None => (1usize, false),
        Some("est") => (1usize, true),
        Some(t) => {
            let n: usize = t
                .parse()
                .expect("threads must be a positive integer or 'est'");
            let est = a.get(8).map(String::as_str) == Some("est");
            (n.max(1), est)
        }
    };

    let data = std::fs::read(path).expect("read rgb8.bin");
    assert_eq!(
        data.len(),
        (w as usize) * (h as usize) * 3,
        "bin size {} != w*h*3 {}",
        data.len(),
        (w as usize) * (h as usize) * 3
    );

    // Estimate-only mode (`est` as a 7th arg): print what the CURRENT model
    // predicts for this cell (min / typical / max peak + time), no encode — so
    // we can compare model vs measured without an encode polluting anything.
    // This is exactly what JxlEncoderConfig::estimate_encode_resources reads
    // (it forwards width/height/input_bpp=3/has_alpha=false/is_lossless/effort
    // to estimate_encode). RGB8 input → input_bpp = 3, has_alpha = false.
    if est {
        let pixels = (w as u64) * (h as u64);
        // estimate_encode_threaded folds in the per-thread term so the EST
        // line is comparable to the measured marginal at the same thread count.
        match jxl_encoder::heuristics::estimate_encode_threaded(
            w,
            h,
            3,
            false,
            is_lossless,
            effort,
            threads,
        ) {
            Some(e) => {
                println!(
                    "{w}\t{h}\t{pixels}\t{mode}\t{effort}\t{quality}\t{threads}\tEST\tmin_kb={}\ttyp_kb={}\tmax_kb={}\ttyp_bpp={:.2}\tmax_bpp={:.2}\ttime_ms={:.1}",
                    e.peak_memory_bytes_min / 1024,
                    e.peak_memory_bytes / 1024,
                    e.peak_memory_bytes_max / 1024,
                    e.peak_memory_bytes as f64 / pixels as f64,
                    e.peak_memory_bytes_max as f64 / pixels as f64,
                    e.time_ms,
                );
            }
            None => {
                println!(
                    "{w}\t{h}\t{pixels}\t{mode}\t{effort}\t{quality}\t{threads}\tEST\tNONE (dim overflow)"
                );
            }
        }
        return;
    }

    // Baseline RSS: process + libs + the input `data` we hold. Marginal =
    // VmHWM − pre isolates the encode's own working set (what the model
    // predicts). Read VmRSS (current), not VmHWM, so any transient pre-encode
    // peak doesn't inflate the baseline.
    let pre = status_kb("VmRSS:");

    // with_threads(N): N=1 forces a 1-worker pool (per-worker scratch excluded —
    // the thread-independent base the model's typical/min/max anchor on); N>1
    // builds a dedicated N-thread pool so the per-thread working set (lossless
    // SplitWorkspace, lossy buttloop/EPF scratch) shows up in VmHWM. Only
    // engages real parallelism when the probe is built `--features parallel`.
    let out = if is_lossless {
        LosslessConfig::new()
            .with_effort(effort)
            .with_threads(threads)
            .encode_request(w, h, PixelLayout::Rgb8)
            .encode(&data)
    } else {
        LossyConfig::new(distance)
            .with_effort(effort)
            .with_threads(threads)
            .encode_request(w, h, PixelLayout::Rgb8)
            .encode(&data)
    }
    .expect("encode");

    // High-water mark immediately after encode — VmHWM is monotonic, so it
    // reflects the peak *during* the encode.
    let peak = status_kb("VmHWM:");

    let pixels = (w as u64) * (h as u64);
    println!(
        "{w}\t{h}\t{pixels}\t{mode}\t{effort}\t{quality}\t{threads}\t{}\t{pre}\t{peak}\t{}",
        out.len(),
        peak.saturating_sub(pre)
    );
    black_box(&out);
}
