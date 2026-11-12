//! Input plugin API: the jadx-normalised instruction model produced by input readers
//! (DEX now, JVM class files later) and consumed by the decompiler core.

mod insn;
mod opcode;

pub use insn::{
    ArrayData, ArrayPayload, CatchData, ClassData, FieldData, FieldRefData, InsnData,
    InsnIndexType, InsnResolver, MethodData, MethodProtoData, MethodRefData, Payload,
    SwitchPayload, TryData,
};
pub use opcode::Opcode;
