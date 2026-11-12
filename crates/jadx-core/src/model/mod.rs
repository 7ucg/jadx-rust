//! Core data model.

pub mod cfg;
pub mod const_storage;
pub mod info;
pub mod insn;
pub mod names;
pub mod node;
pub mod ssa;
pub mod types;

pub use info::{ClassInfo, FieldInfo, MethodInfo};
pub use insn::{InsnArg, InsnKind, InsnNode, InsnType, LiteralArg, RegisterArg};
pub use types::{ArgType, PrimitiveType, WildcardBound};
pub mod bitset;
