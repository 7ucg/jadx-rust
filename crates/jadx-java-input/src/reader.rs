//! Bounds-checked big-endian cursor over a byte slice.

use crate::error::{JavaError, Result};

#[derive(Clone, Copy)]
pub(crate) struct Reader<'a> {
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

    pub fn skip(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n);
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        match self.pos.checked_add(n) {
            Some(end) if end <= self.data.len() => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            _ => Err(JavaError::Truncated {
                offset: self.pos,
                wanted: n,
                size: self.data.len(),
            }),
        }
    }

    pub fn u1(&mut self) -> Result<u32> {
        Ok(self.bytes(1)?[0] as u32)
    }

    pub fn s1(&mut self) -> Result<i32> {
        Ok(self.bytes(1)?[0] as i8 as i32)
    }

    pub fn u2(&mut self) -> Result<u32> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]) as u32)
    }

    pub fn s2(&mut self) -> Result<i32> {
        let b = self.bytes(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]) as i32)
    }

    pub fn u4(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn s4(&mut self) -> Result<i32> {
        Ok(self.u4()? as i32)
    }

    pub fn u8(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}
