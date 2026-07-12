//! Per-image lossless sweep harness for proxy-policy research: encode ONE
//! image under a list of sweep cell ids, print one TSV row per cell to
//! stdout (`basename<TAB>cell<TAB>bytes<TAB>encode_ms`).
//!
//! Designed to be driven one-process-per-image (e.g. `xargs -P N`) so the
//! known cross-cell allocator high-water growth of long-lived jxl sweep
//! processes is bounded by construction (each process encodes one image's
//! cells and exits), and so wall-time parallelism needs no in-process
//! orchestration. Encodes are timed individually with `Instant` — cell
//! wall-time in a `RAYON_NUM_THREADS=1` process is the proxy-relevant
//! single-stream encode cost.
//!
//! Usage:
//!     proxy_policy_sweep <image.png> <cell,cell,...>
//!
//! Cell ids use the sweep grammar (`sweep::variant_from_cell_id`), e.g.
//! `mod-e9_def-pal0`, `mod-e7_lloyd`. Requires `--features __expert`.

use std::time::Instant;

use zenjxl::sweep::{BuiltConfig, variant_from_cell_id};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(cells)) = (args.next(), args.next()) else {
        eprintln!("usage: proxy_policy_sweep <image.png> <cell,cell,...>");
        std::process::exit(2);
    };
    let img = match zenjpeg_bench_utils::load_png(std::path::Path::new(&path)) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("FAIL load {path}: {e}");
            std::process::exit(1);
        }
    };
    let basename = path.rsplit('/').next().unwrap_or(&path);

    for cell in cells.split(',') {
        let variant = match variant_from_cell_id(cell) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("FAIL parse {cell}: {e}");
                std::process::exit(1);
            }
        };
        let BuiltConfig::Lossless(cfg) = variant.build() else {
            eprintln!("FAIL {cell}: not a lossless cell");
            std::process::exit(1);
        };
        let t = Instant::now();
        match jxl_encoder::convenience::encode_rgb8_lossless(img.as_ref(), &cfg) {
            Ok(bytes) => {
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                println!("{basename}\t{cell}\t{}\t{ms:.1}", bytes.len());
            }
            Err(e) => {
                eprintln!("FAIL encode {basename} {cell}: {e}");
                std::process::exit(1);
            }
        }
    }
}
