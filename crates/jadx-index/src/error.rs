//! Error type of the index crate.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, IndexError>;

#[derive(Debug, Error)]
pub enum IndexError {
    /// A configured bound (memory budget, node count, result count) was exceeded.
    #[error("limit exceeded: {0}")]
    Limit(String),
    /// The build or query was cancelled through the cancel flag.
    #[error("cancelled")]
    Cancelled,
    /// A caller supplied an unusable argument (unknown method, bad pattern).
    #[error("{0}")]
    Invalid(String),
    /// The code source could not decode a method.
    #[error("code source: {0}")]
    Source(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
