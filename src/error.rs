//! JXL error types with location tracking via [`whereat::At`].

use zencodec::{CategorizedError, CodecIoKind, ErrorCategory, LimitKind, UnsupportedOperation};

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

    /// Invalid input (dimensions, buffer size, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(alloc::string::String),

    /// A configured resource cap (dimensions, memory, frame count, input/output
    /// size — see [`zencodec::ResourceLimits`]) was exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(alloc::string::String),

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
/// taxonomy (zencodec PR #103). A generic consumer routes on the
/// [`ErrorCategory`] — HTTP status, retry policy, logging — without naming the
/// concrete enum, and [`codec_name`](CategorizedError::codec_name) tags the
/// originating codec.
///
/// The mapping is total: every variant maps to exactly one category, with the
/// `Cancelled` / `UnsupportedOperation` arms delegating to the zencodec cause
/// type's own classification so the cancelled-vs-timed-out and
/// operation-vs-pixel-format splits stay in one place.
impl CategorizedError for JxlError {
    #[inline]
    fn codec_name(&self) -> Option<&'static str> {
        Some("zenjxl")
    }

    fn category(&self) -> ErrorCategory {
        match self {
            // `jxl::api::Error` is a foreign decoder error; a decode failure
            // means the JXL bitstream could not be parsed → malformed.
            #[cfg(feature = "decode")]
            JxlError::Decode(_) => ErrorCategory::MalformedImage,
            // The bitstream is valid; the caller's `DecodePolicy` declined
            // progressive content. Understood-and-declined → PolicyRejected.
            #[cfg(feature = "decode")]
            JxlError::ProgressiveRejected => ErrorCategory::PolicyRejected,
            // `jxl_encoder::EncodeError` is a foreign encode error spanning
            // config / limit / oom / internal kinds; mapped to a single bucket.
            // An encode failure is a producer-side fault not attributable to
            // input image data → Internal.
            #[cfg(feature = "encode")]
            JxlError::Encode(_) => ErrorCategory::Internal,
            // Caller-supplied parameters / dimensions were invalid.
            JxlError::InvalidInput(_) => ErrorCategory::InvalidParameters,
            // A caller-configured `zencodec::ResourceLimits` cap (dimensions,
            // memory, frame count, input/output size) was exceeded. The variant
            // is stringly and conflates several caps, so a single representative
            // `LimitKind` is reported — `Memory` dominates the construction
            // sites.
            JxlError::LimitExceeded(_) => ErrorCategory::LimitsExceeded(LimitKind::Memory),
            // A fallible allocation failed, or a size computation overflowed —
            // not a caller-configured cap. Matches the zenjpeg precedent of
            // routing allocation-failure / size-overflow to `OutOfMemory`
            // rather than folding it into `LimitsExceeded`.
            JxlError::OutOfMemory(_) => ErrorCategory::OutOfMemory,
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
                ErrorCategory::InvalidParameters,
            ),
            (
                JxlError::LimitExceeded("pixel count exceeds limit".into()),
                ErrorCategory::LimitsExceeded(LimitKind::Memory),
            ),
            (
                JxlError::OutOfMemory("out of memory allocating 4096 bytes".into()),
                ErrorCategory::OutOfMemory,
            ),
            // `Cancelled` / `UnsupportedOperation` delegate to the cause type.
            (
                JxlError::Cancelled(enough::StopReason::Cancelled),
                ErrorCategory::Cancelled,
            ),
            (
                JxlError::Cancelled(enough::StopReason::TimedOut),
                ErrorCategory::TimedOut,
            ),
            (
                JxlError::UnsupportedOperation(UnsupportedOperation::AnimationDecode),
                ErrorCategory::UnsupportedOperation,
            ),
            (
                JxlError::UnsupportedOperation(UnsupportedOperation::PixelFormat),
                ErrorCategory::UnsupportedPixelFormat,
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
            cases.push((
                JxlError::Decode(jxl::api::Error::InvalidSignature),
                ErrorCategory::MalformedImage,
            ));
            cases.push((JxlError::ProgressiveRejected, ErrorCategory::PolicyRejected));
        }

        #[cfg(feature = "encode")]
        {
            cases.push((
                JxlError::Encode(jxl_encoder::EncodeError::Internal {
                    message: "boom".into(),
                }),
                ErrorCategory::Internal,
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
