//! Memory behaviour of `DecodeJob::inventory`: no allocation proportional to
//! the image size, caller limits respected, bounded by the part cap on
//! hostile box counts. Lives in its own test binary because it installs a
//! counting global allocator.

#![cfg(all(feature = "zencodec", feature = "encode", feature = "decode"))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use zencodec::decode::{DecodeJob as _, DecoderConfig as _};
use zencodec::encode::{AnimationFrameEncoder as _, EncodeJob as _, EncoderConfig as _};
use zenjxl::{JxlDecoderConfig, JxlEncoderConfig};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            if n >= l.size() {
                let now = LIVE.fetch_add(n - l.size(), Ordering::Relaxed) + (n - l.size());
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(l.size() - n, Ordering::Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

/// The counters are process-wide: run the measuring tests one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn peak_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let r = f();
    (r, PEAK.load(Ordering::Relaxed).saturating_sub(base))
}

const SIG: [u8; 12] = [0, 0, 0, 12, b'J', b'X', b'L', b' ', 13, 10, 0x87, 10];

fn container(boxes: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut o = SIG.to_vec();
    o.extend(20u32.to_be_bytes());
    o.extend(b"ftypjxl \0\0\0\0jxl ");
    boxes(&mut o);
    o
}

/// A two-frame flat-colour animation: tiny file, large frames.
fn animation(size: u32) -> Vec<u8> {
    let mut enc = JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .animation_frame_encoder()
        .expect("animation encoder");
    for f in 0..2u8 {
        let pixels = vec![rgb::Rgb { r: f, g: 7, b: 9 }; (size * size) as usize];
        let buf = zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(pixels, size, size)
            .expect("buffer");
        enc.push_frame(buf.as_slice().into(), 100, None)
            .expect("push");
    }
    enc.finish(None).expect("finish").into_vec()
}

#[test]
fn large_frames_need_no_image_sized_memory() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let size = 2048u32; // a frame plane is 16 MiB of f32
    let mut bytes = animation(size);
    let n = bytes.len();
    bytes.extend(b"APPENDED-SECRET-0123456789-ABCDEFGHIJK");
    let (inv, peak) = peak_during(|| {
        JxlDecoderConfig::new()
            .job()
            .inventory(&bytes)
            .expect("inventory")
            .expect("declared")
    });
    inv.validate().unwrap();
    eprintln!(
        "{size}x{size} x2 frames, {n} bytes: inventory peak heap {} KiB",
        peak >> 10
    );
    assert!(n < 1 << 20, "the animation is {n} bytes");
    assert!(
        peak < 2 << 20,
        "peak heap {} KiB for a {size}x{size} two-frame file of {n} bytes",
        peak >> 10
    );
    // The appended bytes are found, not folded into the codestream.
    let tail = inv
        .children(None)
        .last()
        .map(|id| inv.get(*id).unwrap())
        .unwrap();
    assert_eq!(tail.range, n as u64..bytes.len() as u64, "{inv}");
    assert!(!tail.disposition.is_consumed());
    zencodec_testkit::check_inventory(JxlDecoderConfig::new(), &bytes[..n]).expect("check");
}

#[test]
fn caller_memory_limit_is_respected() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = animation(1024);
    let limits = zencodec::ResourceLimits::none().with_max_memory(8 << 20);
    let (r, peak) = peak_during(|| {
        JxlDecoderConfig::new()
            .job()
            .with_limits(limits)
            .inventory(&bytes)
            .expect("inventory")
            .expect("declared")
    });
    r.validate().unwrap();
    eprintln!("max_memory 8 MiB: inventory peak heap {} KiB", peak >> 10);
    assert!(
        peak < 8 << 20,
        "peak {} KiB under an 8 MiB limit",
        peak >> 10
    );
}

#[test]
fn hostile_box_counts_stay_bounded() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // Each 8-byte `jxll` box becomes a part: the part cap (1 Mi parts) must
    // stop the walk with a bounded allocation, and the memory per part must
    // stay small.
    let n = 2_000_000usize;
    let bytes = container(|o| {
        o.reserve(n * 8);
        for _ in 0..n {
            o.extend([0, 0, 0, 8, b'j', b'x', b'l', b'l']);
        }
    });
    let (r, peak) = peak_during(|| JxlDecoderConfig::new().job().inventory(&bytes).map(|_| ()));
    assert!(r.is_err(), "the part cap applies");
    let per_part = peak / (1 << 20);
    eprintln!(
        "{n} jxll boxes: peak heap {} MiB, {per_part} B per part",
        peak >> 20
    );
    assert!(
        per_part <= 200,
        "{per_part} bytes of heap per recorded part (peak {} MiB)",
        peak >> 20
    );
}
