//! Bytecode decoder: raw 16-bit code units to structured instructions.

use super::{lookup, Format, OpInfo};
use crate::error::{DexError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    PackedSwitch {
        first_key: i32,
        targets: Vec<i32>,
    },
    SparseSwitch {
        keys: Vec<i32>,
        targets: Vec<i32>,
    },
    /// `element_width` bytes per element, `data.len() == element_width * count`.
    ArrayData {
        element_width: u16,
        data: Vec<u8>,
    },
}

/// A decoded instruction. Field usage depends on `info.format`:
///
/// * `regs[..reg_count]` – register operands in operand order (for 35c/45cc the register list,
///   for 3rc/4rcc the first register is `regs[0]` and `reg_count` the range length)
/// * `literal` – inline constant (already sign-extended / shifted as the format prescribes)
/// * `target` – branch/payload offset in code units, relative to this instruction
/// * `index`/`index2` – table indices (`index2` is the proto index of 45cc/4rcc)
#[derive(Debug, Clone)]
pub struct Insn {
    /// Offset in code units from the start of the method's instruction array.
    pub offset: u32,
    /// Length in code units.
    pub len: u32,
    pub info: &'static OpInfo,
    pub regs: [u32; 5],
    pub reg_count: u8,
    pub literal: i64,
    pub target: i32,
    pub index: u32,
    pub index2: u32,
    pub payload: Option<Payload>,
}

impl Insn {
    pub fn regs(&self) -> &[u32] {
        &self.regs[..self.reg_count as usize]
    }
    /// First register of a range instruction (3rc/4rcc).
    pub fn range_start(&self) -> u32 {
        self.regs[0]
    }
    pub fn is_range(&self) -> bool {
        matches!(self.info.format, Format::Format3rc | Format::Format4rcc)
    }
}

fn bad(offset: u32, unit: u16) -> DexError {
    DexError::BadInstruction { offset, unit }
}

fn unit(units: &[u16], at: usize, insn_off: u32, first: u16) -> Result<u16> {
    units.get(at).copied().ok_or_else(|| bad(insn_off, first))
}

fn s32(lo: u16, hi: u16) -> i32 {
    (lo as u32 | (hi as u32) << 16) as i32
}

/// Decode the instruction at unit offset `pos` of `units`.
pub fn decode_at(units: &[u16], pos: usize) -> Result<Insn> {
    let off = pos as u32;
    let u0 = *units.get(pos).ok_or_else(|| bad(off, 0))?;
    let info = lookup(u0).ok_or_else(|| bad(off, u0))?;
    let mut insn = Insn {
        offset: off,
        len: 0,
        info,
        regs: [0; 5],
        reg_count: 0,
        literal: 0,
        target: 0,
        index: 0,
        index2: 0,
        payload: None,
    };
    let g = |at: usize| unit(units, pos + at, off, u0);
    let a8 = (u0 >> 8) as u32;
    let nib_a = ((u0 >> 8) & 0xF) as u32;
    let nib_b = ((u0 >> 12) & 0xF) as u32;
    use Format::*;
    match info.format {
        Format10x => insn.len = 1,
        Format10t => {
            insn.len = 1;
            insn.target = (a8 as u8) as i8 as i32;
        }
        Format11n => {
            insn.len = 1;
            insn.set_regs(&[nib_a]);
            insn.literal = (((nib_b as i32) << 28) >> 28) as i64;
        }
        Format11x => {
            insn.len = 1;
            insn.set_regs(&[a8]);
        }
        Format12x => {
            insn.len = 1;
            insn.set_regs(&[nib_a, nib_b]);
        }
        Format20t => {
            insn.len = 2;
            insn.target = g(1)? as i16 as i32;
        }
        Format21c => {
            insn.len = 2;
            insn.set_regs(&[a8]);
            insn.index = g(1)? as u32;
        }
        Format21ih => {
            insn.len = 2;
            insn.set_regs(&[a8]);
            insn.literal = ((g(1)? as i16 as i32) << 16) as i64;
        }
        Format21lh => {
            insn.len = 2;
            insn.set_regs(&[a8]);
            insn.literal = (g(1)? as i16 as i64) << 48;
        }
        Format21s => {
            insn.len = 2;
            insn.set_regs(&[a8]);
            insn.literal = g(1)? as i16 as i64;
        }
        Format21t => {
            insn.len = 2;
            insn.set_regs(&[a8]);
            insn.target = g(1)? as i16 as i32;
        }
        Format22b => {
            insn.len = 2;
            let u1 = g(1)?;
            insn.set_regs(&[a8, (u1 & 0xFF) as u32]);
            insn.literal = (u1 >> 8) as u8 as i8 as i64;
        }
        Format22c => {
            insn.len = 2;
            insn.set_regs(&[nib_a, nib_b]);
            insn.index = g(1)? as u32;
        }
        Format22s => {
            insn.len = 2;
            insn.set_regs(&[nib_a, nib_b]);
            insn.literal = g(1)? as i16 as i64;
        }
        Format22t => {
            insn.len = 2;
            insn.set_regs(&[nib_a, nib_b]);
            insn.target = g(1)? as i16 as i32;
        }
        Format22x => {
            insn.len = 2;
            insn.set_regs(&[a8, g(1)? as u32]);
        }
        Format23x => {
            insn.len = 2;
            let u1 = g(1)?;
            insn.set_regs(&[a8, (u1 & 0xFF) as u32, (u1 >> 8) as u32]);
        }
        Format30t => {
            insn.len = 3;
            insn.target = s32(g(1)?, g(2)?);
        }
        Format31c => {
            insn.len = 3;
            insn.set_regs(&[a8]);
            insn.index = s32(g(1)?, g(2)?) as u32;
        }
        Format31i => {
            insn.len = 3;
            insn.set_regs(&[a8]);
            insn.literal = s32(g(1)?, g(2)?) as i64;
        }
        Format31t => {
            insn.len = 3;
            insn.set_regs(&[a8]);
            insn.target = s32(g(1)?, g(2)?);
        }
        Format32x => {
            insn.len = 3;
            insn.set_regs(&[g(1)? as u32, g(2)? as u32]);
        }
        Format35c | Format45cc => {
            insn.len = if info.format == Format35c { 3 } else { 4 };
            let count = nib_b as usize;
            if count > 5 {
                return Err(bad(off, u0));
            }
            insn.index = g(1)? as u32;
            let u2 = g(2)?;
            let g_reg = nib_a;
            let all = [
                (u2 & 0xF) as u32,
                ((u2 >> 4) & 0xF) as u32,
                ((u2 >> 8) & 0xF) as u32,
                ((u2 >> 12) & 0xF) as u32,
                g_reg,
            ];
            insn.set_regs(&all[..count]);
            if info.format == Format45cc {
                insn.index2 = g(3)? as u32;
            }
        }
        Format3rc | Format4rcc => {
            insn.len = if info.format == Format3rc { 3 } else { 4 };
            insn.index = g(1)? as u32;
            insn.regs[0] = g(2)? as u32;
            insn.reg_count = 0;
            insn.literal = a8 as i64; // register count of the range
            if info.format == Format4rcc {
                insn.index2 = g(3)? as u32;
            }
        }
        Format51l => {
            insn.len = 5;
            insn.set_regs(&[a8]);
            let v =
                (g(1)? as u64) | (g(2)? as u64) << 16 | (g(3)? as u64) << 32 | (g(4)? as u64) << 48;
            insn.literal = v as i64;
        }
        PackedSwitchPayload => {
            let size = g(1)? as usize;
            insn.len = (size * 2 + 4) as u32;
            let first_key = s32(g(2)?, g(3)?);
            let mut targets = Vec::with_capacity(size);
            for i in 0..size {
                targets.push(s32(g(4 + i * 2)?, g(5 + i * 2)?));
            }
            insn.payload = Some(Payload::PackedSwitch { first_key, targets });
        }
        SparseSwitchPayload => {
            let size = g(1)? as usize;
            insn.len = (size * 4 + 2) as u32;
            let mut keys = Vec::with_capacity(size);
            let mut targets = Vec::with_capacity(size);
            for i in 0..size {
                keys.push(s32(g(2 + i * 2)?, g(3 + i * 2)?));
            }
            for i in 0..size {
                let b = 2 + size * 2 + i * 2;
                targets.push(s32(g(b)?, g(b + 1)?));
            }
            insn.payload = Some(Payload::SparseSwitch { keys, targets });
        }
        ArrayPayload => {
            let width = g(1)?;
            if !matches!(width, 0 | 1 | 2 | 4 | 8) {
                return Err(bad(off, u0));
            }
            let size = (g(2)? as u32 | (g(3)? as u32) << 16) as usize;
            let byte_len = size
                .checked_mul(width as usize)
                .filter(|n| *n <= units.len().saturating_mul(2))
                .ok_or_else(|| bad(off, u0))?;
            insn.len = (4 + byte_len.div_ceil(2)) as u32;
            let mut data = Vec::with_capacity(byte_len);
            for i in 0..byte_len.div_ceil(2) {
                let w = g(4 + i)?;
                data.push(w as u8);
                data.push((w >> 8) as u8);
            }
            data.truncate(byte_len);
            insn.payload = Some(Payload::ArrayData {
                element_width: width,
                data,
            });
        }
    }
    Ok(insn)
}

impl Insn {
    fn set_regs(&mut self, regs: &[u32]) {
        self.regs[..regs.len()].copy_from_slice(regs);
        self.reg_count = regs.len() as u8;
    }
}

/// Decode a whole instruction array.
pub fn decode_all(units: &[u16]) -> Result<Vec<Insn>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < units.len() {
        let insn = decode_at(units, pos)?;
        pos += insn.len as usize;
        out.push(insn);
    }
    Ok(out)
}
