// Copyright (c) Imazen LLC.
// Licensed under AGPL-3.0-or-later. Commercial licenses at https://www.imazen.io/pricing
//
//! Empirical validation of the curated sweep axes (`zenjxl::sweep`).
//!
//! Encodes the default stratum plus every single-deviation stratum of
//! [`SweepAxes::modes_full`] on a small mixed corpus (CID22-512 photos,
//! synthetic noise / complex / checkerboard, a confirmed screenshot-class
//! synthetic — see [`generate_screenshot`] — and one 64×64 tiny) and
//! checks:
//!
//! 1. **Fingerprint contract** — equal fingerprint ⇒ byte-identical
//!    output, on real encodes of the documented alias pairs (the
//!    generic-quality calibration plateau, quality-vs-distance
//!    spellings, the `gather_dedup_phase3` exclusion, the
//!    `tree_parallel_*` scheduling-only claim), plus distinct-
//!    fingerprint negative controls.
//! 2. **No inert step** — every curated axis step changes output bytes
//!    vs its mode's default stratum somewhere in the subset, and the
//!    within-axis probe pairs are mutually distinct.
//! 3. **Lossless exactness** — every lossless cell decodes back to the
//!    exact input bytes (zero tolerance; a mismatch is a shipping bug).
//! 4. **Documented directions** — bytes monotone in quality, e9 ≤ e5
//!    mean bytes, faster_decoding costs bytes (soft checks: reported,
//!    non-fatal).
//! 5. **Queue ordering invariants** on the emitted plan.
//! 6. **ssim2 sanity floor** at q85 (catches corrupt pixel paths).
//! 7. **Alpha leg** (class-conditional axes, playbook pattern 10): the
//!    dev≤1 subset of [`SweepAxes::modes_full_alpha`] on an RGBA corpus
//!    (a sprite with a noisy transparent background + soft alpha edges,
//!    and a CID22 photo under a soft radial alpha mask) with the
//!    two-sided check — every alpha step must change bytes ON the alpha
//!    class AND leave every RGB corpus image (fed as `Rgb8`) byte-
//!    identical to the default — plus alpha-aware lossless exactness
//!    (alpha exact everywhere; RGB exact where alpha > 0 for `zeroinv`,
//!    everywhere otherwise) and a composite-over-grey ssim2 floor.
//!    Results land in `<out>_alpha.tsv`.
//!
//! Build-config caveat: this harness runs without jxl-encoder's
//! `parallel` or `butteraugli-loop` features, so the `tree_parallel_*`
//! byte-pairs prove the sequential build only (the `parallel`-build
//! thread-invariance is checked by `tests/parallel_determinism.rs`,
//! which also documents the one known thread-DEPENDENT default —
//! `SectionedTrees::Auto` at lossless e≤7), and `lossy_search_seeds` is
//! structurally dead (and therefore not a curated probe).
//!
//! Run (the corpus env var is required when the repo layout doesn't put
//! `codec-eval/codec-corpus` next to the workspace root — no silent
//! fallback, the harness panics loudly without a corpus):
//! ```bash
//! CODEC_CORPUS_DIR=$HOME/work/codec-eval/codec-corpus \
//! GIT_COMMIT=$(git rev-parse --short HEAD) cargo run --release \
//!   --example sweep_validate --features __expert -- \
//!   --out benchmarks/sweep_validate_$(date +%F).tsv
//! ```
//!
//! Exit code is non-zero on any hard failure (contract violation,
//! inert step, lossless roundtrip mismatch, ordering breakage, encode
//! error).

use std::collections::HashMap;
use std::io::Write as _;

use imgref::ImgVec;
use rgb::{ComponentBytes, RGB8};
use zenjpeg_bench_utils::{
    RgbImage, codec_corpus_dir, generate_checkerboard, generate_complex, generate_noise,
    generate_photo_like, load_png,
};
use zenjxl::sweep::{
    AlphaCoding, BuiltConfig, LosslessVariant, LossyVariant, NamedLosslessParams, NamedLossyParams,
    QualityGrid, SweepAxes, SweepBuilder, SweepVariant, fingerprint, resolve_distance_for_quality,
};
use zenjxl::{
    EncoderMode, EncoderStrategy, LosslessConfig, LosslessInternalParams, PixelLayout,
    ProgressiveMode,
};

const DEFAULT_LOSSY_BASE: &str = "vd-e7_zen_def";
const DEFAULT_LOSSLESS_BASE: &str = "mod-e7_def";
const Q_GRID: [f32; 6] = [10.0, 30.0, 50.0, 70.0, 85.0, 95.0];

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn image_bytes(img: &RgbImage) -> &[u8] {
    assert_eq!(img.stride(), img.width(), "harness expects tight buffers");
    img.buf().as_bytes()
}

/// Deterministic synthetic screenshot (512×512 in the harness): flat UI
/// chrome (title bar, sidebar, content background), text-like glyph runs
/// on a 24-px line pitch, bordered flat buttons, and one embedded
/// photo-like thumbnail. It is screenshot-class by jxl-encoder's own
/// W44-164 discriminator (`flat_color_block_ratio >= 0.35`), which is
/// what trips Zenjxl's per-image content gates — so the `lean`
/// (LeanFaster-vs-Zenjxl) stratum is hard-checked for inertness like
/// every other step, instead of soft-exempted. The byte-for-byte
/// twin of this generator lives in `tests/lean_strategy_diverges.rs`,
/// which asserts the class membership and the divergence at every
/// `Q_GRID` point; keep the two in sync.
fn generate_screenshot(width: u32, height: u32) -> RgbImage {
    let (w, h) = (width as usize, height as usize);
    let mut px = vec![RGB8::new(0, 0, 0); w * h];
    let mut put = |x: usize, y: usize, c: [u8; 3]| {
        px[y * w + x] = RGB8::new(c[0], c[1], c[2]);
    };
    let bg = [240, 240, 240];
    let bar = [40, 44, 52];
    let side = [250, 250, 250];
    let ink = [20, 20, 20];
    let accent = [30, 110, 220];
    for y in 0..h {
        for x in 0..w {
            let c = if y < 40 {
                bar
            } else if x < 128 {
                side
            } else {
                bg
            };
            put(x, y, c);
        }
    }
    for item in 0..6 {
        let y0 = 64 + item * 48;
        for y in y0..y0 + 28 {
            for x in 12..116 {
                let edge = y == y0 || y == y0 + 27 || x == 12 || x == 115;
                put(x, y, if edge { ink } else { accent });
            }
        }
    }
    let mut state: u32 = 0x5EED_1234;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state >> 16
    };
    let mut line = 0;
    while 56 + line * 24 + 16 <= h - 8 {
        let y0 = 56 + line * 24 + 8;
        let x_end = 144 + (next() as usize % 300) + 40;
        let mut x = 144;
        while x < x_end.min(w - 8) {
            let run = 1 + (next() as usize % 3);
            let gap = 1 + (next() as usize % 4);
            let colour = if next() % 7 == 0 { accent } else { ink };
            for dx in 0..run {
                for dy in 0..8 {
                    if (next() % 5) != 0 {
                        put(x + dx, y0 + dy, colour);
                    }
                }
            }
            x += run + gap;
        }
        line += 1;
    }
    for y in 300..460 {
        for x in 320..480 {
            let gx = ((x - 320) * 255 / 159) as u8;
            let gy = ((y - 300) * 255 / 159) as u8;
            let n = (next() & 0x1F) as u8;
            put(
                x,
                y,
                [
                    gx.wrapping_add(n),
                    gy.wrapping_add(n / 2),
                    (255 - gx / 2).wrapping_sub(n),
                ],
            );
        }
    }
    ImgVec::new(px, w, h)
}

/// Deterministic RGBA sprite (512×512 in the harness): fully transparent
/// background whose RGB samples are LCG noise (so `keepinv` / `zeroinv`
/// have something to preserve or discard), three shaded discs with 24-px
/// soft alpha edges (a non-constant alpha plane, so upstream's squeeze
/// path engages), and fully-opaque interiors. Byte-for-byte twin of
/// `sprite_rgba8` in `tests/alpha_axes_class_conditional.rs`; keep the two
/// in sync.
fn generate_sprite_rgba(width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut px = vec![0u8; w * h * 4];
    let mut state: u32 = 0xA1FA_5EED;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state >> 16
    };
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let mut rgb = [
                (next() & 0xFF) as u8,
                (next() & 0xFF) as u8,
                (next() & 0xFF) as u8,
            ];
            let mut a = 0u8;
            for (cx, cy, r, col) in [
                (160.0f32, 160.0f32, 110.0f32, [220u8, 60, 40]),
                (350.0, 200.0, 90.0, [40, 180, 90]),
                (260.0, 380.0, 120.0, [50, 90, 230]),
            ] {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if d < r + 12.0 {
                    let cov = ((r + 12.0 - d) / 24.0).clamp(0.0, 1.0);
                    let shade = (255.0 - d / r * 90.0).clamp(0.0, 255.0) as u8;
                    rgb = [
                        (u16::from(col[0]) * u16::from(shade) / 255) as u8,
                        (u16::from(col[1]) * u16::from(shade) / 255) as u8,
                        (u16::from(col[2]) * u16::from(shade) / 255) as u8,
                    ];
                    a = a.max((cov * 255.0) as u8);
                }
            }
            px[i..i + 3].copy_from_slice(&rgb);
            px[i + 3] = a;
        }
    }
    px
}

/// A real photo under a soft radial alpha mask: opaque inside 35 % of
/// the short side, a 40-px linear fade, transparent outside. The RGB is
/// the untouched photo everywhere, so the invisible-pixel knobs act on
/// real content, not synthetic noise.
fn photo_with_alpha_mask(img: &RgbImage) -> Vec<u8> {
    let (w, h) = (img.width(), img.height());
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let r_in = 0.35 * w.min(h) as f32;
    let mut px = Vec::with_capacity(w * h * 4);
    for (i, p) in img.buf().iter().enumerate() {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        let a = ((r_in + 40.0 - d) / 40.0).clamp(0.0, 1.0);
        px.extend_from_slice(&[p.r, p.g, p.b, (a * 255.0).round() as u8]);
    }
    px
}

/// Encode through the built config, threads pinned to 1 (byte
/// determinism is the whole point here).
fn encode(cfg: &BuiltConfig, img: &RgbImage) -> Vec<u8> {
    let (w, h) = (img.width() as u32, img.height() as u32);
    encode_layout(cfg, image_bytes(img), w, h, PixelLayout::Rgb8)
}

fn encode_layout(cfg: &BuiltConfig, px: &[u8], w: u32, h: u32, layout: PixelLayout) -> Vec<u8> {
    match cfg {
        BuiltConfig::Lossy(c) => c.clone().with_threads(1).encode(px, w, h, layout),
        BuiltConfig::Lossless(c) => c.clone().with_threads(1).encode(px, w, h, layout),
    }
    .unwrap_or_else(|e| panic!("encode failed: {e:?}"))
}

fn decode_rgba8(jxl: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let out = zenjxl::decode(jxl, None, &[zenpixels::PixelDescriptor::RGBA8]).ok()?;
    let (w, h) = (out.info.width, out.info.height);
    let px = out.pixels.into_vec();
    (px.len() == (w as usize) * (h as usize) * 4).then_some((px, w, h))
}

/// Composite packed RGBA8 over mid-grey (128) — the alpha-aware form of
/// the pixel comparison: what a viewer shows, with alpha folded in.
fn composite_over_grey(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = u32::from(p[3]);
            let mix = |c: u8| ((u32::from(c) * a + 128 * (255 - a) + 127) / 255) as u8;
            [mix(p[0]), mix(p[1]), mix(p[2])]
        })
        .collect()
}

/// ssim2 of the grey-composited decode vs the grey-composited source,
/// plus the alpha-plane mean absolute error. NaN / None on decode failure.
fn score_rgba(src: &[u8], w: usize, h: usize, jxl: &[u8]) -> (f64, Option<f64>) {
    use fast_ssim2::{LinearRgbImage, compute_ssimulacra2, srgb_u8_to_linear};
    let Some((dec, dw, dh)) = decode_rgba8(jxl) else {
        return (f64::NAN, None);
    };
    if (dw as usize, dh as usize) != (w, h) {
        return (f64::NAN, None);
    }
    let to_linear = |bytes: &[u8]| {
        let px: Vec<[f32; 3]> = bytes
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| {
                [
                    srgb_u8_to_linear(c[0]),
                    srgb_u8_to_linear(c[1]),
                    srgb_u8_to_linear(c[2]),
                ]
            })
            .collect();
        LinearRgbImage::new(px, w, h)
    };
    let s = compute_ssimulacra2(
        to_linear(&composite_over_grey(src)),
        to_linear(&composite_over_grey(&dec)),
    )
    .unwrap_or(f64::NAN);
    let mae = src
        .iter()
        .skip(3)
        .step_by(4)
        .zip(dec.iter().skip(3).step_by(4))
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / (w * h) as f64;
    (s, Some(mae))
}

/// The class-conditional alpha leg (module docs item 7). Pushes hard
/// failures / warnings into the caller's lists; writes `<out>_alpha.tsv`.
fn alpha_leg(
    rgb_images: &[(String, RgbImage)],
    out_path: &str,
    hard_failures: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    println!("\n=== alpha leg (modes_full_alpha, dev<=1) ===");
    // On-class corpus: (name, packed RGBA8, w, h).
    let mut alpha_images: Vec<(String, Vec<u8>, u32, u32)> = Vec::new();
    alpha_images.push(("sprite512".into(), generate_sprite_rgba(512, 512), 512, 512));
    if let Some((name, photo)) = rgb_images.iter().find(|(n, _)| n.starts_with("cid_")) {
        alpha_images.push((
            format!("{name}_amask"),
            photo_with_alpha_mask(photo),
            photo.width() as u32,
            photo.height() as u32,
        ));
    }

    let plan = SweepBuilder::new(
        SweepAxes::modes_full_alpha(),
        QualityGrid::ExplicitQuality(Q_GRID.to_vec()),
    )
    .plan();
    let subset: Vec<usize> = plan
        .cells
        .iter()
        .enumerate()
        .take_while(|(_, c)| c.deviations <= 1)
        .map(|(i, _)| i)
        .collect();
    let mut resolve: HashMap<String, usize> = HashMap::new();
    for (i, c) in plan.cells.iter().enumerate() {
        resolve.insert(c.id.clone(), i);
        for a in &c.aliases {
            resolve.insert(a.clone(), i);
        }
    }
    let is_alpha_step = |base: &str| {
        let (_, label) = parse_label(base);
        ALPHA_LABELS.contains(&label.as_str())
    };
    // Cells to run off-class: the defaults plus every alpha-step cell.
    let off_cells: Vec<usize> = subset
        .iter()
        .copied()
        .filter(|&ci| {
            let (base, _) = split_q(&plan.cells[ci].id);
            plan.cells[ci].deviations == 0 || is_alpha_step(base)
        })
        .collect();
    println!(
        "alpha plan: {} cells, {} in the dev<=1 subset, {} run off-class x {} RGB images",
        plan.cells.len(),
        subset.len(),
        off_cells.len(),
        rgb_images.len()
    );

    struct AMeasure {
        bytes: usize,
        hash: u64,
        ssim2: f64,
        alpha_mae: Option<f64>,
    }
    let t0 = std::time::Instant::now();
    let mut on: HashMap<(usize, usize), AMeasure> = HashMap::new();
    for (ii, (iname, px, w, h)) in alpha_images.iter().enumerate() {
        for &ci in &subset {
            let cell = &plan.cells[ci];
            let jxl = encode_layout(&cell.build(), px, *w, *h, PixelLayout::Rgba8);
            let (ssim2, alpha_mae) = score_rgba(px, *w as usize, *h as usize, &jxl);
            if let SweepVariant::Lossless(v) = &cell.variant {
                let zero_invisible = v.zero_invisible;
                match decode_rgba8(&jxl) {
                    Some((dec, dw, dh)) => {
                        let mut bad = (dw, dh) != (*w, *h);
                        if !bad {
                            for (s, d) in px.as_chunks::<4>().0.iter().zip(dec.as_chunks::<4>().0) {
                                if s[3] != d[3]
                                    || ((s[3] != 0 || !zero_invisible) && s[..3] != d[..3])
                                {
                                    bad = true;
                                    break;
                                }
                            }
                        }
                        if bad {
                            hard_failures.push(format!(
                                "LOSSLESS RGBA ROUNDTRIP MISMATCH: {} on {iname}{}",
                                cell.id,
                                if zero_invisible {
                                    " (alpha plane / visible RGB)"
                                } else {
                                    ""
                                }
                            ));
                        }
                    }
                    None => hard_failures.push(format!(
                        "lossless RGBA decode failed: {} on {iname}",
                        cell.id
                    )),
                }
            }
            on.insert(
                (ci, ii),
                AMeasure {
                    bytes: jxl.len(),
                    hash: fnv64(&jxl),
                    ssim2,
                    alpha_mae,
                },
            );
        }
        println!("  encoded {} alpha cells on {iname}", subset.len());
    }
    // Off-class: every alpha-step cell must be byte-identical to its
    // default on Rgb8 input (no coupling into the colour path).
    let mut off_checked = 0usize;
    for (iname, img) in rgb_images {
        let mut def: HashMap<String, u64> = HashMap::new();
        for &ci in &off_cells {
            let cell = &plan.cells[ci];
            let jxl = encode(&cell.build(), img);
            let (base, q) = split_q(&cell.id);
            let key = if base.starts_with("vd-") {
                format!("vd_{q}")
            } else {
                "mod".to_string()
            };
            let h = fnv64(&jxl);
            if cell.deviations == 0 {
                def.insert(key, h);
            } else {
                off_checked += 1;
                match def.get(&key) {
                    Some(&d) if d == h => {}
                    Some(_) => hard_failures.push(format!(
                        "CLASS COUPLING: {} changed Rgb8 (no-alpha) output on {iname}",
                        cell.id
                    )),
                    None => hard_failures.push(format!(
                        "alpha leg: default cell for {} not encoded before it on {iname}",
                        cell.id
                    )),
                }
            }
        }
    }
    println!(
        "alpha encode+score: {:.1}s ({off_checked} off-class identity checks)",
        t0.elapsed().as_secs_f64()
    );

    // TSV.
    let alpha_out = match out_path.strip_suffix(".tsv") {
        Some(stem) => format!("{stem}_alpha.tsv"),
        None => format!("{out_path}_alpha.tsv"),
    };
    let mut tsv = std::fs::File::create(&alpha_out).expect("alpha tsv create");
    writeln!(
        tsv,
        "# sweep_validate (zenjxl) alpha leg: modes_full_alpha dev<=1 subset, q={Q_GRID:?}\n# git_commit: {}\n# images: {}\n# ssim2 is on the mid-grey composite; alpha_mae is the alpha-plane MAE (8-bit)",
        std::env::var("GIT_COMMIT").unwrap_or_else(|_| "unknown".into()),
        alpha_images
            .iter()
            .map(|(n, _, w, h)| format!("{n}({w}x{h})"))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    writeln!(
        tsv,
        "image\tbase_id\tlabel\tdeviations\tq\tbytes\tssim2\talpha_mae\tfingerprint\tbytes_fnv"
    )
    .unwrap();
    for (ii, (iname, _, _, _)) in alpha_images.iter().enumerate() {
        for &ci in &subset {
            let c = &plan.cells[ci];
            let m = &on[&(ci, ii)];
            let (base, q) = split_q(&c.id);
            let (_, label) = parse_label(base);
            writeln!(
                tsv,
                "{iname}\t{base}\t{label}\t{}\t{q}\t{}\t{:.3}\t{}\t{:016x}\t{:016x}",
                c.deviations,
                m.bytes,
                m.ssim2,
                m.alpha_mae
                    .map_or_else(|| "nan".to_string(), |v| format!("{v:.4}")),
                c.fingerprint,
                m.hash
            )
            .unwrap();
        }
    }
    println!("wrote {alpha_out}");

    // On-class liveness per alpha label, plus the per-label table.
    println!(
        "\n{:<12} {:>5} {:>6} {:>9} {:>9} {:>9} {:>8} {:>9}",
        "alpha label", "n", "diff%", "dsize%", "min", "max", "dssim2", "alpha_mae"
    );
    for label in ALPHA_LABELS {
        let lossy = !label.starts_with("zeroinv");
        let base = if lossy {
            format!("{DEFAULT_LOSSY_BASE}-{label}")
        } else {
            format!("{DEFAULT_LOSSLESS_BASE}-{label}")
        };
        let (mut n, mut differing) = (0usize, 0usize);
        let (mut dsum, mut dmin, mut dmax) = (0f64, f64::INFINITY, f64::NEG_INFINITY);
        let (mut ssum, mut sn) = (0f64, 0usize);
        let (mut msum, mut mn) = (0f64, 0usize);
        for (ii, _) in alpha_images.iter().enumerate() {
            let pairs: Vec<(String, String)> = if lossy {
                Q_GRID
                    .iter()
                    .map(|q| (format!("{base}_q{q}"), format!("{DEFAULT_LOSSY_BASE}_q{q}")))
                    .collect()
            } else {
                vec![(base.clone(), DEFAULT_LOSSLESS_BASE.to_string())]
            };
            for (id, def_id) in pairs {
                let (Some(&ci), Some(&di)) = (resolve.get(&id), resolve.get(&def_id)) else {
                    hard_failures.push(format!("alpha leg: {id} / {def_id} missing from plan"));
                    continue;
                };
                let (m, b) = (&on[&(ci, ii)], &on[&(di, ii)]);
                n += 1;
                if m.hash != b.hash {
                    differing += 1;
                }
                let d = (m.bytes as f64 - b.bytes as f64) / b.bytes as f64 * 100.0;
                dsum += d;
                dmin = dmin.min(d);
                dmax = dmax.max(d);
                if m.ssim2.is_finite() && b.ssim2.is_finite() {
                    ssum += m.ssim2 - b.ssim2;
                    sn += 1;
                }
                if let Some(mae) = m.alpha_mae {
                    msum += mae;
                    mn += 1;
                }
            }
        }
        println!(
            "{:<12} {:>5} {:>5.0}% {:>8.2}% {:>8.2}% {:>8.2}% {:>+8.2} {:>9.4}",
            label,
            n,
            differing as f64 / n.max(1) as f64 * 100.0,
            dsum / n.max(1) as f64,
            dmin,
            dmax,
            ssum / sn.max(1) as f64,
            msum / mn.max(1) as f64
        );
        if n == 0 {
            hard_failures.push(format!("alpha leg: no cells for alpha label {label}"));
        } else if differing == 0 {
            hard_failures.push(format!(
                "INERT ALPHA STEP: {label} never changed output bytes across {n} on-class (image,q) pairs"
            ));
        }
    }
    // Documented directions (soft): squeeze beats no-squeeze at the same
    // distance on non-constant alpha (upstream: −30..−56 %); keep_invisible
    // costs bytes (noise under transparent pixels is coded).
    let mean_on = |base: &str| -> f64 {
        let mut sum = 0f64;
        let mut n = 0usize;
        for (ii, _) in alpha_images.iter().enumerate() {
            for &q in &Q_GRID {
                if let Some(&ci) = resolve.get(&format!("{base}_q{q}")) {
                    sum += on[&(ci, ii)].bytes as f64;
                    n += 1;
                }
            }
        }
        sum / n.max(1) as f64
    };
    for (what, a, b) in [
        (
            "alpha squeeze d2 beats no-squeeze d2 (mean bytes)",
            mean_on(&format!("{DEFAULT_LOSSY_BASE}-asq2")),
            mean_on(&format!("{DEFAULT_LOSSY_BASE}-ad2")),
        ),
        (
            "alpha d10 beats d2 (mean bytes)",
            mean_on(&format!("{DEFAULT_LOSSY_BASE}-ad10")),
            mean_on(&format!("{DEFAULT_LOSSY_BASE}-ad2")),
        ),
        (
            "keep_invisible costs bytes vs default (mean)",
            mean_on(DEFAULT_LOSSY_BASE),
            mean_on(&format!("{DEFAULT_LOSSY_BASE}-keepinv")),
        ),
    ] {
        if a < b {
            println!("PASS direction: {what} ({a:.0} < {b:.0})");
        } else {
            warnings.push(format!("alpha direction: {what} FAILED ({a:.0} >= {b:.0})"));
        }
    }
    // Composite ssim2 floor at q85 on the on-class default stratum: the
    // decode→composite path must not be corrupt.
    for (ii, (iname, _, _, _)) in alpha_images.iter().enumerate() {
        for &ci in &subset {
            let c = &plan.cells[ci];
            if c.quality != Some(85.0) {
                continue;
            }
            let s = on[&(ci, ii)].ssim2;
            if !s.is_finite() || s < 30.0 {
                hard_failures.push(format!(
                    "alpha ssim2 sanity: {} on {iname} scored {s:.1} at q85 (composite floor 30)",
                    c.id
                ));
            }
        }
    }
}

/// The alpha-class step labels the leg gates (lossy `ad2`/`ad10`/`asq2`/
/// `asq10`/`keepinv`, lossless `zeroinv`) — must match
/// `SweepAxes::modes_full_alpha`'s dev-1 alpha strata (the test
/// `alpha_axes_carry_no_neutral_spelling` pins the preset side).
const ALPHA_LABELS: [&str; 6] = ["ad2", "ad10", "asq2", "asq10", "keepinv", "zeroinv"];

fn decode_rgb8(jxl: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let out = zenjxl::decode(jxl, None, &[zenpixels::PixelDescriptor::RGB8]).ok()?;
    let (w, h) = (out.info.width, out.info.height);
    Some((out.pixels.into_vec(), w, h))
}

fn ssim2(orig: &RgbImage, jxl: &[u8]) -> f64 {
    use fast_ssim2::{LinearRgbImage, compute_ssimulacra2, srgb_u8_to_linear};
    let Some((pixels, w, h)) = decode_rgb8(jxl) else {
        return f64::NAN;
    };
    if (w as usize, h as usize) != (orig.width(), orig.height()) {
        return f64::NAN;
    }
    let to_linear = |bytes: &[u8]| {
        let px: Vec<[f32; 3]> = bytes
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| {
                [
                    srgb_u8_to_linear(c[0]),
                    srgb_u8_to_linear(c[1]),
                    srgb_u8_to_linear(c[2]),
                ]
            })
            .collect();
        LinearRgbImage::new(px, orig.width(), orig.height())
    };
    compute_ssimulacra2(to_linear(image_bytes(orig)), to_linear(&pixels)).unwrap_or(f64::NAN)
}

/// Strip the `_q…`/`_d…` suffix from a lossy id; lossless ids pass
/// through. Returns (base_id, q-token or "-").
fn split_q(id: &str) -> (&str, &str) {
    if id.starts_with("mod-") {
        return (id, "-");
    }
    let at = id.rfind("_q").or_else(|| id.rfind("_d"));
    let at = at.unwrap_or_else(|| panic!("lossy cell id must end in _q<q>/_d<d>: {id}"));
    (&id[..at], &id[at + 2..])
}

/// Diff a base id against its mode's default tokens. Returns
/// (deviation count, '+'-joined deviating labels).
///
/// Lossy ids are `vd-e{eff}_{strategy}_{internal[-flags…]}`; lossless
/// `mod-e{eff}_{internal[-flags…]}`. The effort and strategy tokens are
/// one axis each; the trailing token carries the internal-probe label
/// plus '-'-joined flags, each its own axis.
fn parse_label(base: &str) -> (usize, String) {
    let (def, n_fixed): (&str, usize) = if base.starts_with("vd-") {
        (DEFAULT_LOSSY_BASE, 3)
    } else {
        (DEFAULT_LOSSLESS_BASE, 2)
    };
    let def_tok: Vec<&str> = def.splitn(n_fixed, '_').collect();
    let got: Vec<&str> = base.splitn(n_fixed, '_').collect();
    assert_eq!(got.len(), n_fixed, "unparseable id {base}");
    let mut devs = Vec::new();
    for (d, g) in def_tok.iter().zip(&got).take(n_fixed - 1) {
        if d != g {
            devs.push((*g).to_string());
        }
    }
    let mut tail = got[n_fixed - 1].split('-');
    let internal = tail.next().unwrap_or_default();
    if internal != def_tok[n_fixed - 1] {
        devs.push(internal.to_string());
    }
    for flag in tail {
        devs.push(flag.to_string());
    }
    (devs.len(), devs.join("+"))
}

struct Measure {
    bytes: usize,
    hash: u64,
    ssim2: f64,
}

fn main() {
    let out_path = {
        let args: Vec<String> = std::env::args().collect();
        args.iter()
            .position(|a| a == "--out")
            .and_then(|i| args.get(i + 1).cloned())
            .unwrap_or_else(|| "benchmarks/sweep_validate.tsv".to_string())
    };
    let mut hard_failures: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // ------------------------------------------------------------------
    // Corpus: 3 CID22-512 photos + 3 synthetic 512s + one 64×64 tiny.
    // ------------------------------------------------------------------
    let mut images: Vec<(String, RgbImage)> = Vec::new();
    let cid_dir = codec_corpus_dir()
        .expect(
            "codec corpus not found — set CODEC_CORPUS_DIR (e.g. ~/work/codec-eval/codec-corpus)",
        )
        .join("CID22/CID22-512/validation");
    let mut cid: Vec<_> = std::fs::read_dir(&cid_dir)
        .expect("CID22-512/validation missing")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    cid.sort();
    for p in cid.iter().take(3) {
        let name = format!("cid_{}", p.file_stem().unwrap().to_string_lossy());
        images.push((name, load_png(p).expect("png load")));
    }
    images.push(("noise512".into(), generate_noise(512, 512, 42)));
    images.push(("complex512".into(), generate_complex(512, 512)));
    images.push(("checker512".into(), generate_checkerboard(512, 512, 8)));
    images.push(("screenshot512".into(), generate_screenshot(512, 512)));
    images.push(("tiny64".into(), generate_photo_like(64, 64)));

    // ------------------------------------------------------------------
    // Plan + ordering invariants.
    // ------------------------------------------------------------------
    let plan = SweepBuilder::new(
        SweepAxes::modes_full(),
        QualityGrid::ExplicitQuality(Q_GRID.to_vec()),
    )
    .plan();
    println!(
        "plan: {} cells, {} merged aliases, {} invalid strata",
        plan.cells.len(),
        plan.duplicates_merged,
        plan.invalid_skipped.len()
    );
    if plan.cells[0].deviations != 0 || !plan.cells[0].id.starts_with(DEFAULT_LOSSY_BASE) {
        hard_failures.push(format!(
            "ordering: first cell is not the lossy default stratum ({})",
            plan.cells[0].id
        ));
    }
    if !plan
        .cells
        .iter()
        .any(|c| c.deviations == 0 && c.id.starts_with(DEFAULT_LOSSLESS_BASE))
    {
        hard_failures.push("ordering: lossless default stratum missing at deviation 0".into());
    }
    if plan
        .cells
        .windows(2)
        .any(|w| w[1].deviations < w[0].deviations)
    {
        hard_failures.push("ordering: deviations not non-decreasing".into());
    }
    {
        let mut seen = std::collections::HashSet::new();
        for c in &plan.cells {
            for id in std::iter::once(&c.id).chain(c.aliases.iter()) {
                if !seen.insert(id.clone()) {
                    hard_failures.push(format!("duplicate cell id {id}"));
                }
            }
        }
    }

    // dev≤1 prefix (sorted ⇒ contiguous), plus id → canonical-index map
    // covering aliases so merged spellings resolve to their encoder.
    let subset: Vec<usize> = plan
        .cells
        .iter()
        .enumerate()
        .take_while(|(_, c)| c.deviations <= 1)
        .map(|(i, _)| i)
        .collect();
    let mut resolve: HashMap<String, usize> = HashMap::new();
    for (i, c) in plan.cells.iter().enumerate() {
        resolve.insert(c.id.clone(), i);
        for a in &c.aliases {
            resolve.insert(a.clone(), i);
        }
    }
    println!(
        "subset: {} canonical cells (dev<=1) x {} images",
        subset.len(),
        images.len()
    );
    for &ci in &subset {
        let c = &plan.cells[ci];
        let (base, _) = split_q(&c.id);
        let (n, _) = parse_label(base);
        if n != c.deviations as usize {
            hard_failures.push(format!(
                "id/deviation mismatch: {} parses to {n} deviations, cell says {}",
                c.id, c.deviations
            ));
        }
    }

    // ------------------------------------------------------------------
    // Encode the subset. Lossless cells additionally roundtrip-decode:
    // decoded pixels must equal the input EXACTLY (zero tolerance).
    // ------------------------------------------------------------------
    let t0 = std::time::Instant::now();
    let mut measures: HashMap<(usize, usize), Measure> = HashMap::new();
    for (ii, (iname, img)) in images.iter().enumerate() {
        for &ci in &subset {
            let cell = &plan.cells[ci];
            let built = cell.build();
            let jxl = encode(&built, img);
            let score = ssim2(img, &jxl);
            if matches!(cell.variant, SweepVariant::Lossless(_)) {
                match decode_rgb8(&jxl) {
                    Some((px, w, h)) => {
                        if (w as usize, h as usize) != (img.width(), img.height())
                            || px != image_bytes(img)
                        {
                            hard_failures.push(format!(
                                "LOSSLESS ROUNDTRIP MISMATCH: {} on {iname}",
                                cell.id
                            ));
                        }
                    }
                    None => hard_failures
                        .push(format!("lossless decode failed: {} on {iname}", cell.id)),
                }
            }
            measures.insert(
                (ci, ii),
                Measure {
                    bytes: jxl.len(),
                    hash: fnv64(&jxl),
                    ssim2: score,
                },
            );
        }
        println!("  encoded {} cells on {iname}", subset.len());
    }
    println!("encode+score: {:.1}s", t0.elapsed().as_secs_f64());

    // ------------------------------------------------------------------
    // TSV.
    // ------------------------------------------------------------------
    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut tsv = std::fs::File::create(&out_path).expect("tsv create");
    writeln!(
        tsv,
        "# sweep_validate (zenjxl): modes_full dev<=1 subset, q={Q_GRID:?}\n# git_commit: {}\n# images: {}",
        std::env::var("GIT_COMMIT").unwrap_or_else(|_| "unknown".into()),
        images
            .iter()
            .map(|(n, i)| format!("{n}({}x{})", i.width(), i.height()))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    writeln!(
        tsv,
        "image\tbase_id\tlabel\tdeviations\tq\tbytes\tssim2\tfingerprint\tbytes_fnv"
    )
    .unwrap();
    for (ii, (iname, _)) in images.iter().enumerate() {
        for &ci in &subset {
            let c = &plan.cells[ci];
            let m = &measures[&(ci, ii)];
            let (base, q) = split_q(&c.id);
            let (_, label) = parse_label(base);
            writeln!(
                tsv,
                "{iname}\t{base}\t{label}\t{}\t{q}\t{}\t{:.3}\t{:016x}\t{:016x}",
                c.deviations, m.bytes, m.ssim2, c.fingerprint, m.hash
            )
            .unwrap();
        }
    }
    println!("wrote {out_path}");

    // ------------------------------------------------------------------
    // Per-label aggregates vs each mode's default stratum.
    // ------------------------------------------------------------------
    let lossy_baseline = |ii: usize, q: f32| -> &Measure {
        let id = format!("{DEFAULT_LOSSY_BASE}_q{q}");
        &measures[&(resolve[&id], ii)]
    };
    let lossless_baseline =
        |ii: usize| -> &Measure { &measures[&(resolve[DEFAULT_LOSSLESS_BASE], ii)] };

    // Collect every dev-1 base id present in the subset (canonical or
    // alias).
    let mut bases: Vec<String> = Vec::new();
    for &ci in &subset {
        let c = &plan.cells[ci];
        for id in std::iter::once(&c.id).chain(c.aliases.iter()) {
            let (base, _) = split_q(id);
            let (n, _) = parse_label(base);
            if n == 1 && !bases.iter().any(|b| b == base) {
                bases.push(base.to_string());
            }
        }
    }
    struct Agg {
        label: String,
        n: usize,
        differing: usize,
        dsize_sum: f64,
        dsize_min: f64,
        dsize_max: f64,
        dssim_sum: f64,
        dssim_n: usize,
    }
    let mut aggs: Vec<Agg> = Vec::new();
    for base in &bases {
        let (_, label) = parse_label(base);
        let lossy = base.starts_with("vd-");
        let mut a = Agg {
            label: label.clone(),
            n: 0,
            differing: 0,
            dsize_sum: 0.0,
            dsize_min: f64::INFINITY,
            dsize_max: f64::NEG_INFINITY,
            dssim_sum: 0.0,
            dssim_n: 0,
        };
        let record = |m: &Measure, b: &Measure, a: &mut Agg| {
            a.n += 1;
            if m.hash != b.hash {
                a.differing += 1;
            }
            let d = (m.bytes as f64 - b.bytes as f64) / b.bytes as f64 * 100.0;
            a.dsize_sum += d;
            a.dsize_min = a.dsize_min.min(d);
            a.dsize_max = a.dsize_max.max(d);
            if m.ssim2.is_finite() && b.ssim2.is_finite() {
                a.dssim_sum += m.ssim2 - b.ssim2;
                a.dssim_n += 1;
            }
        };
        for (ii, _) in images.iter().enumerate() {
            if lossy {
                for &q in &Q_GRID {
                    let id = format!("{base}_q{q}");
                    let Some(&ci) = resolve.get(&id) else {
                        continue;
                    };
                    let Some(m) = measures.get(&(ci, ii)) else {
                        hard_failures.push(format!(
                            "dev-1 spelling {id} resolved to un-encoded cell {} (dev {})",
                            plan.cells[ci].id, plan.cells[ci].deviations
                        ));
                        continue;
                    };
                    record(m, lossy_baseline(ii, q), &mut a);
                }
            } else {
                let Some(&ci) = resolve.get(base.as_str()) else {
                    continue;
                };
                let Some(m) = measures.get(&(ci, ii)) else {
                    hard_failures.push(format!(
                        "dev-1 spelling {base} resolved to un-encoded cell {}",
                        plan.cells[ci].id
                    ));
                    continue;
                };
                record(m, lossless_baseline(ii), &mut a);
            }
        }
        aggs.push(a);
    }
    aggs.sort_by(|x, y| x.label.cmp(&y.label));
    println!(
        "\n{:<22} {:>5} {:>6} {:>9} {:>9} {:>9} {:>8}",
        "label", "n", "diff%", "dsize%", "min", "max", "dssim2"
    );
    for a in &aggs {
        println!(
            "{:<22} {:>5} {:>5.0}% {:>8.2}% {:>8.2}% {:>8.2}% {:>+8.2}",
            a.label,
            a.n,
            a.differing as f64 / a.n as f64 * 100.0,
            a.dsize_sum / a.n as f64,
            a.dsize_min,
            a.dsize_max,
            a.dssim_sum / a.dssim_n.max(1) as f64
        );
    }

    // Hard inert check: every curated axis step must change bytes
    // somewhere. "lean" used to be soft-exempted (the LeanFaster bundle
    // only diverges from Zenjxl on content that trips the per-image
    // gates); `screenshot512` is a confirmed gate-tripping image
    // (`tests/lean_strategy_diverges.rs` proves class membership by
    // upstream's discriminator and divergence at every Q_GRID point), so
    // byte-identity everywhere is now inertness for `lean` too.
    for a in &aggs {
        if a.differing == 0 {
            hard_failures.push(format!(
                "INERT STEP: {} never changed output bytes across {} (image,q) pairs",
                a.label, a.n
            ));
        }
    }

    // ------------------------------------------------------------------
    // Within-axis probe distinctness (must differ somewhere).
    // ------------------------------------------------------------------
    let pair_differs = |base_a: &str, base_b: &str| -> (bool, usize) {
        let mut n = 0;
        let mut differs = false;
        let lossy = base_a.starts_with("vd-");
        for (ii, _) in images.iter().enumerate() {
            let mut check = |ka: &str, kb: &str| {
                if let (Some(&ca), Some(&cb)) = (resolve.get(ka), resolve.get(kb)) {
                    n += 1;
                    if measures[&(ca, ii)].hash != measures[&(cb, ii)].hash {
                        differs = true;
                    }
                }
            };
            if lossy {
                for &q in &Q_GRID {
                    check(&format!("{base_a}_q{q}"), &format!("{base_b}_q{q}"));
                }
            } else {
                check(base_a, base_b);
            }
        }
        (differs, n)
    };
    let must_differ = [
        (
            "mod-e7_def-pred6",
            "mod-e7_def-pred0",
            "predictor Weighted(6) vs Zero(0)",
        ),
        (
            "mod-e7_def-gss0",
            "mod-e7_def-gss3",
            "group size 128 vs 1024",
        ),
        (
            "vd-e7_zen_dct16off",
            "vd-e7_zen_dct64off",
            "dct16 vs dct64 suppression",
        ),
        ("vd-e7_zen_def-epf0", "vd-e7_zen_def-epf3", "epf 0 vs 3"),
        (
            "vd-e7_zen_def-prog1",
            "vd-e7_zen_def-prog2",
            "progressive 2-pass vs 3-pass",
        ),
    ];
    println!();
    for (a, b, what) in must_differ {
        let (differs, n) = pair_differs(a, b);
        if n == 0 {
            hard_failures.push(format!("probe pair missing from plan: {what} ({a} / {b})"));
        } else if differs {
            println!("PASS distinct: {what} (over {n} pairs)");
        } else {
            hard_failures.push(format!(
                "INERT PROBE: {what} byte-identical across all {n} (image,q) pairs"
            ));
        }
    }

    // ------------------------------------------------------------------
    // Fingerprint contract on real encodes.
    // ------------------------------------------------------------------
    let photo = &images[0].1;
    let lossy_variant = |distance: f32| LossyVariant {
        distance,
        effort: 7,
        strategy: EncoderStrategy::Zenjxl,
        encoder_mode: EncoderMode::Reference,
        internal: NamedLossyParams::default_probe(),
        gaborish: None,
        epf_level: -1,
        progressive: ProgressiveMode::Single,
        noise: false,
        faster_decoding: 0,
        ans: None,
        alpha: AlphaCoding::Lossless,
        keep_invisible: false,
    };
    let lossless_variant = |params: LosslessInternalParams| LosslessVariant {
        effort: 7,
        encoder_mode: EncoderMode::Reference,
        internal: NamedLosslessParams::new("pair", params),
        predictor: None,
        group_size_shift: None,
        faster_decoding: 0,
        palette_colors: None,
        zero_invisible: false,
    };
    let byte_pair = |what: &str,
                     a: &SweepVariant,
                     b: &SweepVariant,
                     img: &RgbImage,
                     expect_equal: bool,
                     hard_failures: &mut Vec<String>| {
        let fa = fingerprint(a);
        let fb = fingerprint(b);
        let ea = encode(&a.build(), img);
        let eb = encode(&b.build(), img);
        let bytes_equal = ea == eb;
        if expect_equal {
            if fa != fb {
                hard_failures.push(format!("{what}: fingerprints differ but should alias"));
            }
            if bytes_equal {
                println!("PASS alias bytes: {what}");
            } else {
                hard_failures.push(format!(
                    "FINGERPRINT CONTRACT VIOLATION: {what} — equal fingerprint, bytes differ ({} vs {} bytes)",
                    ea.len(),
                    eb.len()
                ));
            }
        } else {
            if fa == fb {
                hard_failures.push(format!("{what}: fingerprints equal but should differ"));
            }
            if bytes_equal {
                hard_failures.push(format!("{what}: control pair produced identical bytes"));
            } else {
                println!("PASS control distinct: {what}");
            }
        }
    };
    println!();
    // 1. The generic-quality calibration plateau: q10 and q20 resolve to
    //    the same distance — the lowest five step-5 grid points are one
    //    encode.
    byte_pair(
        "calibration plateau q10 vs q20",
        &SweepVariant::Lossy(lossy_variant(resolve_distance_for_quality(10.0))),
        &SweepVariant::Lossy(lossy_variant(resolve_distance_for_quality(20.0))),
        photo,
        true,
        &mut hard_failures,
    );
    // 2. Quality-vs-distance spellings of the same resolved distance.
    //    (Same construction path here by necessity — the real assertion
    //    is that the resolved distance IS the alias key, proven by the
    //    plateau pair above and the unit test that the two grid
    //    spellings fingerprint equal.)
    byte_pair(
        "q85 spelling vs explicit distance spelling",
        &SweepVariant::Lossy(lossy_variant(resolve_distance_for_quality(85.0))),
        &SweepVariant::Lossy(lossy_variant(zenjxl::quality_to_distance(
            zenjxl::calibrated_jxl_quality(85.0),
        ))),
        photo,
        true,
        &mut hard_failures,
    );
    // 3. gather_dedup_phase3 exclusion: byte-neutral with gather_dedup
    //    off (inert prerequisite) AND with it on (table implementation
    //    only).
    let mut p_off = LosslessInternalParams::default();
    p_off.gather_dedup_phase3 = Some(true);
    byte_pair(
        "gather_dedup_phase3 on vs default (gather_dedup off)",
        &SweepVariant::Lossless(lossless_variant(LosslessInternalParams::default())),
        &SweepVariant::Lossless(lossless_variant(p_off)),
        photo,
        true,
        &mut hard_failures,
    );
    let mut p_g = LosslessInternalParams::default();
    p_g.gather_dedup = Some(true);
    let mut p_g3 = p_g.clone();
    p_g3.gather_dedup_phase3 = Some(true);
    byte_pair(
        "gather_dedup_phase3 on vs off (gather_dedup on)",
        &SweepVariant::Lossless(lossless_variant(p_g.clone())),
        &SweepVariant::Lossless(lossless_variant(p_g3)),
        photo,
        true,
        &mut hard_failures,
    );
    // 4. tree_parallel_* scheduling-only claim (sequential build: the
    //    small-image fallback pair forces the parallel code path on a
    //    sub-1MP input vs the auto-sequential default).
    let mut p_par = LosslessInternalParams::default();
    p_par.tree_parallel_small_image_fallback = Some(false);
    byte_pair(
        "tree_parallel_small_image_fallback force-parallel vs auto",
        &SweepVariant::Lossless(lossless_variant(LosslessInternalParams::default())),
        &SweepVariant::Lossless(lossless_variant(p_par)),
        photo,
        true,
        &mut hard_failures,
    );
    let mut p_depth = LosslessInternalParams::default();
    p_depth.tree_parallel_max_depth = Some(2);
    byte_pair(
        "tree_parallel_max_depth 2 vs default",
        &SweepVariant::Lossless(lossless_variant(LosslessInternalParams::default())),
        &SweepVariant::Lossless(lossless_variant(p_depth)),
        photo,
        true,
        &mut hard_failures,
    );
    // 5. smart_fanout (config-level, not a sweep axis): upstream
    //    documents it bitstream-equivalent.
    {
        let a = LosslessConfig::new().with_effort(8).with_threads(1);
        let b = a.clone().with_smart_fanout(true);
        let (w, h) = (photo.width() as u32, photo.height() as u32);
        let ea = a
            .encode(image_bytes(photo), w, h, PixelLayout::Rgb8)
            .unwrap();
        let eb = b
            .encode(image_bytes(photo), w, h, PixelLayout::Rgb8)
            .unwrap();
        if ea == eb {
            println!("PASS alias bytes: smart_fanout on vs off @e8");
        } else {
            hard_failures.push(format!(
                "smart_fanout changed bytes ({} vs {}) — upstream bitstream-equivalence claim falsified on this build",
                ea.len(),
                eb.len()
            ));
        }
    }
    // 6. Negative controls: distinct fingerprints, distinct bytes.
    byte_pair(
        "negative control: e5 vs e9 lossy @q85",
        &SweepVariant::Lossy(LossyVariant {
            effort: 5,
            ..lossy_variant(resolve_distance_for_quality(85.0))
        }),
        &SweepVariant::Lossy(LossyVariant {
            effort: 9,
            ..lossy_variant(resolve_distance_for_quality(85.0))
        }),
        photo,
        false,
        &mut hard_failures,
    );
    let mut p_rct1 = LosslessInternalParams::default();
    p_rct1.nb_rcts_to_try = Some(1);
    byte_pair(
        "negative control: rct1 vs default lossless (photo content)",
        &SweepVariant::Lossless(lossless_variant(LosslessInternalParams::default())),
        &SweepVariant::Lossless(lossless_variant(p_rct1)),
        photo,
        false,
        &mut hard_failures,
    );

    // ------------------------------------------------------------------
    // Soft direction checks (512px images only, mean bytes per label).
    // ------------------------------------------------------------------
    let mean_bytes = |base: &str| -> f64 {
        let mut sum = 0f64;
        let mut n = 0usize;
        let lossy = base.starts_with("vd-");
        for (ii, (iname, _)) in images.iter().enumerate() {
            if iname == "tiny64" {
                continue;
            }
            if lossy {
                for &q in &Q_GRID {
                    if let Some(&ci) = resolve.get(&format!("{base}_q{q}")) {
                        sum += measures[&(ci, ii)].bytes as f64;
                        n += 1;
                    }
                }
            } else if let Some(&ci) = resolve.get(base) {
                sum += measures[&(ci, ii)].bytes as f64;
                n += 1;
            }
        }
        sum / n.max(1) as f64
    };
    let mean_bytes_at_q = |base: &str, q: f32| -> f64 {
        let mut sum = 0f64;
        let mut n = 0usize;
        for (ii, (iname, _)) in images.iter().enumerate() {
            if iname == "tiny64" {
                continue;
            }
            if let Some(&ci) = resolve.get(&format!("{base}_q{q}")) {
                sum += measures[&(ci, ii)].bytes as f64;
                n += 1;
            }
        }
        sum / n.max(1) as f64
    };
    println!();
    // Bytes monotone in quality on the default stratum (plateau makes
    // q10 == q20-resolved cells, so start at q30).
    let q_sizes: Vec<f64> = [30.0, 50.0, 70.0, 85.0, 95.0]
        .iter()
        .map(|&q| mean_bytes_at_q(DEFAULT_LOSSY_BASE, q))
        .collect();
    if q_sizes.windows(2).all(|w| w[0] < w[1]) {
        println!("PASS direction: default-stratum bytes strictly monotone in q (q30..q95)");
    } else {
        warnings.push(format!("bytes not monotone in q: {q_sizes:?}"));
    }
    let checks = [
        (
            "lossy e9 compresses better than e5 (mean)",
            mean_bytes("vd-e9_zen_def"),
            mean_bytes("vd-e5_zen_def"),
        ),
        (
            "lossless e9 compresses better than e5 (mean)",
            mean_bytes("mod-e9_def"),
            mean_bytes("mod-e5_def"),
        ),
        (
            "faster_decoding 4 costs bytes vs default (mean)",
            mean_bytes(DEFAULT_LOSSY_BASE),
            mean_bytes("vd-e7_zen_def-fd4"),
        ),
    ];
    for (what, a, b) in checks {
        if a < b {
            println!("PASS direction: {what} ({a:.0} < {b:.0})");
        } else {
            warnings.push(format!("direction: {what} FAILED ({a:.0} >= {b:.0})"));
        }
    }

    // ssim2 sanity floor at q85 on 512px content: catches corrupt pixel
    // paths. Pure noise legitimately scores low (incompressible); its
    // floor exists for corruption, not quality.
    for (ii, (iname, _)) in images.iter().enumerate() {
        if iname == "tiny64" {
            continue;
        }
        let floor = if iname == "noise512" { 15.0 } else { 30.0 };
        for &ci in &subset {
            let c = &plan.cells[ci];
            if c.quality != Some(85.0) {
                continue;
            }
            let s = measures[&(ci, ii)].ssim2;
            if !s.is_finite() || s < floor {
                hard_failures.push(format!(
                    "ssim2 sanity: {} on {iname} scored {s:.1} at q85 (floor {floor})",
                    c.id
                ));
            }
        }
    }

    // ------------------------------------------------------------------
    // Alpha leg (class-conditional axes).
    // ------------------------------------------------------------------
    alpha_leg(&images, &out_path, &mut hard_failures, &mut warnings);

    // ------------------------------------------------------------------
    // Verdict.
    // ------------------------------------------------------------------
    println!();
    for w in &warnings {
        println!("WARN {w}");
    }
    if hard_failures.is_empty() {
        println!("\nALL HARD CHECKS PASSED ({} warnings)", warnings.len());
    } else {
        println!("\n{} HARD FAILURES:", hard_failures.len());
        for f in &hard_failures {
            println!("FAIL {f}");
        }
        std::process::exit(1);
    }
}
