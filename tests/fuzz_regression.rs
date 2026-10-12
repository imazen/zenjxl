//! Fuzz regression gate: every file under `fuzz/regression/` (minimised crash
//! reproducers) and `fuzz/seeds/` (curated container shapes) goes through the
//! `inventory` fuzz target's body on the stable toolchain.

#![cfg(feature = "inventory")]

use std::path::PathBuf;

use zencodec::decode::{DecodeJob as _, DecoderConfig as _};
use zenjxl::JxlDecoderConfig;

fn files(dir: &str) -> Vec<(String, Vec<u8>)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{} must exist: {e}", root.display()))
        .map(|e| e.expect("readable dir entry").path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            !name.starts_with('.') && !name.ends_with(".md") && !name.ends_with(".txt")
        })
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).expect("readable seed"),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Mirror of `fuzz/fuzz_targets/inventory.rs`.
fn run(data: &[u8]) {
    let inv = JxlDecoderConfig::new()
        .job()
        .inventory(data)
        .expect("inventory never errors without a stop token or part overflow");
    assert_eq!(inv.input_len(), data.len() as u64);
    inv.validate().expect("inventory must validate");
}

#[test]
fn regression_and_seed_files_yield_valid_inventories() {
    let seeds = files("fuzz/seeds");
    // A count, not just non-empty: a gutted corpus must not pass.
    assert!(seeds.len() >= 15, "fuzz/seeds holds {} files", seeds.len());
    let mut n = 0;
    for (name, data) in seeds.iter().chain(files("fuzz/regression").iter()) {
        std::panic::catch_unwind(|| run(data)).unwrap_or_else(|_| panic!("{name} failed"));
        n += 1;
        // Every truncation too, as the conformance check does.
        for cut in (0..data.len()).step_by(7) {
            run(&data[..cut]);
        }
    }
    assert!(n >= 15);
}
