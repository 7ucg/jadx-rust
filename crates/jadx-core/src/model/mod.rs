//! Core data model.

pub mod info;
pub mod insn;
pub mod node;
pub mod types;

pub use info::{ClassInfo, FieldInfo, MethodInfo};
pub use insn::{InsnArg, InsnKind, InsnNode, InsnType, LiteralArg, RegisterArg};
pub use types::{ArgType, PrimitiveType, WildcardBound};
