//! Input plugin API: the jadx-normalised instruction model produced by input readers
//! (DEX now, JVM class files later) and consumed by the decompiler core.

mod insn;
mod opcode;

pub use insn::{
    ArrayData, ArrayPayload, FieldRefData, InsnData, InsnIndexType, InsnResolver, MethodProtoData,
    MethodRefData, Payload, SwitchPayload,
};
pub use opcode::Opcode;
