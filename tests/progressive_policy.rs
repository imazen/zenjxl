//! End-to-end test for the JXL during-decode progressive gate.
//!
//! Part B of the progressive gate: the zencodec decode adapter wires the
//! caller's [`DecodePolicy::allow_progressive`] into the decoder's
//! `JxlDecoderOptions::reject_progressive`. When the policy forbids progressive
//! (`allow_progressive == Some(false)`), a progressive JXL codestream
//! (multi-pass or LF frame) is rejected at the first frame header with
//! [`JxlError::ProgressiveRejected`]; a non-progressive JXL still decodes.
//!
//! The fixtures are encoded in-process so the bitstreams are guaranteed to be
//! progressive (or not) — `progressive_ac.jxl` from the decoder's corpus is
//! ~497 KB, too large to commit. The encoder's `with_progressive` /
//! single-pass behaviour was confirmed against the decoder gate during Part B:
//! `DcVlfLfAc` / `QuantizedAcFullAc` / `with_lf_frame` all trip the gate, a
//! plain config does not.

#![cfg(all(feature = "decode", feature = "zencodec", feature = "encode"))]

use std::borrow::Cow;

use zencodec::decode::{Decode, DecodeJob, DecodePolicy, DecoderConfig};
// `ProgressiveMode` is re-exported from zenjxl only under the `__expert`
// feature; reference it from the jxl-encoder dev-dependency directly so this
// test stays on the stable feature set (`encode` is enough to build fixtures).
use jxl_encoder::ProgressiveMode;
use zenjxl::{JxlDecoderConfig, JxlError, LossyConfig, PixelLayout};

const W: u32 = 64;
const H: u32 = 64;

/// A structured RGB image so VarDCT has real AC content (a flat image can
/// collapse to a near-trivial frame). Independent of the production code.
fn structured_pixels() -> Vec<u8> {
    let (w, h) = (W as usize, H as usize);
    let mut v = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            v.push(((x * 7 + y * 3) % 256) as u8);
            v.push(((x * 3 + y * 11) % 256) as u8);
            v.push(((x * 13 + y * 5) % 256) as u8);
        }
    }
    v
}

/// Encode a 3-pass progressive JXL (`ProgressiveMode::DcVlfLfAc`).
fn progressive_jxl() -> Vec<u8> {
    LossyConfig::new(2.0)
        .with_effort(3)
        .with_threads(1)
        .with_progressive(ProgressiveMode::DcVlfLfAc)
        .encode(&structured_pixels(), W, H, PixelLayout::Rgb8)
        .expect("encode progressive fixture")
}

/// Encode a plain single-pass JXL (no progressive, no LF frame).
fn non_progressive_jxl() -> Vec<u8> {
    LossyConfig::new(2.0)
        .with_effort(3)
        .with_threads(1)
        .encode(&structured_pixels(), W, H, PixelLayout::Rgb8)
        .expect("encode non-progressive fixture")
}

/// Decode `data` through the zencodec decode adapter under `policy`.
///
/// The adapter returns the envelope (`At<CodecError>`, Pattern B), not the
/// typed `At<JxlError>`; the specific variant stays recoverable as the
/// envelope's detail (asserted below).
fn decode_with_policy(
    data: Vec<u8>,
    policy: DecodePolicy,
) -> Result<zencodec::decode::DecodeOutput, whereat::At<zencodec::CodecError>> {
    JxlDecoderConfig::new()
        .job()
        .with_policy(policy)
        .decoder(Cow::Owned(data), &[])
        .expect("build decoder")
        .decode()
}

/// `allow_progressive == Some(false)` rejects a progressive JXL during decode
/// with the dedicated [`JxlError::ProgressiveRejected`] variant — recovered here
/// through the [`CodecError`](zencodec::CodecError) envelope as both the codec-
/// agnostic [`Policy`](zencodec::ErrorCategory::Policy)`(`[`PolicyKind::Decode`](
/// zencodec::PolicyKind::Decode)`)` category and the typed detail.
#[test]
fn progressive_rejected_when_policy_forbids() {
    use zencodec::CodecErrorExt;
    let policy = DecodePolicy::none().with_allow_progressive(false);
    let result = decode_with_policy(progressive_jxl(), policy);
    match result {
        Err(at) => {
            // Codec-agnostic axis: the category + codec name survive in the
            // envelope (this is what a generic consumer routes on).
            assert_eq!(
                at.error().category(),
                zencodec::ErrorCategory::Policy(zencodec::PolicyKind::Decode),
                "progressive rejection must categorize as Policy(Decode)"
            );
            assert_eq!(at.error().codec(), Some("zenjxl"));
            // Typed axis: the exact JxlError variant is still recoverable as the
            // envelope's detail.
            assert!(
                matches!(
                    at.error().find_cause::<JxlError>(),
                    Some(JxlError::ProgressiveRejected)
                ),
                "expected JxlError::ProgressiveRejected detail, got {:?}",
                at.error()
            );
        }
        Ok(_) => panic!("progressive JXL must be rejected when allow_progressive == Some(false)"),
    }
}

/// `allow_progressive == Some(true)` decodes a progressive JXL normally.
#[test]
fn progressive_allowed_when_policy_permits() {
    let policy = DecodePolicy::none().with_allow_progressive(true);
    let out = decode_with_policy(progressive_jxl(), policy)
        .expect("progressive JXL must decode when allow_progressive == Some(true)");
    assert_eq!(out.width(), W);
    assert_eq!(out.height(), H);
}

/// Default policy (`allow_progressive == None`) does NOT gate — progressive
/// decodes, matching the decoder's default (`reject_progressive = false`).
#[test]
fn progressive_allowed_by_default() {
    let out = decode_with_policy(progressive_jxl(), DecodePolicy::none())
        .expect("progressive JXL must decode under the default (no-preference) policy");
    assert_eq!(out.width(), W);
    assert_eq!(out.height(), H);
}

/// A non-progressive JXL decodes fine even with `allow_progressive == Some(false)`:
/// the gate is specific to progressive content, not a blanket rejection.
#[test]
fn non_progressive_decodes_under_strict_policy() {
    let policy = DecodePolicy::none().with_allow_progressive(false);
    let out = decode_with_policy(non_progressive_jxl(), policy)
        .expect("non-progressive JXL must decode even when allow_progressive == Some(false)");
    assert_eq!(out.width(), W);
    assert_eq!(out.height(), H);
}
