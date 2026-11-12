//! Error type of the class file reader. Parsing never panics: malformed input yields a
//! [`JavaError`].

use thiserror::Error;

pub type Result<T> = std::result::Result<T, JavaError>;

#[derive(Debug, Error)]
pub enum JavaError {
    #[error("unexpected end of data at offset 0x{offset:x} (wanted {wanted} bytes, size {size})")]
    Truncated {
        offset: usize,
        wanted: usize,
        size: usize,
    },
    #[error("invalid class file: {0}")]
    Format(String),
    #[error("unknown opcode: 0x{0:x}")]
    UnknownOpcode(u32),
    #[error("zip error: {0}")]
    Zip(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl JavaError {
    pub(crate) fn format(msg: impl Into<String>) -> JavaError {
        JavaError::Format(msg.into())
    }
}
