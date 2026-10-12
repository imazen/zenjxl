#![no_main]

use libfuzzer_sys::fuzz_target;
use zencodec::decode::{DecodeJob, DecoderConfig};

// The walker must not panic on any input, must return Ok, and its result must
// validate (full coverage, no overlaps, nesting).
fuzz_target!(|data: &[u8]| {
    let job = zenjxl::JxlDecoderConfig::new().job();
    let inv = job
        .inventory(data)
        .expect("inventory never errors without a stop token or part overflow");
    assert_eq!(inv.input_len(), data.len() as u64);
    inv.validate().expect("inventory must validate");
});
