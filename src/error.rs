//! JXL error types with location tracking via [`whereat::At`].

use zencodec::UnsupportedOperation;

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
