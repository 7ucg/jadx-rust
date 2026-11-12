//! Error type of the package reader.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, PackageError>;

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("{0}")]
    Invalid(String),
    #[error("limit exceeded: {0}")]
    Limit(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("archive changed on disk: {0}")]
    Changed(String),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl PackageError {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        PackageError::Invalid(msg.into())
    }
}

/// Return an `Invalid` error unless the condition holds.
macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {
        if !($cond) {
            return Err($crate::error::PackageError::Invalid(format!($($arg)+)));
        }
    };
}
pub(crate) use ensure;
