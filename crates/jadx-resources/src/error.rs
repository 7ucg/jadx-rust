//! Error type of the resource decoders.

use std::fmt;

/// Decode failure. The message follows the wording of the reference decoder where it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResError(pub String);

impl ResError {
    pub fn new(msg: impl Into<String>) -> Self {
        ResError(msg.into())
    }
}

impl fmt::Display for ResError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ResError {}

impl From<std::io::Error> for ResError {
    fn from(e: std::io::Error) -> Self {
        ResError(e.to_string())
    }
}

impl From<zip::result::ZipError> for ResError {
    fn from(e: zip::result::ZipError) -> Self {
        ResError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ResError>;
