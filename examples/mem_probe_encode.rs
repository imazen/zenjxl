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
//!   w h pixels mode effort quality threads out_bytes pre_rss_kb vmhwm_kb
//!   marginal_kb encode_ms alloc_count peak_live_kb
//!
//! `alloc_count` (allocations during the encode call) and `peak_live_kb` (the
//! high-water mark of LIVE allocated bytes) come from the probe's counting
//! global allocator. They are the two numbers the memory work is actually
//! judged on:
//!   * `peak_live_kb` is ALLOCATOR-AGNOSTIC. `vmhwm_kb` / peak RSS include
//!     whatever this platform's malloc declined to return to the OS, so they
//!     move with the allocator; `peak_live_kb` does not.
//!   * `alloc_count` guards the Windows path, where allocation is slow enough
//!     that trading bytes for many more small allocations regresses overall.
//! On non-Linux hosts `pre_rss_kb`/`vmhwm_kb` read 0 (they use /proc); wrap the
//! binary in `/usr/bin/time -l` for peak RSS there.
//!
//! `encode_ms` is the wall time of the `encode()` call only (file read +
//! process startup excluded), so it is comparable to the `est` row's
//! predicted `time_ms` — the time half of the per-effort estimate validation.
//!
//! Per-site allocation profiling (see `alloc_sites` below):
//!   JXL_ALLOC_SITES=1          enable (off by default — profiled runs count
//!                              the profiler's own maps in peak_live/alloc_count)
//!   JXL_ALLOC_SITE_MIN=65536   only track allocations >= this many bytes
//!   JXL_ALLOC_SNAP_STEP=8388608  re-snapshot per-site live on this much peak growth
//!   JXL_ALLOC_SITES_OUT=path   write full resolved stacks per site

use std::hint::black_box;

use jxl_encoder::{LosslessConfig, LossyConfig, PixelLayout};

/// Counting allocator: tracks allocation COUNT and the high-water mark of
/// LIVE bytes.
///
/// Peak RSS answers "how big does the process get on THIS allocator" — it
/// folds in whatever the platform's malloc chose not to return to the OS, so
/// it moves when the allocator changes even if the encoder does not.
/// `peak_live` is the allocator-agnostic number: the largest total the encoder
/// ever had outstanding. A change that lowers RSS but not `peak_live` only
/// flattered this platform's retention policy; a change that lowers
/// `peak_live` is a real reduction on glibc, libmalloc, jemalloc and mimalloc
/// alike.
///
/// `count` is tracked because allocation *count* is its own cost — Windows'
/// allocator is slow enough that trading bytes for many more small
/// allocations is a net regression there. Any buffer change must report both.
mod counting_alloc {
    use core::alloc::{GlobalAlloc, Layout};
    use core::sync::atomic::{AtomicUsize, Ordering};

    pub static COUNT: AtomicUsize = AtomicUsize::new(0);
    pub static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub static PEAK_LIVE: AtomicUsize = AtomicUsize::new(0);
    /// Size of the single allocation that last pushed `PEAK_LIVE` to a new
    /// high. Identifies the transient responsible for the peak, which
    /// `malloc_history` cannot show (it only samples LIVE allocations, so a
    /// short-lived spike is invisible to it).
    pub static PEAK_TRIGGER: AtomicUsize = AtomicUsize::new(0);
    /// Whether the peak was set by a realloc rather than a fresh alloc. A
    /// realloc transiently holds old+new, so a realloc-triggered peak means the
    /// fix is to pre-size the buffer, not to shrink it.
    pub static PEAK_FROM_REALLOC: AtomicUsize = AtomicUsize::new(0);
    /// `JXL_PEAK_TRACE_AT=<bytes>`: capture and print ONE backtrace the first
    /// time live bytes cross this threshold.
    ///
    /// Attribution taken from an RSS-polled `malloc_history` snapshot is
    /// unreliable — RSS crosses a threshold at a different instant than
    /// `PEAK_LIVE` is set, so the snapshot shows a nearby moment, not the peak.
    /// That error sent this work after the wrong allocation for several rounds.
    /// Run once to learn the peak, then set this just below it to get the stack
    /// AT the peak.
    pub static TRACE_AT: AtomicUsize = AtomicUsize::new(0);
    pub static TRACED: AtomicUsize = AtomicUsize::new(0);

    pub struct Counting;

    impl Counting {
        fn record_alloc(ptr: *mut u8, size: usize) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
            // Track BEFORE the peak check so a peak-setting allocation is in
            // the site map when the snapshot fires.
            crate::alloc_sites::track_alloc(ptr, size);
            // Monotonic max. Racy under contention by at most one concurrent
            // delta, which is irrelevant at the magnitudes we report.
            let prev = PEAK_LIVE.fetch_max(live, Ordering::Relaxed);
            if live > prev {
                PEAK_TRIGGER.store(size, Ordering::Relaxed);
                PEAK_FROM_REALLOC.store(0, Ordering::Relaxed);
                crate::alloc_sites::maybe_snapshot(live);
            }
            Self::maybe_trace(live, size);
        }

        /// Print one backtrace when live bytes first cross `TRACE_AT`.
        /// Guarded so the capture itself (which allocates) cannot recurse.
        fn maybe_trace(live: usize, size: usize) {
            let at = TRACE_AT.load(Ordering::Relaxed);
            if at == 0 || live < at {
                return;
            }
            if TRACED.swap(1, Ordering::Relaxed) != 0 {
                return;
            }
            let bt = std::backtrace::Backtrace::force_capture();
            eprintln!("[peak-trace] live={live} triggered_by={size}\n{bt}");
        }
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { std::alloc::System.alloc(layout) };
            if !p.is_null() {
                Self::record_alloc(p, layout.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { std::alloc::System.alloc_zeroed(layout) };
            if !p.is_null() {
                Self::record_alloc(p, layout.size());
            }
            p
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            crate::alloc_sites::track_free(ptr, layout.size());
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let p = unsafe { std::alloc::System.realloc(ptr, layout, new_size) };
            if !p.is_null() {
                // A realloc is one allocator round-trip, and between the two
                // sizes the allocator may hold both — model the worst case so
                // growth-by-realloc shows up in the peak rather than hiding.
                COUNT.fetch_add(1, Ordering::Relaxed);
                let live = LIVE.fetch_add(new_size, Ordering::Relaxed) + new_size;
                crate::alloc_sites::track_realloc(ptr, layout.size(), p, new_size);
                let prev = PEAK_LIVE.fetch_max(live, Ordering::Relaxed);
                if live > prev {
                    PEAK_TRIGGER.store(new_size, Ordering::Relaxed);
                    PEAK_FROM_REALLOC.store(1, Ordering::Relaxed);
                    crate::alloc_sites::maybe_snapshot(live);
                }
                Self::maybe_trace(live, new_size);
                LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            }
            p
        }
    }
}

/// Per-site allocation profiler (`JXL_ALLOC_SITES=1`): for every allocation of
/// at least `JXL_ALLOC_SITE_MIN` bytes (default 64 KiB), capture the raw call
/// stack (unresolved instruction pointers — ~1-2 us, no symbolization) and
/// aggregate per unique stack: total bytes ever allocated, allocation count,
/// live bytes, and the site's own live high-water.
///
/// Whenever the global live high-water rises by `JXL_ALLOC_SNAP_STEP` (default
/// 8 MiB) past the last snapshot, the per-site live map is snapshotted — so at
/// exit we hold the per-site composition AT (within one step of) the peak
/// instant. That is the number that answers "which code line owns the peak",
/// which neither total-churn profiles (heaptrack's default view) nor
/// RSS-polled `malloc_history` snapshots answer: the former counts bytes that
/// were never simultaneously live, the latter samples the wrong instant.
///
/// Symbolization happens once, at exit. Attribution picks the innermost frame
/// that lands in jxl-encoder/zenjxl source (inlined frames are expanded, so a
/// user function inlined into rayon plumbing still attributes correctly).
/// `JXL_ALLOC_SITES_OUT=<path>` additionally writes full resolved stacks per
/// site.
///
/// The profiler's own maps allocate through the same global allocator (guarded
/// against recursion, but still COUNTED), so a profiled run's `peak_live` /
/// `alloc_count` are a few MB / few hundred above a clean run's — take
/// canonical numbers from runs without `JXL_ALLOC_SITES`.
mod alloc_sites {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::sync::Mutex;

    pub const MAX_FRAMES: usize = 26;

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SiteKey {
        len: u8,
        frames: [usize; MAX_FRAMES],
    }

    #[derive(Clone, Copy, Default)]
    pub struct SiteStats {
        pub total: u64,
        pub count: u64,
        pub live: i64,
        pub live_max: i64,
    }

    #[derive(Default)]
    struct Prof {
        sites: HashMap<SiteKey, SiteStats>,
        /// tracked pointer -> (site, size); entries only for allocations >= min.
        ptrs: HashMap<usize, (SiteKey, usize)>,
        /// (global live bytes, per-site live) at the highest snapshot so far.
        snap: Option<(usize, Vec<(SiteKey, i64)>)>,
    }

    pub static ENABLED: AtomicUsize = AtomicUsize::new(0);
    pub static SITE_MIN: AtomicUsize = AtomicUsize::new(64 * 1024);
    pub static SNAP_STEP: AtomicUsize = AtomicUsize::new(8 * 1024 * 1024);
    static SNAP_AT: AtomicUsize = AtomicUsize::new(0);
    static PROF: Mutex<Option<Prof>> = Mutex::new(None);

    thread_local! {
        /// Recursion guard: the tracker's own map/snapshot allocations re-enter
        /// the global allocator; with the guard set they are counted globally
        /// but not tracked per-site.
        static GUARD: Cell<bool> = const { Cell::new(false) };
    }

    #[inline]
    pub fn enabled() -> bool {
        ENABLED.load(Ordering::Relaxed) != 0
    }
    #[inline]
    fn site_min() -> usize {
        SITE_MIN.load(Ordering::Relaxed)
    }

    /// Capture raw frame IPs. No symbolization, no allocation on the steady
    /// path. Innermost frames first; the allocator's own frames are skipped at
    /// resolve time by symbol filter (inlining makes skip-counts unreliable).
    fn capture() -> SiteKey {
        let mut key = SiteKey {
            len: 0,
            frames: [0; MAX_FRAMES],
        };
        backtrace::trace(|frame| {
            let i = key.len as usize;
            if i >= MAX_FRAMES {
                return false;
            }
            key.frames[i] = frame.ip() as usize;
            key.len += 1;
            true
        });
        key
    }

    fn with_prof(f: impl FnOnce(&mut Prof)) {
        let mut lock = PROF.lock().unwrap_or_else(|e| e.into_inner());
        f(lock.get_or_insert_with(Prof::default));
    }

    pub fn track_alloc(ptr: *mut u8, size: usize) {
        if !enabled() || size < site_min() {
            return;
        }
        GUARD.with(|g| {
            if g.get() {
                return;
            }
            g.set(true);
            let key = capture();
            with_prof(|p| {
                let s = p.sites.entry(key).or_default();
                s.total += size as u64;
                s.count += 1;
                s.live += size as i64;
                s.live_max = s.live_max.max(s.live);
                p.ptrs.insert(ptr as usize, (key, size));
            });
            g.set(false);
        });
    }

    pub fn track_free(ptr: *mut u8, size: usize) {
        if !enabled() || size < site_min() {
            return;
        }
        GUARD.with(|g| {
            if g.get() {
                return;
            }
            g.set(true);
            with_prof(|p| {
                if let Some((key, sz)) = p.ptrs.remove(&(ptr as usize)) {
                    if let Some(s) = p.sites.get_mut(&key) {
                        s.live -= sz as i64;
                    }
                }
            });
            g.set(false);
        });
    }

    /// Realloc = free(old) + alloc(new) attributed to the realloc call site
    /// (the growing vec's push/reserve line). If old == new (in-place) the map
    /// entry is replaced. Tracking happens after the system realloc, so a
    /// same-address reuse by another thread in that window can momentarily
    /// mis-attribute one buffer — acceptable for a measurement tool.
    pub fn track_realloc(old_ptr: *mut u8, old_size: usize, new_ptr: *mut u8, new_size: usize) {
        let min = site_min();
        if !enabled() || (old_size < min && new_size < min) {
            return;
        }
        GUARD.with(|g| {
            if g.get() {
                return;
            }
            g.set(true);
            let key = (new_size >= min).then(capture);
            with_prof(|p| {
                if old_size >= min {
                    if let Some((k, sz)) = p.ptrs.remove(&(old_ptr as usize)) {
                        if let Some(s) = p.sites.get_mut(&k) {
                            s.live -= sz as i64;
                        }
                    }
                }
                if let Some(key) = key {
                    let s = p.sites.entry(key).or_default();
                    s.total += new_size as u64;
                    s.count += 1;
                    s.live += new_size as i64;
                    s.live_max = s.live_max.max(s.live);
                    p.ptrs.insert(new_ptr as usize, (key, new_size));
                }
            });
            g.set(false);
        });
    }

    /// Snapshot the per-site live map when the global high-water has risen a
    /// full step past the last snapshot. Called only on peak raises, so after
    /// warmup it fires rarely; a peak set by a >= step allocation snapshots at
    /// exactly the peak instant (the triggering site is inserted first).
    pub fn maybe_snapshot(live: usize) {
        if !enabled() {
            return;
        }
        let at = SNAP_AT.load(Ordering::Relaxed);
        if live < at.saturating_add(SNAP_STEP.load(Ordering::Relaxed)) {
            return;
        }
        GUARD.with(|g| {
            if g.get() {
                return;
            }
            g.set(true);
            SNAP_AT.store(live, Ordering::Relaxed);
            with_prof(|p| {
                let v: Vec<(SiteKey, i64)> = p
                    .sites
                    .iter()
                    .filter(|(_, s)| s.live > 0)
                    .map(|(k, s)| (*k, s.live))
                    .collect();
                p.snap = Some((live, v));
            });
            g.set(false);
        });
    }

    // ---- exit-time symbolization + report ----

    #[derive(Clone, Default)]
    struct RFrame {
        sym: String,
        file: String,
        line: u32,
    }

    fn resolve_ip(cache: &mut HashMap<usize, Vec<RFrame>>, ip: usize) -> Vec<RFrame> {
        if let Some(v) = cache.get(&ip) {
            return v.clone();
        }
        let mut out = Vec::new();
        // resolve() expands inlined frames: one ip can yield several logical
        // frames, innermost first — this is what keeps attribution working in
        // release builds where user code inlines into rayon plumbing.
        backtrace::resolve(ip as *mut core::ffi::c_void, |sym| {
            let mut f = RFrame::default();
            if let Some(n) = sym.name() {
                f.sym = strip_hash(&n.to_string());
            }
            if let Some(p) = sym.filename() {
                f.file = p.display().to_string();
            }
            f.line = sym.lineno().unwrap_or(0);
            out.push(f);
        });
        cache.insert(ip, out.clone());
        out
    }

    /// Strip mangling noise: legacy `::h<16 hex>` suffixes and v0 `[hash]`
    /// crate-disambiguator brackets (`jxl_encoder[10a2...]::` -> `jxl_encoder::`).
    fn strip_hash(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'[' {
                if let Some(j) = s[i + 1..].find(']') {
                    let inner = &s[i + 1..i + 1 + j];
                    if (8..=17).contains(&inner.len())
                        && inner.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        i += j + 2;
                        continue;
                    }
                }
            }
            out.push(b[i] as char);
            i += 1;
        }
        if let Some(i) = out.rfind("::h") {
            if out.len() - i == 19 && out[i + 3..].chars().all(|c| c.is_ascii_hexdigit()) {
                out.truncate(i);
            }
        }
        out
    }

    fn short_file(f: &str) -> String {
        for marker in ["/work/zen/", "/registry/src/"] {
            if let Some(i) = f.find(marker) {
                return f[i + marker.len()..].to_string();
            }
        }
        if let Some(i) = f.find("/rustc/") {
            let rest = &f[i + 7..];
            return match rest.find('/') {
                Some(j) => format!("rust:{}", &rest[j + 1..]),
                None => rest.to_string(),
            };
        }
        f.to_string()
    }

    /// Does this frame's function BELONG TO one of our crates? Checks the
    /// symbol's own crate-path prefix (v0-demangled, hash brackets stripped),
    /// not `contains` — `<alloc::vec::Vec<jxl_encoder::...::Channel>>::clone`
    /// names our type but is alloc's frame; the caller is the line we want.
    fn is_ours(fr: &RFrame) -> bool {
        let s = fr.sym.trim_start_matches('<');
        ["jxl_encoder::", "zenjxl::", "jxl::"]
            .iter()
            .any(|p| s.starts_with(p))
            || (fr.file.contains("jxl-encoder/") && !fr.file.contains("registry/src"))
    }

    fn is_noise(fr: &RFrame) -> bool {
        const NOISE_PREFIX: &[&str] = &[
            "alloc::",
            "core::",
            "std::",
            "hashbrown",
            "backtrace",
            "mem_probe_encode",
            "__",
            "_rjem",
        ];
        let s = fr.sym.trim_start_matches('<');
        s.is_empty() || NOISE_PREFIX.iter().any(|n| s.starts_with(n))
    }

    /// The frame a site is attributed to: innermost frame whose function is in
    /// jxl-encoder/zenjxl; else the innermost non-noise frame.
    fn attribute(frames: &[RFrame]) -> RFrame {
        frames
            .iter()
            .find(|fr| is_ours(fr))
            .or_else(|| frames.iter().find(|fr| !is_noise(fr)))
            .or_else(|| frames.first())
            .cloned()
            .unwrap_or_default()
    }

    fn short_sym(s: &str) -> String {
        let segs: Vec<&str> = s.split("::").collect();
        if segs.len() <= 4 {
            s.to_string()
        } else {
            segs[segs.len() - 4..].join("::")
        }
    }

    fn mb(b: i64) -> f64 {
        b as f64 / (1024.0 * 1024.0)
    }

    /// Symbolize + print the report. stderr gets the two ranked by-line tables
    /// (live-at-peak-snapshot and total churn); `out` gets full per-site
    /// resolved stacks.
    pub fn report(out: Option<&str>) {
        if !enabled() {
            return;
        }
        GUARD.with(|g| g.set(true));
        let (sites, snap) = {
            let mut lock = PROF.lock().unwrap_or_else(|e| e.into_inner());
            match lock.as_mut() {
                Some(p) => (
                    p.sites.iter().map(|(k, s)| (*k, *s)).collect::<Vec<_>>(),
                    p.snap.take(),
                ),
                None => (Vec::new(), None),
            }
        };
        let mut cache: HashMap<usize, Vec<RFrame>> = HashMap::new();
        let mut resolved: HashMap<SiteKey, Vec<RFrame>> = HashMap::new();
        let resolve_key = |key: &SiteKey, cache: &mut HashMap<usize, Vec<RFrame>>| {
            let mut frames = Vec::new();
            for &ip in &key.frames[..key.len as usize] {
                frames.extend(resolve_ip(cache, ip));
            }
            frames
        };

        // by-line aggregation of the snapshot (live at peak) and of totals.
        let line_of = |key: &SiteKey,
                           cache: &mut HashMap<usize, Vec<RFrame>>,
                           resolved: &mut HashMap<SiteKey, Vec<RFrame>>| {
            let frames = resolved
                .entry(*key)
                .or_insert_with(|| resolve_key(key, cache))
                .clone();
            let a = attribute(&frames);
            if a.file.is_empty() {
                short_sym(&a.sym)
            } else {
                format!("{}:{} {}", short_file(&a.file), a.line, short_sym(&a.sym))
            }
        };

        let mut at_peak: HashMap<String, (i64, u64)> = HashMap::new(); // live, count
        let (snap_live, snap_sites) = snap.unwrap_or((0, Vec::new()));
        for (key, live) in &snap_sites {
            let line = line_of(key, &mut cache, &mut resolved);
            let e = at_peak.entry(line).or_default();
            e.0 += live;
            e.1 += 1;
        }
        let mut churn: HashMap<String, (u64, u64)> = HashMap::new(); // total, count
        for (key, s) in &sites {
            let line = line_of(key, &mut cache, &mut resolved);
            let e = churn.entry(line).or_default();
            e.0 += s.total;
            e.1 += s.count;
        }

        if std::env::var("JXL_ALLOC_SITES_DEBUG").is_ok() {
            let mut ss = snap_sites.clone();
            ss.sort_by_key(|(_, l)| -*l);
            for (key, live) in ss.iter().take(3) {
                eprintln!("[sites-debug] site live={:.1} MiB len={} frames:", mb(*live), key.len);
                for &ip in &key.frames[..key.len as usize] {
                    let fr = resolve_ip(&mut cache, ip);
                    if fr.is_empty() {
                        eprintln!("    {ip:#x} <unresolved>");
                    } else {
                        for f in fr {
                            eprintln!("    {ip:#x} {} ({}:{})", f.sym, short_file(&f.file), f.line);
                        }
                    }
                }
            }
        }

        let tracked_at_peak: i64 = snap_sites.iter().map(|(_, l)| l).sum();
        eprintln!(
            "[sites] snapshot: global_live={:.1} MiB, tracked={:.1} MiB ({:.1}%), \
             small/untracked={:.1} MiB, {} sites, min_size={} B",
            mb(snap_live as i64),
            mb(tracked_at_peak),
            100.0 * tracked_at_peak as f64 / (snap_live as f64).max(1.0),
            mb(snap_live as i64 - tracked_at_peak),
            snap_sites.len(),
            SITE_MIN.load(Ordering::Relaxed),
        );

        let mut peak_rows: Vec<(&String, &(i64, u64))> = at_peak.iter().collect();
        peak_rows.sort_by_key(|(_, (l, _))| -*l);
        eprintln!("[sites] live at peak snapshot, by attributed line:");
        for (i, (line, (live, n))) in peak_rows.iter().take(30).enumerate() {
            eprintln!("  {:>2}  {:>9.1} MiB  n={:<5} {}", i + 1, mb(*live), n, line);
        }

        let mut churn_rows: Vec<(&String, &(u64, u64))> = churn.iter().collect();
        churn_rows.sort_by_key(|(_, (t, _))| std::cmp::Reverse(*t));
        eprintln!("[sites] total allocated over run (churn), by attributed line:");
        for (i, (line, (total, n))) in churn_rows.iter().take(30).enumerate() {
            eprintln!(
                "  {:>2}  {:>9.1} MiB  n={:<7} {}",
                i + 1,
                mb(*total as i64),
                n,
                line
            );
        }

        if let Some(path) = out {
            use std::fmt::Write as _;
            let mut txt = String::new();
            let _ = writeln!(
                txt,
                "# per-site allocation report; snapshot global_live={} B, tracked={} B\n\
                 # ranked by live bytes at the peak snapshot; full resolved stacks",
                snap_live, tracked_at_peak
            );
            let mut snap_sorted = snap_sites.clone();
            snap_sorted.sort_by_key(|(_, l)| -*l);
            for (key, live) in snap_sorted.iter().take(80) {
                let frames = resolved
                    .entry(*key)
                    .or_insert_with(|| resolve_key(key, &mut cache))
                    .clone();
                let s = sites
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, s)| *s)
                    .unwrap_or_default();
                let _ = writeln!(
                    txt,
                    "\nsite live_at_peak={:.1} MiB site_live_max={:.1} MiB total={:.1} MiB count={}",
                    mb(*live),
                    mb(s.live_max),
                    mb(s.total as i64),
                    s.count
                );
                for fr in frames.iter().filter(|f| !is_noise(f)).take(18) {
                    let _ = writeln!(
                        txt,
                        "    {} ({}:{})",
                        short_sym(&fr.sym),
                        short_file(&fr.file),
                        fr.line
                    );
                }
            }
            if let Err(e) = std::fs::write(path, txt) {
                eprintln!("[sites] failed to write {path}: {e}");
            } else {
                eprintln!("[sites] full stacks written to {path}");
            }
        }
        GUARD.with(|g| g.set(false));
    }
}

#[global_allocator]
static ALLOC: counting_alloc::Counting = counting_alloc::Counting;

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
    // Wall time of the encode() call only (file read + startup excluded), so
    // it lines up with the `est` row's predicted time_ms.
    // Allocator counters are read around the encode call only, so the file
    // read and process startup don't pollute them. PEAK_LIVE is monotonic, so
    // it is taken as an absolute (it can only have been set during the encode
    // if the encode exceeded the pre-encode high-water).
    use core::sync::atomic::Ordering;
    if let Ok(v) = std::env::var("JXL_PEAK_TRACE_AT") {
        if let Ok(n) = v.parse::<usize>() {
            counting_alloc::TRACE_AT.store(n, Ordering::Relaxed);
        }
    }
    // Per-site allocation profiler (see `alloc_sites`). Enabled here, right
    // before the encode, so startup/file-read allocations stay out of the maps.
    if std::env::var("JXL_ALLOC_SITES").is_ok_and(|v| v == "1") {
        if let Some(n) = std::env::var("JXL_ALLOC_SITE_MIN")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            alloc_sites::SITE_MIN.store(n, Ordering::Relaxed);
        }
        if let Some(n) = std::env::var("JXL_ALLOC_SNAP_STEP")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            alloc_sites::SNAP_STEP.store(n, Ordering::Relaxed);
        }
        alloc_sites::ENABLED.store(1, Ordering::Relaxed);
    }
    let alloc_count_pre = counting_alloc::COUNT.load(Ordering::Relaxed);
    let t0 = std::time::Instant::now();
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
    let encode_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // High-water mark immediately after encode — VmHWM is monotonic, so it
    // reflects the peak *during* the encode.
    let peak = status_kb("VmHWM:");

    let alloc_count = counting_alloc::COUNT.load(Ordering::Relaxed) - alloc_count_pre;
    let peak_live_kb = counting_alloc::PEAK_LIVE.load(Ordering::Relaxed) / 1024;
    let peak_trigger_kb = counting_alloc::PEAK_TRIGGER.load(Ordering::Relaxed) / 1024;
    let peak_from_realloc = counting_alloc::PEAK_FROM_REALLOC.load(Ordering::Relaxed);
    eprintln!(
        "[peak] peak_live={peak_live_kb} KB  triggered_by={peak_trigger_kb} KB  \
         from_realloc={peak_from_realloc}"
    );
    alloc_sites::report(std::env::var("JXL_ALLOC_SITES_OUT").ok().as_deref());

    let pixels = (w as u64) * (h as u64);
    println!(
        "{w}\t{h}\t{pixels}\t{mode}\t{effort}\t{quality}\t{threads}\t{}\t{pre}\t{peak}\t{}\t{encode_ms:.1}\t{alloc_count}\t{peak_live_kb}",
        out.len(),
        peak.saturating_sub(pre)
    );
    black_box(&out);
}
