//! Conversion of JVM instructions to the jadx-normalised [`InsnData`].
//!
//! The JVM is a stack machine, the normalised model is register based: every operand stack
//! slot gets a register (`0..=max_stack`, one extra register is reserved for `swap`) and local
//! variable `n` lives in register `max_stack + 1 + n`. A wide value (`long`, `double`) occupies
//! one stack slot (and one register) but two local variable slots.

use std::collections::{HashMap, HashSet};

use jadx_input_api::{InsnData, InsnIndexType, Opcode, Payload, SwitchPayload};

use crate::attrs::{Slot, StackFrame};
use crate::class::{
    JavaClass, TAG_CLASS, TAG_DOUBLE, TAG_FLOAT, TAG_INTEGER, TAG_LONG, TAG_STRING, TAG_UTF8,
};
use crate::error::{JavaError, Result};
use crate::reader::Reader;

#[derive(Clone, Copy)]
struct Info {
    /// Operand bytes after the opcode, `-1` if the decoder computes it.
    payload: i32,
    /// Declared register count, `-1` if the decoder computes it.
    regs: i32,
    opcode: Opcode,
    index_type: InsnIndexType,
}

fn info(op: u32) -> Option<Info> {
    use InsnIndexType as T;
    use Opcode as O;
    let i = |payload, regs, opcode, index_type| {
        Some(Info {
            payload,
            regs,
            opcode,
            index_type,
        })
    };
    let n = T::None;
    match op {
        0x00 => i(0, 0, O::Nop, n),
        0x01..=0x08 | 0x0b..=0x0d => i(0, 1, O::Const, n),
        0x09 | 0x0a | 0x0e | 0x0f => i(0, 1, O::ConstWide, n),
        0x10 => i(1, 2, O::Const, n),
        0x11 => i(2, 2, O::Const, n),
        0x12 => i(1, 2, O::Const, n),
        0x13 | 0x14 => i(2, 2, O::Const, n),
        0x15 | 0x17 | 0x19 => i(1, 2, O::Move, n),
        0x16 | 0x18 => i(1, 2, O::MoveWide, n),
        0x1a..=0x1d | 0x22..=0x25 | 0x2a..=0x2d => i(0, 2, O::Move, n),
        0x1e..=0x21 | 0x26..=0x29 => i(0, 2, O::MoveWide, n),
        0x2e | 0x30 => i(0, 3, O::Aget, n),
        0x2f | 0x31 => i(0, 3, O::AgetWide, n),
        0x32 => i(0, 3, O::AgetObject, n),
        0x33 => i(0, 3, O::AgetByteBoolean, n),
        0x34 => i(0, 3, O::AgetChar, n),
        0x35 => i(0, 3, O::AgetShort, n),
        0x36 | 0x38 | 0x3a => i(1, 2, O::Move, n),
        0x37 | 0x39 => i(1, 2, O::MoveWide, n),
        0x3b..=0x3e | 0x43..=0x46 | 0x4b..=0x4e => i(0, 2, O::Move, n),
        0x3f..=0x42 | 0x47..=0x4a => i(0, 2, O::MoveWide, n),
        0x4f | 0x51 => i(0, 3, O::Aput, n),
        0x50 | 0x52 => i(0, 3, O::AputWide, n),
        0x53 => i(0, 3, O::AputObject, n),
        0x54 => i(0, 3, O::AputByteBoolean, n),
        0x55 => i(0, 3, O::AputChar, n),
        0x56 => i(0, 3, O::AputShort, n),
        0x57 | 0x58 => i(0, 0, O::Nop, n),
        0x59 => i(0, 2, O::Move, n),
        0x5a => i(0, 6, O::MoveMulti, n),
        0x5b => i(0, 8, O::MoveMulti, n),
        0x5c => i(0, 4, O::MoveMulti, n),
        0x5d => i(0, 10, O::MoveMulti, n),
        0x5e => i(0, 12, O::MoveMulti, n),
        0x5f => i(0, 6, O::MoveMulti, n),
        0x60 => i(0, 3, O::AddInt, n),
        0x61 => i(0, 3, O::AddLong, n),
        0x62 => i(0, 3, O::AddFloat, n),
        0x63 => i(0, 3, O::AddDouble, n),
        0x64 => i(0, 3, O::SubInt, n),
        0x65 => i(0, 3, O::SubLong, n),
        0x66 => i(0, 3, O::SubFloat, n),
        0x67 => i(0, 3, O::SubDouble, n),
        0x68 => i(0, 3, O::MulInt, n),
        0x69 => i(0, 3, O::MulLong, n),
        0x6a => i(0, 3, O::MulFloat, n),
        0x6b => i(0, 3, O::MulDouble, n),
        0x6c => i(0, 3, O::DivInt, n),
        0x6d => i(0, 3, O::DivLong, n),
        0x6e => i(0, 3, O::DivFloat, n),
        0x6f => i(0, 3, O::DivDouble, n),
        0x70 => i(0, 3, O::RemInt, n),
        0x71 => i(0, 3, O::RemLong, n),
        0x72 => i(0, 3, O::RemFloat, n),
        0x73 => i(0, 3, O::RemDouble, n),
        0x74 => i(0, 2, O::NegInt, n),
        0x75 => i(0, 2, O::NegLong, n),
        0x76 => i(0, 2, O::NegFloat, n),
        0x77 => i(0, 2, O::NegDouble, n),
        0x78 => i(0, 3, O::ShlInt, n),
        0x79 => i(0, 3, O::ShlLong, n),
        0x7a => i(0, 3, O::ShrInt, n),
        0x7b => i(0, 3, O::ShrLong, n),
        0x7c => i(0, 3, O::UshrInt, n),
        0x7d => i(0, 3, O::UshrLong, n),
        0x7e => i(0, 3, O::AndInt, n),
        0x7f => i(0, 3, O::AndLong, n),
        0x80 => i(0, 3, O::OrInt, n),
        0x81 => i(0, 3, O::OrLong, n),
        0x82 => i(0, 3, O::XorInt, n),
        0x83 => i(0, 3, O::XorLong, n),
        0x84 => i(2, 2, O::AddIntLit, n),
        0x85 => i(0, 2, O::IntToLong, n),
        0x86 => i(0, 2, O::IntToFloat, n),
        0x87 => i(0, 2, O::IntToDouble, n),
        0x88 => i(0, 2, O::LongToInt, n),
        0x89 => i(0, 2, O::LongToFloat, n),
        0x8a => i(0, 2, O::LongToDouble, n),
        0x8b => i(0, 2, O::FloatToInt, n),
        0x8c => i(0, 2, O::FloatToLong, n),
        0x8d => i(0, 2, O::FloatToDouble, n),
        0x8e => i(0, 2, O::DoubleToInt, n),
        0x8f => i(0, 2, O::DoubleToLong, n),
        0x90 => i(0, 2, O::DoubleToFloat, n),
        0x91 => i(0, 2, O::IntToByte, n),
        0x92 => i(0, 2, O::IntToChar, n),
        0x93 => i(0, 2, O::IntToShort, n),
        0x94 => i(0, 3, O::CmpLong, n),
        0x95 => i(0, 3, O::CmplFloat, n),
        0x96 => i(0, 3, O::CmpgFloat, n),
        0x97 => i(0, 3, O::CmplDouble, n),
        0x98 => i(0, 3, O::CmpgDouble, n),
        0x99 | 0xc6 => i(2, 1, O::IfEqz, n),
        0x9a | 0xc7 => i(2, 1, O::IfNez, n),
        0x9b => i(2, 1, O::IfLtz, n),
        0x9c => i(2, 1, O::IfGez, n),
        0x9d => i(2, 1, O::IfGtz, n),
        0x9e => i(2, 1, O::IfLez, n),
        0x9f | 0xa5 => i(2, 2, O::IfEq, n),
        0xa0 | 0xa6 => i(2, 2, O::IfNe, n),
        0xa1 => i(2, 2, O::IfLt, n),
        0xa2 => i(2, 2, O::IfGe, n),
        0xa3 => i(2, 2, O::IfGt, n),
        0xa4 => i(2, 2, O::IfLe, n),
        0xa7 => i(2, 0, O::Goto, n),
        0xa8 => i(2, 1, O::JavaJsr, n),
        0xa9 => i(1, 1, O::JavaRet, n),
        0xaa => i(-1, 1, O::PackedSwitch, n),
        0xab => i(-1, 1, O::SparseSwitch, n),
        0xac..=0xb0 => i(0, 1, O::Return, n),
        0xb1 => i(0, 0, O::ReturnVoid, n),
        0xb2 => i(2, 1, O::Sget, T::FieldRef),
        0xb3 => i(2, 1, O::Sput, T::FieldRef),
        0xb4 => i(2, 2, O::Iget, T::FieldRef),
        0xb5 => i(2, 2, O::Iput, T::FieldRef),
        0xb6 => i(2, -1, O::InvokeVirtual, T::MethodRef),
        0xb7 => i(2, -1, O::InvokeSpecial, T::MethodRef),
        0xb8 => i(2, -1, O::InvokeStatic, T::MethodRef),
        0xb9 => i(4, -1, O::InvokeInterface, T::MethodRef),
        0xba => i(4, -1, O::InvokeCustom, T::CallSite),
        0xbb => i(2, 1, O::NewInstance, T::TypeRef),
        0xbc => i(1, 2, O::NewArray, T::TypeRef),
        0xbd => i(2, 2, O::NewArray, T::TypeRef),
        0xbe => i(0, 2, O::ArrayLength, n),
        0xbf => i(0, 1, O::Throw, n),
        0xc0 => i(2, 2, O::CheckCast, T::TypeRef),
        0xc1 => i(2, 2, O::InstanceOf, T::TypeRef),
        0xc2 => i(0, 1, O::MonitorEnter, n),
        0xc3 => i(0, 1, O::MonitorExit, n),
        0xc4 => i(-1, -1, O::Nop, n),
        0xc5 => i(3, -1, O::NewArray, T::TypeRef),
        0xc8 => i(4, 0, O::Goto, n),
        0xc9 => i(4, 1, O::JavaJsr, n),
        _ => None,
    }
}

const FLOAT_ONE: i64 = 0x3f80_0000;
const FLOAT_TWO: i64 = 0x4000_0000;
const DOUBLE_ONE: i64 = 0x3ff0_0000_0000_0000;

/// Operand stack with one register per slot.
#[derive(Clone)]
struct Stack {
    pos: i32,
    slots: Vec<Slot>,
}

impl Stack {
    fn new(size: usize) -> Stack {
        Stack {
            pos: -1,
            slots: vec![Slot::Unset; size],
        }
    }

    fn from_frame(size: usize, frame: &[Slot]) -> Result<Stack> {
        if frame.len() > size {
            return Err(JavaError::format("stack frame is bigger than max stack"));
        }
        let mut s = Stack::new(size);
        s.pos = frame.len() as i32 - 1;
        s.slots[..frame.len()].copy_from_slice(frame);
        Ok(s)
    }

    fn in_range(&self, p: i32) -> bool {
        p >= 0 && (p as usize) < self.slots.len()
    }

    fn peek_at(&self, at: i32) -> i32 {
        self.pos - at
    }

    fn peek_type_at(&self, at: i32) -> Slot {
        let p = self.pos - at;
        if self.in_range(p) {
            self.slots[p as usize]
        } else {
            Slot::Narrow
        }
    }

    fn insert(&mut self, at: i32, ty: Slot) -> Result<()> {
        let p = self.pos - at;
        if p < 0 || (p + 1 + at) as usize > self.slots.len() {
            return Err(JavaError::format("stack index out of bounds"));
        }
        let (p, at) = (p as usize, at as usize);
        self.slots.copy_within(p..p + at, p + 1);
        self.slots[p] = ty;
        self.pos += 1;
        Ok(())
    }

    fn push(&mut self, ty: Slot) -> i32 {
        self.pos += 1;
        let p = self.pos;
        if self.in_range(p) {
            self.slots[p as usize] = ty;
        }
        p
    }

    fn pop(&mut self) -> i32 {
        let p = self.pos;
        self.pos -= 1;
        p
    }

    fn clear(&mut self) {
        self.pos = -1;
    }
}

/// The instruction being decoded.
struct Cur {
    offset: u32,
    opcode: Opcode,
    regs: Vec<Option<i32>>,
    regs_count: i32,
    literal: i64,
    target: i64,
    index: u32,
    result: Option<u32>,
    payload: Option<Payload>,
    payload_size: i32,
}

pub(crate) struct Decoder<'a> {
    cls: &'a JavaClass,
    r: Reader<'a>,
    /// Number of operand stack registers (max stack plus the register for `swap`).
    max_stack: i32,
    exc_handlers: HashSet<u32>,
    frames: HashMap<u32, Vec<Slot>>,
    jump_stack: HashMap<i32, Stack>,
    stack: Stack,
    exc_handler: bool,
    cur: Cur,
}

impl<'a> Decoder<'a> {
    pub fn new(
        cls: &'a JavaClass,
        code_pos: usize,
        max_stack: u32,
        exc_handlers: HashSet<u32>,
        frames: Vec<StackFrame>,
    ) -> Decoder<'a> {
        let size = max_stack as usize + 1;
        Decoder {
            cls,
            r: cls.reader_at(code_pos),
            max_stack: size as i32,
            exc_handlers,
            frames: frames.into_iter().map(|f| (f.offset, f.stack)).collect(),
            jump_stack: HashMap::new(),
            stack: Stack::new(size),
            exc_handler: false,
            cur: Cur {
                offset: 0,
                opcode: Opcode::Nop,
                regs: Vec::new(),
                regs_count: 0,
                literal: 0,
                target: 0,
                index: 0,
                result: None,
                payload: None,
                payload_size: 0,
            },
        }
    }

    /// Decode all instructions of `code_length` bytes of code.
    pub fn decode_all(&mut self, code_length: u32) -> Result<Vec<InsnData>> {
        let mut out = Vec::new();
        let mut offset = 0u32;
        while offset < code_length {
            let op = self.r.u1()?;
            let info = info(op).ok_or(JavaError::UnknownOpcode(op))?;
            self.cur = Cur {
                offset,
                opcode: info.opcode,
                regs: Vec::new(),
                regs_count: info.regs,
                literal: 0,
                target: 0,
                index: 0,
                result: None,
                payload: None,
                payload_size: info.payload,
            };
            self.on_insn(offset)?;
            self.decode(op)?;
            if self.exc_handler && self.cur.opcode == Opcode::Move {
                self.cur.opcode = Opcode::MoveException;
                self.cur.regs_count = 1;
            }
            let c = &self.cur;
            let mut regs: Vec<Option<i32>> = c.regs.clone();
            regs.truncate(c.regs_count.max(0) as usize);
            while regs.last() == Some(&None) {
                regs.pop();
            }
            let length = (1 + c.payload_size.max(0)) as u32;
            out.push(InsnData {
                offset,
                length,
                opcode: c.opcode,
                raw_opcode_unit: op as u16,
                index_type: info.index_type,
                regs: regs.into_iter().map(|r| r.unwrap_or(0) as u32).collect(),
                literal: c.literal,
                target: c.target,
                index: c.index,
                result_reg: c.result,
                payload: c.payload.clone(),
            });
            offset += length;
        }
        Ok(out)
    }

    fn on_insn(&mut self, offset: u32) -> Result<()> {
        let key = offset as i32;
        let new_stack = if let Some(s) = self.jump_stack.get(&key) {
            Some(s.clone())
        } else if let Some(frame) = self.frames.get(&offset) {
            Some(Stack::from_frame(self.max_stack as usize, frame)?)
        } else {
            None
        };
        if let Some(s) = new_stack {
            self.stack = s;
        }
        if self.exc_handlers.contains(&offset) {
            self.stack.clear();
            self.stack.push(Slot::Narrow); // the exception
            self.exc_handler = true;
        } else {
            self.exc_handler = false;
        }
        Ok(())
    }

    fn register_jump(&mut self, target: i32) {
        if !self.jump_stack.contains_key(&target) {
            self.jump_stack.insert(target, self.stack.clone());
        }
    }

    // ---- operations on the current instruction ----

    fn set_reg(&mut self, arg: usize, reg: i32) {
        if self.cur.regs.len() <= arg {
            self.cur.regs.resize(arg + 1, None);
        }
        self.cur.regs[arg] = Some(reg);
    }

    fn local(&mut self, arg: usize, local: u32) {
        self.set_reg(arg, self.max_stack.wrapping_add(local as i32));
    }

    fn pop(&mut self, arg: usize) {
        let p = self.stack.pop();
        self.set_reg(arg, p);
    }

    fn peek(&mut self, arg: usize) {
        self.set_reg(arg, self.stack.pos);
    }

    fn peek_from(&mut self, pos: i32, arg: usize) {
        self.set_reg(arg, self.stack.peek_at(pos));
    }

    fn push(&mut self, arg: usize, ty: Slot) -> i32 {
        let p = self.stack.push(ty);
        self.set_reg(arg, p);
        p
    }

    fn jump(&mut self, offset: i32) {
        let target = (self.cur.offset as i32).wrapping_add(offset);
        self.cur.target = target as i64;
        self.register_jump(target);
    }

    fn slot_for_type(ty: &str) -> Slot {
        if ty == "J" || ty == "D" {
            Slot::Wide
        } else {
            Slot::Narrow
        }
    }

    fn two_regs(&mut self, ty: Slot) {
        self.pop(2);
        self.pop(1);
        self.push(0, ty);
    }

    fn one_reg(&mut self, ty: Slot) {
        self.pop(1);
        self.push(0, ty);
    }

    fn decode(&mut self, op: u32) -> Result<()> {
        use Slot::{Narrow, Wide};
        match op {
            0x00 => {}
            0x01..=0x08 => {
                self.cur.literal = if op == 0x01 { 0 } else { op as i64 - 3 };
                self.push(0, Narrow);
            }
            0x09 | 0x0a => {
                self.cur.literal = op as i64 - 9;
                self.push(0, Wide);
            }
            0x0b..=0x0d => {
                self.cur.literal = match op {
                    0x0b => 0,
                    0x0c => FLOAT_ONE,
                    _ => FLOAT_TWO,
                };
                self.push(0, Narrow);
            }
            0x0e | 0x0f => {
                self.cur.literal = if op == 0x0e { 0 } else { DOUBLE_ONE };
                self.push(0, Wide);
            }
            0x10 => {
                self.cur.literal = self.r.s1()? as i64;
                self.push(0, Narrow);
            }
            0x11 => {
                self.cur.literal = self.r.s2()? as i64;
                self.push(0, Narrow);
            }
            0x12..=0x14 => self.load_const(op != 0x12)?,
            0x15 | 0x17 | 0x19 => {
                let n = self.r.u1()?;
                self.local(1, n);
                self.push(0, Narrow);
            }
            0x16 | 0x18 => {
                let n = self.r.u1()?;
                self.local(1, n);
                self.push(0, Wide);
            }
            0x1a..=0x2d => {
                // iload_0 .. aload_3: four variants per type
                let n = (op - 0x1a) % 4;
                let wide = matches!(op, 0x1e..=0x21 | 0x26..=0x29);
                self.local(1, n);
                self.push(0, if wide { Wide } else { Narrow });
            }
            0x2e | 0x30 | 0x32..=0x35 => {
                self.pop(2);
                self.pop(1);
                self.push(0, Narrow);
            }
            0x2f | 0x31 => {
                self.pop(2);
                self.pop(1);
                self.push(0, Wide);
            }
            0x36..=0x3a => {
                self.pop(1);
                let n = self.r.u1()?;
                self.local(0, n);
            }
            0x3b..=0x4e => {
                let n = (op - 0x3b) % 4;
                self.pop(1);
                self.local(0, n);
            }
            0x4f..=0x56 => {
                self.pop(0);
                self.pop(2);
                self.pop(1);
            }
            0x57 => {
                self.stack.pop();
            }
            0x58 => {
                let ty = self.stack.peek_type_at(0);
                self.stack.pop();
                if ty == Narrow {
                    self.stack.pop();
                }
            }
            0x59 => {
                self.peek(1);
                let ty = self.stack.peek_type_at(1);
                self.push(0, ty);
            }
            0x5a => {
                let ty = self.stack.peek_type_at(1);
                self.push(0, ty);
                self.peek_from(1, 1);
                self.peek_from(1, 2);
                self.peek_from(2, 3);
                self.peek_from(2, 4);
                self.peek_from(0, 5);
            }
            0x5b => {
                let ty = self.stack.peek_type_at(1);
                self.push(0, ty);
                self.peek_from(1, 1);
                self.peek_from(1, 2);
                self.peek_from(2, 3);
                self.peek_from(2, 4);
                self.peek_from(3, 5);
                self.peek_from(3, 6);
                self.peek_from(0, 7);
            }
            0x5c => {
                if self.stack.peek_type_at(0) == Narrow {
                    self.peek_from(0, 3);
                    self.peek_from(1, 1);
                    self.push(0, Narrow);
                    self.push(2, Narrow);
                } else {
                    self.peek(1);
                    let ty = self.stack.peek_type_at(1);
                    self.push(0, ty);
                }
            }
            0x5d => self.dup2_x1()?,
            0x5e => self.dup2_x2()?,
            0x5f => {
                self.peek_from(-1, 0);
                self.peek_from(1, 1);
                self.peek_from(1, 2);
                self.peek_from(0, 3);
                self.peek_from(0, 4);
                self.peek_from(-1, 5);
            }
            // long / double arithmetic and shifts produce a wide value
            0x60 | 0x62 | 0x64 | 0x66 | 0x68 | 0x6a | 0x6c | 0x6e | 0x70 | 0x72 | 0x78 | 0x7a
            | 0x7c | 0x7e | 0x80 | 0x82 => self.two_regs(Narrow),
            0x61 | 0x63 | 0x65 | 0x67 | 0x69 | 0x6b | 0x6d | 0x6f | 0x71 | 0x73 | 0x79 | 0x7b
            | 0x7d | 0x7f | 0x81 | 0x83 => self.two_regs(Wide),
            0x74 | 0x76 => self.one_reg(Narrow),
            0x75 | 0x77 => self.one_reg(Wide),
            0x84 => {
                let n = self.r.u1()?;
                self.local(0, n);
                self.local(1, n);
                self.cur.literal = self.r.s1()? as i64;
            }
            0x85 | 0x87 | 0x8a | 0x8c | 0x8d | 0x8f => self.one_reg(Wide),
            0x86 | 0x88 | 0x89 | 0x8b | 0x8e | 0x90..=0x93 => self.one_reg(Narrow),
            0x94..=0x98 => self.two_regs(Narrow),
            0x99..=0x9e | 0xc6 | 0xc7 => {
                self.pop(0);
                let off = self.r.s2()?;
                self.jump(off);
            }
            0x9f..=0xa6 => {
                self.pop(1);
                self.pop(0);
                let off = self.r.s2()?;
                self.jump(off);
            }
            0xa7 => {
                let off = self.r.s2()?;
                self.jump(off);
            }
            0xa8 => {
                self.push(0, Narrow);
                let off = self.r.s2()?;
                self.jump(off);
            }
            0xa9 => {
                let n = self.r.u1()?;
                self.local(0, n);
            }
            0xaa => self.switch(true)?,
            0xab => self.switch(false)?,
            0xac..=0xb0 => self.pop(0),
            0xb1 => {}
            0xb2 | 0xb4 => {
                self.cur.index = self.r.u2()?;
                if op == 0xb4 {
                    self.pop(1);
                }
                let ty = Self::slot_for_type(&self.cls.field_ref_type(self.cur.index)?);
                self.push(0, ty);
            }
            0xb3 => {
                self.cur.index = self.r.u2()?;
                self.pop(0);
            }
            0xb5 => {
                self.cur.index = self.r.u2()?;
                self.pop(0);
                self.pop(1);
            }
            0xb6..=0xba => self.invoke(op)?,
            0xbb => {
                self.cur.index = self.r.u2()?;
                self.push(0, Narrow);
            }
            0xbc | 0xbd => {
                self.cur.index = if op == 0xbc {
                    self.r.u1()?
                } else {
                    self.r.u2()?
                };
                self.pop(1);
                self.push(0, Narrow);
                self.cur.literal = 1;
            }
            0xbe => self.one_reg(Narrow),
            0xbf => {
                self.pop(0);
                self.stack.clear();
            }
            0xc0 | 0xc1 => {
                self.cur.index = self.r.u2()?;
                self.pop(1);
                self.push(0, Narrow);
            }
            0xc2 | 0xc3 => self.pop(0),
            0xc4 => self.wide()?,
            0xc5 => {
                self.cur.index = self.r.u2()?;
                let dim = self.r.u1()?;
                // the operand is the full array type, the dimensions are carried by the
                // register count
                self.cur.literal = 0;
                self.cur.regs_count = dim as i32 + 1;
                for i in (1..=dim as usize).rev() {
                    self.pop(i);
                }
                self.push(0, Narrow);
            }
            0xc8 => {
                let off = self.r.s4()?;
                self.jump(off);
            }
            0xc9 => {
                self.push(0, Narrow);
                let off = self.r.s4()?;
                self.jump(off);
            }
            _ => return Err(JavaError::UnknownOpcode(op)),
        }
        Ok(())
    }

    fn dup2_x1(&mut self) -> Result<()> {
        use Slot::{Narrow, Wide};
        if self.stack.peek_type_at(0) == Narrow {
            self.stack.insert(2, Narrow)?;
            self.stack.insert(2, Narrow)?;
            for (pos, arg) in [
                (0, 0),
                (2, 1),
                (1, 2),
                (3, 3),
                (2, 4),
                (4, 5),
                (3, 6),
                (0, 7),
                (4, 8),
                (1, 9),
            ] {
                self.peek_from(pos, arg);
            }
        } else {
            self.cur.regs_count = 6;
            self.stack.insert(1, Wide)?;
            for (pos, arg) in [(0, 0), (1, 1), (1, 2), (2, 3), (2, 4), (0, 5)] {
                self.peek_from(pos, arg);
            }
        }
        Ok(())
    }

    fn dup2_x2(&mut self) -> Result<()> {
        use Slot::{Narrow, Wide};
        if self.stack.peek_type_at(0) == Narrow {
            self.stack.insert(2, Narrow)?;
            self.stack.insert(2, Narrow)?;
            for (pos, arg) in [
                (0, 0),
                (2, 1),
                (1, 2),
                (3, 3),
                (2, 4),
                (4, 5),
                (3, 6),
                (5, 7),
                (4, 8),
                (0, 9),
                (5, 10),
                (1, 11),
            ] {
                self.peek_from(pos, arg);
            }
        } else {
            self.cur.regs_count = 8;
            self.stack.insert(2, Wide)?;
            for (pos, arg) in [
                (0, 0),
                (1, 1),
                (1, 2),
                (2, 3),
                (2, 4),
                (3, 5),
                (3, 6),
                (0, 7),
            ] {
                self.peek_from(pos, arg);
            }
        }
        Ok(())
    }

    fn load_const(&mut self, wide: bool) -> Result<()> {
        let index = if wide { self.r.u2()? } else { self.r.u1()? };
        let mut r = self.cls.entry_reader(index)?;
        let tag = self.cls.tag(index)?;
        match tag {
            TAG_INTEGER | TAG_FLOAT => {
                self.cur.literal = r.s4()? as i64;
                self.cur.opcode = Opcode::Const;
                self.push(0, Slot::Narrow);
            }
            TAG_LONG | TAG_DOUBLE => {
                self.cur.literal = r.u8()? as i64;
                self.cur.opcode = Opcode::ConstWide;
                self.push(0, Slot::Wide);
            }
            TAG_STRING => {
                self.cur.index = r.u2()?;
                self.cur.opcode = Opcode::ConstString;
                self.push(0, Slot::Narrow);
            }
            TAG_UTF8 => {
                self.cur.index = index;
                self.cur.opcode = Opcode::ConstString;
                self.push(0, Slot::Narrow);
            }
            TAG_CLASS => {
                self.cur.index = index;
                self.cur.opcode = Opcode::ConstClass;
                self.push(0, Slot::Narrow);
            }
            t => {
                return Err(JavaError::format(format!(
                    "Unsupported constant type: tag {t}"
                )))
            }
        }
        Ok(())
    }

    fn wide(&mut self) -> Result<()> {
        use Slot::{Narrow, Wide};
        let op = self.r.u1()?;
        if op == 0x84 {
            let n = self.r.u2()?;
            let c = self.r.s2()?;
            self.local(0, n);
            self.local(1, n);
            self.cur.literal = c as i64;
            self.cur.payload_size = 5;
            self.cur.regs_count = 2;
            self.cur.opcode = Opcode::AddIntLit;
            return Ok(());
        }
        let idx = self.r.u2()?;
        match op {
            0x15 | 0x17 | 0x19 => {
                self.local(1, idx);
                self.push(0, Narrow);
            }
            0x16 | 0x18 => {
                self.local(1, idx);
                self.push(0, Wide);
            }
            0x36..=0x3a => {
                self.pop(1);
                self.local(0, idx);
            }
            _ => {
                return Err(JavaError::format(format!(
                    "Unexpected opcode in 'wide': 0x{op:x}"
                )))
            }
        }
        self.cur.payload_size = 3;
        self.cur.regs_count = 2;
        self.cur.opcode = Opcode::Move;
        Ok(())
    }

    fn switch(&mut self, table: bool) -> Result<()> {
        let data_off = self.r.pos();
        let insn_off = self.cur.offset as i32;
        self.r.skip((3 - insn_off % 4) as usize);
        let def_target = insn_off.wrapping_add(self.r.s4()?);
        let (keys, rel_targets): (Vec<i32>, Vec<i32>) = if table {
            let low = self.r.s4()?;
            let high = self.r.s4()?;
            let count = high.wrapping_sub(low).wrapping_add(1);
            if count < 0 {
                return Err(JavaError::format("negative tableswitch size"));
            }
            self.pop(0);
            let mut keys = Vec::new();
            let mut targets = Vec::new();
            for i in 0..count {
                keys.push(low.wrapping_add(i));
                targets.push(self.r.s4()?);
            }
            (keys, targets)
        } else {
            let pairs = self.r.s4()?;
            if pairs < 0 {
                return Err(JavaError::format("negative lookupswitch size"));
            }
            self.pop(0);
            let mut keys = Vec::new();
            let mut targets = Vec::new();
            for _ in 0..pairs {
                keys.push(self.r.s4()?);
                targets.push(self.r.s4()?);
            }
            (keys, targets)
        };
        let mut targets = Vec::with_capacity(rel_targets.len());
        for rel in rel_targets {
            let t = insn_off.wrapping_add(rel);
            targets.push(t);
            self.register_jump(t);
        }
        self.cur.target = def_target as i64;
        self.register_jump(def_target);
        self.cur.payload = Some(Payload::Switch(SwitchPayload { keys, targets }));
        self.cur.payload_size = (self.r.pos() - data_off) as i32;
        Ok(())
    }

    fn invoke(&mut self, op: u32) -> Result<()> {
        let idx = self.r.s2()?;
        if op == 0xb9 || op == 0xba {
            self.r.skip(2);
        }
        if idx < 0 {
            return Err(JavaError::format("negative constant pool index"));
        }
        let idx = idx as u32;
        self.cur.index = idx;
        let (arg_types, ret, instance) = if op == 0xba {
            let cs = self.cls.call_site(idx)?;
            let proto = cs
                .proto()
                .ok_or_else(|| JavaError::format("call site without prototype"))?;
            // the receiver is already part of the prototype arguments
            (proto.arg_types.clone(), proto.return_type.clone(), false)
        } else {
            let m = self.cls.method_ref(idx)?;
            (m.arg_types, m.return_type, op != 0xb8)
        };
        let mut used: Vec<bool> = Vec::new();
        if instance {
            used.push(true);
        }
        for t in &arg_types {
            used.push(true);
            if t.starts_with('J') || t.starts_with('D') {
                used.push(false);
            }
        }
        self.cur.regs_count = used.len() as i32;
        self.cur.regs = vec![None; used.len()];
        for i in (0..used.len()).rev() {
            if used[i] {
                self.pop(i);
            } else {
                // the second slot of a wide argument
                self.set_reg(i, 0);
            }
        }
        if ret != "V" {
            let p = self.stack.push(Self::slot_for_type(&ret));
            // the reference treats register -1 as "no result"
            self.cur.result = (p != -1).then_some(p as u32);
        }
        Ok(())
    }
}
