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

    /// Resource limit exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(alloc::string::String),

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
            // A configured resource cap (or a graceful allocation/sizing failure
            // deliberately routed as a limit rather than an abort) was exceeded.
            // The variant is stringly and conflates several caps (pixel count,
            // buffer/byte-size overflow, allocation failure), so a single
            // representative `LimitKind` is reported — `Memory` dominates the
            // construction sites (the `alloc_util` graceful-allocation path plus
            // the buffer-size overflow checks).
            JxlError::LimitExceeded(_) => ErrorCategory::LimitsExceeded(LimitKind::Memory),
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
        let mut cases: Vec<(JxlError, ErrorCategory)> = alloc::vec![
            (
                JxlError::InvalidInput("bad dims".into()),
                ErrorCategory::InvalidParameters,
            ),
            (
                JxlError::LimitExceeded("pixel count exceeds limit".into()),
                ErrorCategory::LimitsExceeded(LimitKind::Memory),
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
