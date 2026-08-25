// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! Ad-hoc repro harness for the 2026-08-24/25 fleetbench OOM +
//! encoder-panic failures (see zenmetrics `Known Bugs`). Encodes real
//! PNGs through the exact plan-cell grammar the fleet used
//! (`vd-e9_zen_def_q50`, `mod-e5_def`, etc.) so the failures can be
//! reproduced and profiled locally, outside the fleet. Mirrors
//! `zenmetrics-cli`'s `PlannedConfig::Zenjxl` re-encode path exactly,
//! including its `.with_threads(1)` pin (playbook pattern 9).
//!
//! Accepts any number of `<path-to-png> <cell-id>` pairs and encodes
//! them IN SEQUENCE within one process (mirrors a fleet "chunk" that
//! batches many cells per process/container) — prints `VmRSS`/`VmHWM`
//! after each cell so cross-cell accumulation is visible even when a
//! single cell's own peak is small.
//!
//! ```bash
//! cargo run --release --features "__expert butteraugli-loop" \
//!   --example fleetbench_repro -- \
//!   <path-to-png> <cell-id> [<path-to-png> <cell-id> ...]
//! # e.g.
//! cargo run --release --features "__expert butteraugli-loop" \
//!   --example fleetbench_repro -- \
//!   6609_....scale2048x1618.png vd-e9_zen_def_q50 \
//!   6609_....scale2048x1618.png vd-e9_zen_def_q95
//! ```
//!
//! Diagnostic tool, not a regression test — kept in-repo for the next
//! fleet-failure investigation (per this workspace's "commit the
//! harness, don't leave it in scratch" rule). The actual regression
//! coverage for the 2026-08-25 fix lives in jxl-encoder's
//! `vardct::coeff_order::tests` (`fallible_alloc_toggle_is_byte_identical_multiband`
//! + the two `*_errors_instead_of_aborting_on_absurd_size` tests).
//!
//! Findings from the investigation this tool was built for
//! (`benchmarks/` in zenmetrics has the fleet-side incident writeup):
//! the `oom` error class is fleet-concurrency-driven (many single-threaded
//! encodes admitted concurrently into one memory-capped container/cgroup —
//! reproduced with plain shell concurrency, not this tool), and the
//! `encoder_panic` class was a generic Rust allocator abort under
//! artificial memory pressure (`ulimit -v`), not a jxl-encoder logic bug —
//! see jxl-encoder commit `cf50d7cf99de` for the fix (a fallible-alloc
//! conversion of `count_zero_coefficients`, so an allocation failure now
//! returns `Result::Err` instead of aborting the process).

use std::path::PathBuf;

use rgb::ComponentBytes;
use zenjxl::PixelLayout;
use zenjxl::sweep::{BuiltConfig, variant_from_cell_id};

fn print_rss(tag: &str) {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        let mut rss = "?";
        let mut hwm = "?";
        for line in status.lines() {
            if let Some(v) = line.strip_prefix("VmRSS:") {
                rss = v.trim();
            } else if let Some(v) = line.strip_prefix("VmHWM:") {
                hwm = v.trim();
            }
        }
        eprintln!("[{tag}] VmRSS={rss} VmHWM={hwm}");
    }
}

fn run_one(path: &PathBuf, cell_id: &str, idx: usize) {
    eprintln!("--- cell {idx}: {path:?} {cell_id:?} ---");
    let img =
        zenjpeg_bench_utils::load_png(path).unwrap_or_else(|e| panic!("failed to load png: {e}"));
    let width = img.width() as u32;
    let height = img.height() as u32;
    assert_eq!(img.stride(), img.width(), "harness expects tight buffers");
    let pixels = img.buf().as_bytes();
    eprintln!(
        "loaded {width}x{height} ({} px, {} bytes rgb8)",
        (width as u64) * (height as u64),
        pixels.len()
    );

    let variant = variant_from_cell_id(cell_id)
        .unwrap_or_else(|e| panic!("failed to parse cell id {cell_id:?}: {e}"));

    let built = variant.build();
    built
        .validate()
        .unwrap_or_else(|e| panic!("config failed validation: {e}"));

    // Mirror zenmetrics-cli's `PlannedConfig::Zenjxl::encode_bytes` EXACTLY
    // (crates/zenmetrics-cli/src/sweep/plan.rs): threads pinned to 1 for
    // deterministic content addressing, no explicit `Limits` (so
    // `fallible_alloc` defaults to `false` — the production behaviour).
    //
    // `FLEETBENCH_FALLIBLE_ALLOC=1` opts into `Limits::with_fallible_alloc(true)`
    // instead — zenmetrics' actual plan-cell path does NOT do this today (it
    // calls the bare `.encode()` convenience method with no `Limits` at all),
    // so this flag demonstrates what the jxl-encoder-side fallible-alloc fix
    // (`vardct::coeff_order::count_zero_coefficients`) can prevent IF a
    // caller opts in — it is not, by itself, what the fleet's production
    // path does. See the module docs above.
    let fallible_alloc = std::env::var_os("FLEETBENCH_FALLIBLE_ALLOC").is_some();
    let limits = jxl_encoder::Limits::new().with_fallible_alloc(fallible_alloc);
    eprintln!("encoding... (fallible_alloc={fallible_alloc})");
    let result = match &built {
        BuiltConfig::Lossy(cfg) => cfg
            .clone()
            .with_threads(1)
            .encode_request(width, height, PixelLayout::Rgb8)
            .with_limits(&limits)
            .encode(pixels),
        BuiltConfig::Lossless(cfg) => cfg
            .clone()
            .with_threads(1)
            .encode_request(width, height, PixelLayout::Rgb8)
            .with_limits(&limits)
            .encode(pixels),
    };

    match result {
        Ok(bytes) => {
            eprintln!("OK: {} bytes", bytes.len());
        }
        Err(e) => {
            eprintln!("ENCODE ERROR (graceful): {e}");
            std::process::exit(2);
        }
    }
    print_rss(&format!("after cell {idx} ({cell_id})"));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(
        !args.is_empty() && args.len() % 2 == 0,
        "usage: <png-path> <cell-id> [<png-path> <cell-id> ...]"
    );
    print_rss("start");
    for (idx, chunk) in args.chunks(2).enumerate() {
        let path = PathBuf::from(&chunk[0]);
        let cell_id = &chunk[1];
        run_one(&path, cell_id, idx);
    }
    print_rss("end");
}
