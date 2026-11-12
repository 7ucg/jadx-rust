//! Input plugin API: the jadx-normalised instruction model produced by input readers
//! (DEX now, JVM class files later) and consumed by the decompiler core.

mod insn;
mod opcode;

pub use insn::{
    AnnotationRef, ArrayData, ArrayPayload, CatchData, ClassData, CodeRef, CodeRefProvider,
    ConstValue, FieldData, FieldRefData, InnerClassData, InsnData, InsnIndexType, InsnResolver,
    MethodData, MethodProtoData, MethodRefData, Payload, SwitchPayload, TryData,
};
pub use opcode::Opcode;
