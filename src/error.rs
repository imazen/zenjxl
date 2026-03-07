//! JXL error types with location tracking via [`whereat::At`].

use zc::{HasUnsupportedOperation, UnsupportedOperation};

/// Errors from JXL encode/decode operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JxlError {
    /// JXL decoding error from jxl-rs.
    #[cfg(feature = "decode")]
    #[error("JXL decode error: {0}")]
    Decode(#[from] jxl::api::Error),

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

    /// Unsupported codec operation.
    #[error(transparent)]
    UnsupportedOperation(#[from] UnsupportedOperation),
}

impl HasUnsupportedOperation for JxlError {
    fn unsupported_operation(&self) -> Option<UnsupportedOperation> {
        match self {
            Self::UnsupportedOperation(op) => Some(*op),
            _ => None,
        }
    }
}
