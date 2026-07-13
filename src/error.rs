//! JXL error types with location tracking via [`whereat::At`].

use zencodec::{
    CategorizedError, CodecIoKind, ErrorCategory, ImageError, InvalidKind, LimitKind, RequestError,
    ResourceError, UnsupportedOperation,
};
// Only reachable from the `#[cfg(feature = "decode")]` arms below (jxl-rs's
// `ErrorClass::Unsupported` and the `ProgressiveRejected` policy rejection) —
// gate the imports too so a `decode`-less build doesn't warn on unused.
#[cfg(feature = "decode")]
use zencodec::{PolicyKind, UnsupportedImageKind};
// `InternalKind` is only constructed inside the `Decode`/`Encode` foreign-error
// introspection arms — gate it the same way (a `zencodec`-only build with
// neither `decode` nor `encode` never constructs it).
#[cfg(any(feature = "decode", feature = "encode"))]
use zencodec::InternalKind;

/// Errors from JXL encode/decode operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JxlError {
    /// JXL decoding error from jxl-rs.
    #[cfg(feature = "decode")]
    #[error("JXL decode error: {0}")]
    Decode(#[from] jxl::api::Error),

    /// Progressive content was rejected by the decode policy.
    ///
    /// Surfaced when the caller's [`DecodePolicy`](zencodec::decode::DecodePolicy)
    /// sets `allow_progressive == Some(false)` and the JXL codestream carries a
    /// progressive frame header (multi-pass or LF frame). The decoder reports
    /// this during the decode pass — the header-only probe never triggers it.
    #[cfg(feature = "decode")]
    #[error("progressive content rejected by decode policy")]
    ProgressiveRejected,

    /// JXL encoding error from jxl-encoder.
    #[cfg(feature = "encode")]
    #[error("JXL encode error: {0}")]
    Encode(#[from] jxl_encoder::EncodeError),

    /// Invalid input (dimensions, buffer size, etc.) — a caller-request-origin
    /// parameter fault: a *different* call with corrected parameters would
    /// succeed. Distinct from [`MalformedImage`](JxlError::MalformedImage)
    /// (image-bytes origin — no parameter choice would fix it) and
    /// [`InvalidState`](JxlError::InvalidState) (call-sequencing origin).
    #[error("invalid input: {0}")]
    InvalidInput(alloc::string::String),

    /// The input ended before a complete JXL codestream could be decoded — the
    /// decoder reported `NeedsMoreInput` on a one-shot buffer (truncated /
    /// incomplete input). Distinct from [`InvalidInput`](JxlError::InvalidInput)
    /// so a generic consumer routes truncated input to
    /// [`ImageError::UnexpectedEof`] (a 4xx incomplete-request) rather than
    /// [`InvalidKind::Parameters`].
    #[error("unexpected end of input: {0}")]
    UnexpectedEof(alloc::string::String),

    /// A value obtained by decoding the JXL bitstream fails zenjxl's own
    /// post-decode validation — e.g. a header-reported dimension that
    /// overflows `u32`, or an embedded ISO 21496-1 (`jhgm`) gain-map box that
    /// fails to parse. Image-bytes-origin: distinct from
    /// [`InvalidInput`](JxlError::InvalidInput) because no caller-supplied
    /// parameter would fix it — the encoded bytes themselves are the fault.
    /// Additive variant (the enum is `#[non_exhaustive]`).
    #[error("malformed image: {0}")]
    MalformedImage(alloc::string::String),

    /// The operation was invoked out of sequence — e.g. `finish()` called
    /// before any rows or frames were pushed. Distinct from
    /// [`InvalidInput`](JxlError::InvalidInput): a call-protocol violation by
    /// the caller, not a bad parameter value. Additive variant (the enum is
    /// `#[non_exhaustive]`).
    #[error("invalid state: {0}")]
    InvalidState(alloc::string::String),

    /// A configured resource cap (dimensions, memory, frame count, input/output
    /// size — see [`zencodec::ResourceLimits`]) was exceeded. Carries the
    /// actual [`LimitKind`] that was hit — read from the triggering
    /// [`zencodec::LimitExceeded`] at each construction site, or set directly
    /// for zenjxl's own [`JxlLimits`](crate::decode::JxlLimits) checks — so
    /// [`category`](CategorizedError::category) reports the real cap instead
    /// of a single hardcoded value.
    #[error("limit exceeded ({1:?}): {0}")]
    LimitExceeded(alloc::string::String, LimitKind),

    /// A fallible allocation or a size computation overflowed, not a
    /// caller-configured cap. Distinct from [`JxlError::LimitExceeded`] so a
    /// generic consumer's [`ErrorCategory`] routing (see [`category`](
    /// CategorizedError::category)) doesn't conflate "your configured limit
    /// was hit" with "the process ran out of memory / the size computation
    /// wrapped".
    #[error("out of memory: {0}")]
    OutOfMemory(alloc::string::String),

    /// Operation was cancelled via Stop token.
    #[error("cancelled: {0}")]
    Cancelled(enough::StopReason),

    /// Unsupported codec operation.
    #[error("{0}")]
    UnsupportedOperation(
        #[source]
        #[from]
        UnsupportedOperation,
    ),

    /// Decode row sink error.
    #[error("sink error: {0}")]
    Sink(zencodec::decode::SinkError),
}

/// Coarse, codec-agnostic classification of [`JxlError`] for the zencodec error
/// taxonomy (zencodec PR #116 — the two-level origin-first `ErrorCategory`
/// reshape: `Image`/`Request`/`Resource`/`Policy`/`Stopped`/`Io`/`Internal`).
/// A generic consumer routes on the [`ErrorCategory`] — HTTP status, retry
/// policy, logging — without naming the concrete enum, and
/// [`codec_name`](CategorizedError::codec_name) tags the originating codec.
///
/// The mapping is total: every variant maps to exactly one category.
/// `Cancelled` / `UnsupportedOperation` delegate to the zencodec cause type's
/// own classification. `Decode` / `Encode` **introspect** the wrapped foreign
/// error's own sub-variants — via [`jxl::api::Error::kind`] (a coarse
/// `ErrorClass` jxl-rs itself maintains for exactly this purpose) and a direct
/// match over [`jxl_encoder::EncodeError`]'s variants respectively — rather
/// than folding every foreign decode/encode failure into one bucket.
impl CategorizedError for JxlError {
    #[inline]
    fn codec_name(&self) -> Option<&'static str> {
        Some("zenjxl")
    }

    fn category(&self) -> ErrorCategory {
        match self {
            // Introspect the decoder's own best-effort classification instead
            // of blanket-folding every decode failure to `Malformed`.
            // `ErrorClass` is `#[non_exhaustive]`; the wildcard mirrors
            // jxl-rs's own default arm for unclassified variants (a
            // malformed/corrupt/spec-violating bitstream).
            #[cfg(feature = "decode")]
            JxlError::Decode(e) => match e.kind() {
                jxl::api::ErrorClass::InvalidBitstream => {
                    ErrorCategory::Image(ImageError::Malformed)
                }
                // A progressive-content rejection is intercepted earlier (see
                // `JxlError::ProgressiveRejected`); today this arm is reached
                // only by `InvalidRenderingIntent`, but any future decoder
                // variant classified `Unsupported` lands here too — a
                // well-formed image using a feature this decoder declines.
                jxl::api::ErrorClass::Unsupported => {
                    ErrorCategory::Image(ImageError::Unsupported(UnsupportedImageKind::Feature))
                }
                // The decoder's own security/resource ceiling (pixels,
                // memory_bytes, icc_size, icc_amplification, extra_channels,
                // reference_frames — see `JxlDecoderLimits`) doesn't carry a
                // `zencodec::LimitKind`-shaped discriminant; `Memory` is the
                // representative bucket (honest best-effort, not a claim that
                // every one of those resources IS memory).
                jxl::api::ErrorClass::LimitExceeded => {
                    ErrorCategory::Resource(ResourceError::Limits(LimitKind::Memory))
                }
                jxl::api::ErrorClass::OutOfMemory => {
                    ErrorCategory::Resource(ResourceError::OutOfMemory)
                }
                jxl::api::ErrorClass::Cancelled => {
                    ErrorCategory::Stopped(enough::StopReason::Cancelled)
                }
                jxl::api::ErrorClass::Io => ErrorCategory::Io(CodecIoKind::opaque()),
                // "wrong output buffer size/count, grayscale mismatch, or an
                // ICC/CMS configuration that cannot satisfy the request" — a
                // usage error by the *caller*, not the image bytes.
                jxl::api::ErrorClass::OutputConfiguration => {
                    ErrorCategory::Request(RequestError::Invalid(InvalidKind::Buffer))
                }
                // An internal decoder invariant violation — a jxl-rs bug, not
                // ours, and not attributable to the input or the request.
                jxl::api::ErrorClass::Internal => ErrorCategory::Internal(InternalKind::Dependency),
                _ => ErrorCategory::Image(ImageError::Malformed),
            },
            // The bitstream is valid; the caller's `DecodePolicy` declined
            // progressive content. Understood-and-declined, decode-side policy.
            #[cfg(feature = "decode")]
            JxlError::ProgressiveRejected => ErrorCategory::Policy(PolicyKind::Decode),
            // `jxl_encoder::EncodeError` is a foreign, `#[non_exhaustive]`
            // encode error; introspect its variants instead of blanket-folding
            // to `Internal` — most of them are caller-request faults, not
            // encoder bugs.
            #[cfg(feature = "encode")]
            JxlError::Encode(e) => match e {
                jxl_encoder::EncodeError::InvalidInput { .. }
                | jxl_encoder::EncodeError::InvalidConfig { .. } => {
                    ErrorCategory::Request(RequestError::Invalid(InvalidKind::Parameters))
                }
                jxl_encoder::EncodeError::UnsupportedPixelLayout(_) => ErrorCategory::Request(
                    RequestError::Unsupported(UnsupportedOperation::PixelFormat),
                ),
                // No `zencodec::LimitKind`-shaped discriminant is available
                // from jxl-encoder's flat `{ message }` payload; `Memory` is
                // the representative bucket (same caveat as the decode-side
                // `LimitExceeded` arm above).
                jxl_encoder::EncodeError::LimitExceeded { .. } => {
                    ErrorCategory::Resource(ResourceError::Limits(LimitKind::Memory))
                }
                jxl_encoder::EncodeError::Cancelled => {
                    ErrorCategory::Stopped(enough::StopReason::Cancelled)
                }
                jxl_encoder::EncodeError::Oom(_) => {
                    ErrorCategory::Resource(ResourceError::OutOfMemory)
                }
                jxl_encoder::EncodeError::Io(_) => ErrorCategory::Io(CodecIoKind::opaque()),
                // An encoder-internal bug — jxl-encoder's, not ours.
                jxl_encoder::EncodeError::Internal { .. } => {
                    ErrorCategory::Internal(InternalKind::Dependency)
                }
                // The JPEG bytes handed to the lossless transcoder could not
                // be parsed (not baseline-sequential) or use an unsupported
                // JPEG feature (e.g. arithmetic coding) — either way the
                // *input image bytes* are the fault, not the caller's request.
                jxl_encoder::EncodeError::JpegParse { .. } => {
                    ErrorCategory::Image(ImageError::Malformed)
                }
                // `#[non_exhaustive]`: an unclassified foreign failure.
                _ => ErrorCategory::Internal(InternalKind::Dependency),
            },
            // Caller-supplied parameters were invalid — a request-origin fault.
            JxlError::InvalidInput(_) => {
                ErrorCategory::Request(RequestError::Invalid(InvalidKind::Parameters))
            }
            // The input ended mid-codestream (decoder returned `NeedsMoreInput`
            // on a one-shot buffer) → incomplete client input, not a bad
            // parameter. A truncated request is image-bytes-origin.
            JxlError::UnexpectedEof(_) => ErrorCategory::Image(ImageError::UnexpectedEof),
            // zenjxl's own post-decode validation rejected a value the
            // decoder reported — the encoded bytes are the fault.
            JxlError::MalformedImage(_) => ErrorCategory::Image(ImageError::Malformed),
            // An operation was invoked out of sequence — a call-protocol
            // violation by the caller, not a bad parameter value.
            JxlError::InvalidState(_) => {
                ErrorCategory::Request(RequestError::Invalid(InvalidKind::State))
            }
            // A caller-configured `zencodec::ResourceLimits` cap (dimensions,
            // memory, frame count, input/output size) was exceeded — the
            // *real* kind is carried structurally, read from the triggering
            // `zencodec::LimitExceeded` at each construction site.
            JxlError::LimitExceeded(_, kind) => {
                ErrorCategory::Resource(ResourceError::Limits(*kind))
            }
            // A fallible allocation failed, or a size computation overflowed —
            // not a caller-configured cap. Matches the zenjpeg precedent of
            // routing allocation-failure / size-overflow to `OutOfMemory`
            // rather than folding it into `LimitsExceeded`.
            JxlError::OutOfMemory(_) => ErrorCategory::Resource(ResourceError::OutOfMemory),
            // Delegate to the zencodec cause type: cancelled vs timed out.
            JxlError::Cancelled(reason) => reason.category(),
            // Delegate to the zencodec cause type: unsupported operation vs
            // unsupported pixel format.
            JxlError::UnsupportedOperation(op) => op.category(),
            // A caller-supplied decode row sink failed — an output-sink I/O
            // failure (`SinkError` is an opaque boxed error).
            JxlError::Sink(_) => ErrorCategory::Io(CodecIoKind::opaque()),
        }
    }
}

/// Bridge a bare [`JxlError`] into the shared [`CodecError`](zencodec::CodecError)
/// envelope as `At<CodecError>` — the one-impl adoption cost of the zencodec
/// **envelope** error pattern (Pattern B). The zencodec trait impls
/// ([`codec`](crate::codec)) return `At<CodecError>` so a generic consumer
/// recovers the [`ErrorCategory`] *and* the codec name through `Dyn*` dispatch
/// and `Box<dyn Error>` erasure; the native API ([`decode`](crate::decode) etc.)
/// keeps the typed `At<JxlError>`.
///
/// `.start_at()` begins the location trace; [`CodecError::of`](zencodec::CodecError::of)
/// then maps that `At<JxlError>` to `At<CodecError>`, reading the
/// [`category`](CategorizedError::category) and
/// [`codec_name`](CategorizedError::codec_name) off the value — keeping the `At`
/// trace on the outside and the `JxlError` as the recoverable detail. With this
/// in place, `JxlError::from(op).into()` (and `?` on a bare `JxlError`) auto-wraps
/// into the envelope at a trait boundary.
///
/// Already-located `At<JxlError>` values cannot use this (`From<At<JxlError>> for
/// At<CodecError>` is barred by the orphan rule); they convert with
/// `.map_err(zencodec::CodecError::of)`, which the adapter does once at each
/// trait boundary (its fallible internals stay `At<JxlError>`).
impl From<JxlError> for whereat::At<zencodec::CodecError> {
    #[track_caller]
    fn from(e: JxlError) -> Self {
        use whereat::ErrorAtExt;
        zencodec::CodecError::of(e.start_at())
    }
}

#[cfg(test)]
mod categorized_error_tests {
    use super::*;

    /// Every `JxlError` variant maps to its documented [`ErrorCategory`], and
    /// `codec_name()` is `Some("zenjxl")` across the board.
    #[test]
    fn category_and_codec_name_mapping() {
        use alloc::boxed::Box;
        use alloc::vec::Vec;

        // Each variant is constructed and checked against its expected category.
        // The foreign cause types (`jxl::api::Error`, `jxl_encoder::EncodeError`)
        // are `#[non_exhaustive]` enums, but enum-level non-exhaustiveness only
        // blocks exhaustive matching downstream — their (non-marked) variants
        // are still constructible here.
        // `mut` is only exercised by the `decode` / `encode` pushes below; with
        // neither feature on (e.g. CI's `--no-default-features --features zencodec`
        // clippy job) the vec is never mutated.
        #[cfg_attr(not(any(feature = "decode", feature = "encode")), allow(unused_mut))]
        let mut cases: Vec<(JxlError, ErrorCategory)> = alloc::vec![
            (
                JxlError::InvalidInput("bad dims".into()),
                ErrorCategory::Request(RequestError::Invalid(InvalidKind::Parameters)),
            ),
            (
                JxlError::UnexpectedEof("insufficient data for header".into()),
                ErrorCategory::Image(ImageError::UnexpectedEof),
            ),
            (
                JxlError::MalformedImage("JXL: width dimension exceeds u32".into()),
                ErrorCategory::Image(ImageError::Malformed),
            ),
            (
                JxlError::InvalidState("finish: no rows were pushed".into()),
                ErrorCategory::Request(RequestError::Invalid(InvalidKind::State)),
            ),
            (
                JxlError::LimitExceeded("pixel count exceeds limit".into(), LimitKind::Pixels),
                ErrorCategory::Resource(ResourceError::Limits(LimitKind::Pixels)),
            ),
            (
                JxlError::LimitExceeded("frame count exceeds limit".into(), LimitKind::Frames),
                ErrorCategory::Resource(ResourceError::Limits(LimitKind::Frames)),
            ),
            (
                JxlError::OutOfMemory("out of memory allocating 4096 bytes".into()),
                ErrorCategory::Resource(ResourceError::OutOfMemory),
            ),
            // `Cancelled` / `UnsupportedOperation` delegate to the cause type.
            (
                JxlError::Cancelled(enough::StopReason::Cancelled),
                ErrorCategory::Stopped(enough::StopReason::Cancelled),
            ),
            (
                JxlError::Cancelled(enough::StopReason::TimedOut),
                ErrorCategory::Stopped(enough::StopReason::TimedOut),
            ),
            (
                JxlError::UnsupportedOperation(UnsupportedOperation::AnimationDecode),
                ErrorCategory::Request(RequestError::Unsupported(
                    UnsupportedOperation::AnimationDecode
                )),
            ),
            (
                JxlError::UnsupportedOperation(UnsupportedOperation::PixelFormat),
                ErrorCategory::Request(RequestError::Unsupported(
                    UnsupportedOperation::PixelFormat
                )),
            ),
            (
                JxlError::Sink(Box::<dyn core::error::Error + Send + Sync>::from(
                    "sink failed"
                )),
                ErrorCategory::Io(CodecIoKind::opaque()),
            ),
        ];

        #[cfg(feature = "decode")]
        {
            // `InvalidSignature` falls through jxl-rs's own `ErrorClass`
            // default arm (`InvalidBitstream`) → `Image(Malformed)`.
            cases.push((
                JxlError::Decode(jxl::api::Error::InvalidSignature),
                ErrorCategory::Image(ImageError::Malformed),
            ));
            // `ErrorClass::LimitExceeded` — the decoder's own security cap.
            cases.push((
                JxlError::Decode(jxl::api::Error::LimitExceeded {
                    resource: "pixels",
                    actual: 9,
                    limit: 8,
                }),
                ErrorCategory::Resource(ResourceError::Limits(LimitKind::Memory)),
            ));
            // `ErrorClass::OutOfMemory`.
            cases.push((
                JxlError::Decode(jxl::api::Error::ImageOutOfMemory(4, 4)),
                ErrorCategory::Resource(ResourceError::OutOfMemory),
            ));
            // `ErrorClass::OutputConfiguration` — a caller usage error, not an
            // image-bytes fault.
            cases.push((
                JxlError::Decode(jxl::api::Error::NotGrayscale),
                ErrorCategory::Request(RequestError::Invalid(InvalidKind::Buffer)),
            ));
            // `ErrorClass::Unsupported` (excluding `ProgressiveRejected`, which
            // is intercepted before it ever reaches `JxlError::Decode`).
            cases.push((
                JxlError::Decode(jxl::api::Error::InvalidRenderingIntent),
                ErrorCategory::Image(ImageError::Unsupported(UnsupportedImageKind::Feature)),
            ));
            // `ErrorClass::Internal` — jxl-rs's own bug, not zenjxl's.
            cases.push((
                JxlError::Decode(jxl::api::Error::ArithmeticOverflow),
                ErrorCategory::Internal(InternalKind::Dependency),
            ));
            cases.push((
                JxlError::ProgressiveRejected,
                ErrorCategory::Policy(PolicyKind::Decode),
            ));
        }

        #[cfg(feature = "encode")]
        {
            cases.push((
                JxlError::Encode(jxl_encoder::EncodeError::Internal {
                    message: "boom".into(),
                }),
                ErrorCategory::Internal(InternalKind::Dependency),
            ));
            cases.push((
                JxlError::Encode(jxl_encoder::EncodeError::InvalidInput {
                    message: "zero width".into(),
                }),
                ErrorCategory::Request(RequestError::Invalid(InvalidKind::Parameters)),
            ));
            cases.push((
                JxlError::Encode(jxl_encoder::EncodeError::UnsupportedPixelLayout(
                    jxl_encoder::PixelLayout::Gray16,
                )),
                ErrorCategory::Request(RequestError::Unsupported(
                    UnsupportedOperation::PixelFormat,
                )),
            ));
            cases.push((
                JxlError::Encode(jxl_encoder::EncodeError::LimitExceeded {
                    message: "output too large".into(),
                }),
                ErrorCategory::Resource(ResourceError::Limits(LimitKind::Memory)),
            ));
            cases.push((
                JxlError::Encode(jxl_encoder::EncodeError::Cancelled),
                ErrorCategory::Stopped(enough::StopReason::Cancelled),
            ));
        }

        for (err, expected) in cases {
            assert_eq!(err.category(), expected, "wrong category for {err:?}");
            assert_eq!(
                err.codec_name(),
                Some("zenjxl"),
                "wrong codec_name for {err:?}"
            );
        }
    }
}
