//! Little endian cursor over a byte slice with the read semantics of the reference parser stream:
//! reads past the end yield `-1` bytes (and still advance the position counter), skipping past
//! the end is an error and a negative skip only moves the position counter.

use crate::error::{ResError, Result};

pub struct ParserStream<'a> {
    data: &'a [u8],
    /// Position in the data.
    real: usize,
    /// Position counter of the reference stream.
    pos: i64,
    mark_real: usize,
    mark_pos: i64,
}

impl<'a> ParserStream<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        ParserStream {
            data,
            real: 0,
            pos: 0,
            mark_real: 0,
            mark_pos: 0,
        }
    }

    pub fn pos(&self) -> u64 {
        self.pos.max(0) as u64
    }

    fn read_byte(&mut self) -> i32 {
        self.pos += 1;
        if self.real < self.data.len() {
            let b = i32::from(self.data[self.real]);
            self.real += 1;
            b
        } else {
            -1
        }
    }

    /// Bytes of the data left.
    pub fn remaining(&self) -> i64 {
        (self.data.len() - self.real) as i64
    }

    pub fn read_i8(&mut self) -> i32 {
        self.read_byte()
    }

    pub fn read_i16(&mut self) -> i32 {
        let b1 = self.read_byte();
        let b2 = self.read_byte();
        (b2 & 0xFF) << 8 | (b1 & 0xFF)
    }

    pub fn read_i32(&mut self) -> i32 {
        let b1 = self.read_byte();
        let b2 = self.read_byte();
        let b3 = self.read_byte();
        let b4 = self.read_byte();
        (b4 << 24) | ((b3 & 0xFF) << 16) | ((b2 & 0xFF) << 8) | (b1 & 0xFF)
    }

    pub fn read_u32(&mut self) -> u32 {
        self.read_i32() as u32
    }

    /// Fixed length UTF-16LE string (`len` chars), trimmed like `String.trim()`.
    pub fn read_string16_fixed(&mut self, len: usize) -> Result<String> {
        let bytes = self.read_bytes(len * 2)?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&units);
        Ok(java_trim(&s).to_string())
    }

    pub fn read_bytes(&mut self, count: usize) -> Result<Vec<u8>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        self.pos += count as i64;
        let end = self
            .real
            .checked_add(count)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| ResError::new(format!("No data, can't read {count} bytes")))?;
        let v = self.data[self.real..end].to_vec();
        self.real = end;
        Ok(v)
    }

    /// Skips bytes. A negative count only moves the position counter, like the reference stream.
    pub fn skip(&mut self, count: i64) -> Result<()> {
        self.pos += count;
        if count <= 0 {
            return Ok(());
        }
        let new_real = self.real as i64 + count;
        if new_real > self.data.len() as i64 {
            return Err(ResError::new(format!("No data, can't skip {count} bytes")));
        }
        self.real = new_real as usize;
        Ok(())
    }

    pub fn check_i8(&mut self, expected: i32, error: &str) -> Result<()> {
        let v = self.read_i8();
        if v != expected {
            return Err(self.mismatch(error, expected, v));
        }
        Ok(())
    }

    pub fn check_i16(&mut self, expected: i32, error: &str) -> Result<()> {
        let v = self.read_i16();
        if v != expected {
            return Err(self.mismatch(error, expected, v));
        }
        Ok(())
    }

    fn mismatch(&self, error: &str, expected: i32, actual: i32) -> ResError {
        ResError::new(format!(
            "{error}, expected: 0x{expected:x}, actual: 0x{actual:x}, offset: 0x{:x}",
            self.pos
        ))
    }

    pub fn check_pos(&self, expected: u64, error: &str) -> Result<()> {
        if self.pos != expected as i64 {
            return Err(ResError::new(format!(
                "{error}, expected offset: 0x{expected:x}, actual: 0x{:x}",
                self.pos
            )));
        }
        Ok(())
    }

    pub fn skip_to_pos(&mut self, expected: u64, error: &str) -> Result<()> {
        if self.pos > expected as i64 {
            return Err(ResError::new(format!(
                "{error}, expected offset not reachable: 0x{expected:x}, actual: 0x{:x}",
                self.pos
            )));
        }
        if self.pos < expected as i64 {
            self.skip(expected as i64 - self.pos)?;
        }
        self.check_pos(expected, error)
    }

    pub fn mark(&mut self) {
        self.mark_real = self.real;
        self.mark_pos = self.pos;
    }

    pub fn reset(&mut self) {
        self.real = self.mark_real;
        self.pos = self.mark_pos;
    }
}

/// `String.trim()`: strips leading and trailing chars up to U+0020.
pub fn java_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_and_eof() {
        let mut s = ParserStream::new(&[1, 2, 3, 4, 0xff]);
        assert_eq!(s.read_i32(), 0x04030201);
        assert_eq!(s.read_i8(), 0xff);
        assert_eq!(s.read_i8(), -1);
        assert_eq!(s.pos(), 6);
    }

    #[test]
    fn skip_to_pos_rules() {
        let mut s = ParserStream::new(&[0; 8]);
        s.skip_to_pos(4, "x").unwrap();
        assert!(s.skip_to_pos(2, "x").is_err());
        assert!(s.skip(10).is_err());
    }

    #[test]
    fn negative_skip_moves_only_the_counter() {
        let mut s = ParserStream::new(&[1, 2, 3, 4]);
        s.read_i16();
        s.skip(-2).unwrap();
        assert_eq!(s.pos(), 0);
        assert_eq!(s.read_i8(), 3);
    }

    #[test]
    fn trims_like_java() {
        assert_eq!(java_trim("\0 ab\0"), "ab");
    }
}
