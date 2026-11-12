//! Input plugin API: the jadx-normalised instruction model produced by input readers
//! (DEX now, JVM class files later) and consumed by the decompiler core.

mod debug;
mod insn;
mod opcode;

pub use debug::{DebugInfo, LocalVar};
pub use insn::{
    pack_annotations, AnnotationData, AnnotationRef, AnnotationVisibility, ArrayData, ArrayPayload,
    CallSiteValue, CatchData, ClassAttrs, ClassData, CodeRef, CodeRefProvider, ConstValue,
    EncodedValueData, FieldAttrs, FieldData, FieldRefData, InnerClassData, InsnData, InsnIndexType,
    InsnResolver, MethodAttrs, MethodData, MethodHandleData, MethodHandleKind, MethodHandleTarget,
    MethodProtoData, MethodRefData, Payload, SwitchPayload, TryData,
};
pub use opcode::Opcode;
