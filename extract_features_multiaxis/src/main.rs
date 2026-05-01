//! Feature extractor for the 2026-05-01 multiaxis corpus.
//!
//! Reads /mnt/v/output/codec-corpus-2026-05-01-multiaxis/manifest.tsv with
//! columns: relative_path, bytes, width, height, axis_class, source,
//! description.
//!
//! Emits the standard zentrain features TSV schema used by the existing
//! zenjxl picker config:
//!
//!   image_path  image_sha  split  content_class  source  size_class
//!   width  height  feat_<name>...
//!
//! For the "image_sha" column we synthesize a stable id from the
//! relative_path (since the new manifest doesn't carry sha256). The
//! "split" column is set to "train" for every row (we're not doing a
//! held-out validation in this run; downstream Tier 0 correlation
//! cleanup ignores split). "content_class" maps to `axis_class`.
//! "size_class" maps to `axis_class` (an axis IS the size+content
//! class for the new corpus). We do NOT resize.

use image::{GenericImageView, ImageReader};
use std::collections::HashMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use zenanalyze::analyze_features_rgb8;
use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet, FeatureValue};

#[derive(Clone, Debug)]
struct ManifestEntry {
    relative_path: String,
    axis_class: String,
    source: String,
}

fn read_manifest(path: &Path) -> Result<Vec<ManifestEntry>, String> {
    let f = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let r = BufReader::new(f);
    let mut out = Vec::new();
    let mut header_idx: HashMap<String, usize> = HashMap::new();
    for (i, line) in r.lines().enumerate() {
        let line = line.map_err(|e| format!("read line {i}: {e}"))?;
        let cols: Vec<&str> = line.split('\t').collect();
        if i == 0 {
            for (idx, name) in cols.iter().enumerate() {
                header_idx.insert(name.to_string(), idx);
            }
            continue;
        }
        let get = |k: &str| {
            header_idx
                .get(k)
                .and_then(|&idx| cols.get(idx).copied())
                .unwrap_or("")
                .to_string()
        };
        let rp = get("relative_path");
        if rp.is_empty() {
            continue;
        }
        out.push(ManifestEntry {
            relative_path: rp,
            axis_class: get("axis_class"),
            source: get("source"),
        });
    }
    Ok(out)
}

fn feature_value_str(
    analysis: &zenanalyze::feature::AnalysisResults,
    f: AnalysisFeature,
) -> String {
    if let Some(v) = analysis.get_f32(f) {
        if v.is_nan() {
            return String::new();
        }
        return format!("{v:.6}");
    }
    if let Some(v) = analysis.get(f) {
        match v {
            FeatureValue::F32(x) => {
                if x.is_nan() {
                    String::new()
                } else {
                    format!("{x:.6}")
                }
            }
            FeatureValue::U32(x) => format!("{x}"),
            FeatureValue::Bool(b) => format!("{}", b as u8),
            _ => String::new(),
        }
    } else {
        String::new()
    }
}

fn synth_sha(rel: &str) -> String {
    // Deterministic 16-hex id for joinability, derived from FxHash-ish
    // running u64 hash of bytes. This is NOT a cryptographic hash;
    // it's just a stable identifier for the join column.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in rel.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    let mut h2: u64 = 0xa07d_b321_77ec_d56b;
    for b in rel.as_bytes().iter().rev() {
        h2 ^= *b as u64;
        h2 = h2.wrapping_mul(0xff51_afd7_ed55_8ccd);
    }
    format!("{:016x}{:016x}", h, h2)
}

fn main() -> ExitCode {
    let raw: Vec<String> = env::args().collect();
    let mut manifest_path: Option<PathBuf> = None;
    let mut corpus_root: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut iter = raw.iter().skip(1);
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--manifest" => manifest_path = iter.next().map(PathBuf::from),
            "--corpus-root" => corpus_root = iter.next().map(PathBuf::from),
            "--output" => output = iter.next().map(PathBuf::from),
            other => {
                eprintln!("unknown arg {other}");
                return ExitCode::from(2);
            }
        }
    }
    let manifest_path = match manifest_path {
        Some(p) => p,
        None => {
            eprintln!("--manifest PATH required");
            return ExitCode::from(2);
        }
    };
    let corpus_root = corpus_root
        .unwrap_or_else(|| PathBuf::from("/mnt/v/output/codec-corpus-2026-05-01-multiaxis"));
    let output = match output {
        Some(p) => p,
        None => {
            eprintln!("--output PATH required");
            return ExitCode::from(2);
        }
    };

    let entries = match read_manifest(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    eprintln!("manifest: {} entries", entries.len());

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let cols: Vec<AnalysisFeature> = FeatureSet::SUPPORTED.iter().collect();
    eprintln!("extracting {} features per image", cols.len());

    let f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&output)
        .unwrap_or_else(|e| panic!("open {}: {e}", output.display()));
    let mut w = BufWriter::new(f);
    write!(
        w,
        "image_path\timage_sha\tsplit\tcontent_class\tsource\tsize_class\twidth\theight"
    )
    .unwrap();
    for c in &cols {
        write!(w, "\tfeat_{}", c.name()).unwrap();
    }
    writeln!(w).unwrap();

    let query = AnalysisQuery::new(FeatureSet::SUPPORTED);
    let mut done = 0usize;
    let mut failed = 0usize;
    for (idx, e) in entries.iter().enumerate() {
        let path = corpus_root.join(&e.relative_path);
        let dyn_img = match ImageReader::open(&path).and_then(|r| Ok(r.decode())) {
            Ok(Ok(img)) => img,
            _ => {
                eprintln!("skip (decode fail): {}", path.display());
                failed += 1;
                continue;
            }
        };
        let (rw, rh) = dyn_img.dimensions();
        let rgb8 = dyn_img.to_rgb8();
        let row = analyze_features_rgb8(rgb8.as_raw(), rw, rh, &query);

        let image_path = format!("multiaxis:{}", e.relative_path);
        let sha = synth_sha(&e.relative_path);
        write!(
            w,
            "{}\t{}\ttrain\t{}\t{}\t{}\t{}\t{}",
            image_path, sha, e.axis_class, e.source, e.axis_class, rw, rh
        )
        .ok();
        for c in &cols {
            write!(w, "\t{}", feature_value_str(&row, *c)).ok();
        }
        writeln!(w).ok();
        done += 1;
        if (idx + 1) % 25 == 0 {
            w.flush().ok();
            eprintln!("[{}/{}] done={done} failed={failed}", idx + 1, entries.len());
        }
    }
    w.flush().ok();
    eprintln!("final: done={done} failed={failed}");
    ExitCode::from(0)
}
