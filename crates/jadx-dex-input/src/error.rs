//! Error type for DEX parsing. Parsing never panics: malformed input yields a `DexError`.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, DexError>;

#[derive(Debug, Error)]
pub enum DexError {
    #[error(
        "unexpected end of data at offset 0x{offset:x} (wanted {wanted} bytes, file size {size})"
    )]
    Truncated {
        offset: usize,
        wanted: usize,
        size: usize,
    },
    #[error("invalid DEX magic: {0:02x?}")]
    BadMagic([u8; 4]),
    #[error("invalid DEX version: {0:?}")]
    BadVersion(String),
    #[error("unexpected endian tag: 0x{0:08x}")]
    BadEndianTag(u32),
    #[error("invalid LEB128 sequence at offset 0x{0:x}")]
    BadLeb128(usize),
    #[error("invalid MUTF-8 string: {0}")]
    BadMutf8(&'static str),
    #[error("{what} index {idx} out of range (count {count})")]
    IndexOutOfRange {
        what: &'static str,
        idx: u32,
        count: u32,
    },
    #[error("{what} table out of bounds: offset 0x{off:x}, size {size}, file size {file_size}")]
    TableOutOfBounds {
        what: &'static str,
        off: u32,
        size: u32,
        file_size: usize,
    },
    #[error("bad dex file checksum: 0x{actual:08x}, expected: 0x{expected:08x}, file: {file}")]
    BadChecksum {
        actual: u32,
        expected: u32,
        file: String,
    },
    #[error("dex file truncated: {0}")]
    FileTruncated(String),
    #[error("invalid dex container: {0}")]
    BadContainer(String),
    #[error("invalid instruction 0x{unit:04x} at code unit 0x{offset:x}")]
    BadInstruction { offset: u32, unit: u16 },
    #[error("zip error: {0}")]
    Zip(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
