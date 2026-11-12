//! Dalvik instruction set: opcode table and bytecode decoder.
//!
//! The opcode table (`opcodes_gen.rs`) is generated from the reference dexlib2 table so that
//! mnemonics, formats and reference kinds match the reference disassembler exactly.

mod convert;
mod decode;
#[rustfmt::skip]
mod jadx_map_gen;
#[rustfmt::skip]
mod opcodes_gen;

pub use convert::decode_insn_data;
pub use decode::{decode_all, decode_at, Insn, Payload};
pub use opcodes_gen::OPCODES;

use std::sync::OnceLock;

/// Instruction format (see the Dalvik "Bytecode formats" specification).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Format10t,
    Format10x,
    Format11n,
    Format11x,
    Format12x,
    Format20t,
    Format21c,
    Format21ih,
    Format21lh,
    Format21s,
    Format21t,
    Format22b,
    Format22c,
    Format22s,
    Format22t,
    Format22x,
    Format23x,
    Format30t,
    Format31c,
    Format31i,
    Format31t,
    Format32x,
    Format35c,
    Format3rc,
    Format45cc,
    Format4rcc,
    Format51l,
    ArrayPayload,
    PackedSwitchPayload,
    SparseSwitchPayload,
}

impl Format {
    /// Size in 16-bit code units for fixed-size formats; `None` for payloads.
    pub fn units(self) -> Option<u32> {
        use Format::*;
        Some(match self {
            Format10t | Format10x | Format11n | Format11x | Format12x => 1,
            Format20t | Format21c | Format21ih | Format21lh | Format21s | Format21t | Format22b
            | Format22c | Format22s | Format22t | Format22x | Format23x => 2,
            Format30t | Format31c | Format31i | Format31t | Format32x | Format35c | Format3rc => 3,
            Format45cc | Format4rcc => 4,
            Format51l => 5,
            ArrayPayload | PackedSwitchPayload | SparseSwitchPayload => return None,
        })
    }
}

/// What kind of table entry an instruction's index operand refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    None,
    String,
    Type,
    Field,
    Method,
    Proto,
    CallSite,
    MethodHandle,
}

#[derive(Debug, Clone, Copy)]
pub struct OpInfo {
    /// Opcode byte, or the full ident unit (`0x0100`..`0x0300`) for payload pseudo-instructions.
    pub value: u16,
    pub name: &'static str,
    pub format: Format,
    pub ref1: RefKind,
    pub ref2: RefKind,
    pub can_throw: bool,
    pub can_continue: bool,
}

struct Tables {
    by_byte: [Option<&'static OpInfo>; 256],
    packed: &'static OpInfo,
    sparse: &'static OpInfo,
    array: &'static OpInfo,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut by_byte = [None; 256];
        let (mut packed, mut sparse, mut array) = (None, None, None);
        for op in OPCODES {
            match op.value {
                0x0100 => packed = Some(op),
                0x0200 => sparse = Some(op),
                0x0300 => array = Some(op),
                v => by_byte[v as usize] = Some(op),
            }
        }
        Tables {
            by_byte,
            packed: packed.expect("packed-switch-payload in table"),
            sparse: sparse.expect("sparse-switch-payload in table"),
            array: array.expect("array-payload in table"),
        }
    })
}

/// Look up the opcode for a first code unit. Payload idents (`0x0100` etc.) are recognised.
pub fn lookup(unit: u16) -> Option<&'static OpInfo> {
    let t = tables();
    if unit & 0xFF == 0 && unit != 0 {
        return match unit {
            0x0100 => Some(t.packed),
            0x0200 => Some(t.sparse),
            0x0300 => Some(t.array),
            _ => None,
        };
    }
    t.by_byte[(unit & 0xFF) as usize]
}
