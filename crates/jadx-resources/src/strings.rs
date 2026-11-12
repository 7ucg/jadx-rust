//! String pools of binary XML files and resource tables.

use crate::consts::UTF8_FLAG;
use crate::error::Result;
use crate::stream::ParserStream;

pub const INVALID_STRING_PLACEHOLDER: &str = "\u{27e8}STRING_DECODE_ERROR\u{27e9}";

/// Raw string pool: the bytes following the pool header (offset table first, then the strings).
#[derive(Debug, Clone, Default)]
pub struct BinaryXmlStrings {
    string_count: i32,
    strings_start: i64,
    buffer: Vec<u8>,
    is_utf8: bool,
}

impl BinaryXmlStrings {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn size(&self) -> i32 {
        self.string_count
    }

    pub fn get(&self, id: i32) -> String {
        let limit = self.buffer.len() as i64;
        if i64::from(id) * 4 >= limit - 3 || id < 0 {
            return INVALID_STRING_PLACEHOLDER.to_string();
        }
        let p = id as usize * 4;
        let off = i32::from_le_bytes([
            self.buffer[p],
            self.buffer[p + 1],
            self.buffer[p + 2],
            self.buffer[p + 3],
        ]);
        if off < 0 {
            // unsigned offset larger than i32::MAX, only seen in obfuscated files
            return INVALID_STRING_PLACEHOLDER.to_string();
        }
        let offset = self.strings_start + i64::from(off);
        let offset = offset as i32;
        if offset < 0 {
            return INVALID_STRING_PLACEHOLDER.to_string();
        }
        let res = if self.is_utf8 {
            extract_string8(&self.buffer, offset as usize)
        } else {
            extract_string16(&self.buffer, offset as usize)
        };
        res.unwrap_or_else(|| INVALID_STRING_PLACEHOLDER.to_string())
    }
}

fn extract_string8(arr: &[u8], offset: usize) -> Option<String> {
    if offset >= arr.len() {
        return Some(INVALID_STRING_PLACEHOLDER.to_string());
    }
    let skip = if arr[offset] & 0x80 == 0 { 1 } else { 2 };
    let mut start = offset + skip;
    let mut len = i32::from(*arr.get(start)? as i8);
    start += 1;
    if len == 0 {
        return Some(String::new());
    }
    if len & 0x80 != 0 {
        len = (len & 0x7F) << 8 | i32::from(*arr.get(start)?);
        start += 1;
    }
    let len = len as usize;
    if start > arr.len() {
        return None;
    }
    let end = start + len;
    let mut bytes = arr[start..end.min(arr.len())].to_vec();
    // `Arrays.copyOfRange` pads with zeros
    bytes.resize(len, 0);
    Some(crate::jstr::java_utf8_decode(&bytes))
}

fn extract_string16(arr: &[u8], offset: usize) -> Option<String> {
    if offset + 2 >= arr.len() {
        return Some(INVALID_STRING_PLACEHOLDER.to_string());
    }
    let len = arr.len();
    let start = offset + if arr[offset + 1] & 0x80 == 0 { 2 } else { 4 };
    let mut end = start;
    loop {
        if end + 1 >= len {
            break;
        }
        if arr[end] == 0 && arr[end + 1] == 0 {
            break;
        }
        end += 2;
    }
    if start > end {
        return None;
    }
    let units: Vec<u16> = arr[start..end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Parses a string pool chunk whose type was already consumed.
pub fn parse_string_pool_no_type(is: &mut ParserStream) -> Result<BinaryXmlStrings> {
    let start = is.pos() - 2;
    let _header_size = is.read_i16();
    let size = u64::from(is.read_u32());
    let chunk_end = start + size;
    parse_string_pool_no_size(is, start, chunk_end)
}

/// Parses a string pool chunk whose type and size were already consumed.
pub fn parse_string_pool_no_size(
    is: &mut ParserStream,
    start: u64,
    chunk_end: u64,
) -> Result<BinaryXmlStrings> {
    let string_count = is.read_i32();
    let _style_count = is.read_i32();
    let flags = is.read_i32();
    let mut strings_start = i64::from(is.read_i32());
    let _styles_start = is.read_i32();
    strings_start -= is.pos() as i64 - start as i64;
    let remaining = chunk_end as i64 - is.pos() as i64;
    if remaining < 0 {
        return Err(crate::error::ResError::new(
            "Expected strings pool end before current position",
        ));
    }
    let buffer = is.read_bytes(remaining as usize)?;
    is.check_pos(chunk_end, "Expected strings pool end")?;
    Ok(BinaryXmlStrings {
        string_count,
        strings_start,
        buffer,
        is_utf8: flags & UTF8_FLAG != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_utf16_pools() {
        // one string "ab": offset table [0], strings start at 4
        let mut buf8 = vec![0u8; 4];
        buf8.extend_from_slice(&[2, 2, b'a', b'b', 0]);
        let p8 = BinaryXmlStrings {
            string_count: 1,
            strings_start: 4,
            buffer: buf8,
            is_utf8: true,
        };
        assert_eq!(p8.get(0), "ab");
        let mut buf16 = vec![0u8; 4];
        buf16.extend_from_slice(&[2, 0, b'a', 0, b'b', 0, 0, 0]);
        let p16 = BinaryXmlStrings {
            string_count: 1,
            strings_start: 4,
            buffer: buf16,
            is_utf8: false,
        };
        assert_eq!(p16.get(0), "ab");
        assert_eq!(p16.get(5), INVALID_STRING_PLACEHOLDER);
    }
}
