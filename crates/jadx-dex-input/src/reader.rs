//! Bounds-checked little-endian cursor over a byte slice.

use crate::error::{DexError, Result};

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Reader { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n);
        match end {
            Some(end) if end <= self.data.len() => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            _ => Err(DexError::Truncated {
                offset: self.pos,
                wanted: n,
                size: self.data.len(),
            }),
        }
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        self.u32().map(|v| v as i32)
    }

    /// Unsigned LEB128, at most 5 bytes (same limit as the Java reference).
    pub fn uleb128(&mut self) -> Result<u32> {
        let start = self.pos;
        let mut result: u32 = 0;
        let mut count = 0u32;
        loop {
            let cur = self.u8()?;
            result |= ((cur & 0x7f) as u32) << (count * 7);
            count += 1;
            if cur & 0x80 == 0 {
                return Ok(result);
            }
            if count >= 5 {
                return Err(DexError::BadLeb128(start));
            }
        }
    }

    /// `uleb128p1`: value minus one, so that 0 encodes "no index" (-1).
    pub fn uleb128p1(&mut self) -> Result<i32> {
        Ok((self.uleb128()? as i32).wrapping_sub(1))
    }

    /// Signed LEB128, at most 5 bytes.
    pub fn sleb128(&mut self) -> Result<i32> {
        let start = self.pos;
        let mut result: i32 = 0;
        let mut sign_bits: i32 = -1;
        let mut count = 0u32;
        let mut cur;
        loop {
            cur = self.u8()?;
            result |= ((cur & 0x7f) as i32) << (count * 7);
            sign_bits <<= 7;
            count += 1;
            if cur & 0x80 == 0 || count >= 5 {
                break;
            }
        }
        if cur & 0x80 != 0 {
            return Err(DexError::BadLeb128(start));
        }
        if (sign_bits >> 1) & result != 0 {
            result |= sign_bits;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uleb() {
        let mut r = Reader::new(&[0xe5, 0x8e, 0x26], 0);
        assert_eq!(r.uleb128().unwrap(), 624485);
        let mut r = Reader::new(&[0x00], 0);
        assert_eq!(r.uleb128p1().unwrap(), -1);
        let mut r = Reader::new(&[0xff, 0xff, 0xff, 0xff, 0x0f], 0);
        assert_eq!(r.uleb128().unwrap(), u32::MAX);
    }

    #[test]
    fn uleb_too_long() {
        let mut r = Reader::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x00], 0);
        assert!(r.uleb128().is_err());
    }

    #[test]
    fn sleb() {
        assert_eq!(Reader::new(&[0x02], 0).sleb128().unwrap(), 2);
        assert_eq!(Reader::new(&[0x7e], 0).sleb128().unwrap(), -2);
        assert_eq!(Reader::new(&[0xff, 0x00], 0).sleb128().unwrap(), 127);
        assert_eq!(Reader::new(&[0x81, 0x7f], 0).sleb128().unwrap(), -127);
        assert_eq!(Reader::new(&[0x80, 0x7f], 0).sleb128().unwrap(), -128);
    }

    #[test]
    fn truncated() {
        let mut r = Reader::new(&[1, 2, 3], 0);
        assert!(r.u32().is_err());
        let mut r = Reader::new(&[1, 2, 3], usize::MAX);
        assert!(r.u8().is_err());
    }
}
