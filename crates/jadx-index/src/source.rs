//! Access to the decoded instructions of methods.
//!
//! The index never looks at raw input formats: a [`CodeSource`] hands out the instructions that
//! can carry a cross reference (invokes, field accesses, literals, type operands) with their
//! operands already resolved. [`DexCodeSource`] implements it for loaded dex files.

use jadx_dex_input::annotations::EncodedValue;
use jadx_dex_input::insns::decode_insn_data;
use jadx_dex_input::DexFile;
use jadx_input_api::{FieldRefData, InsnIndexType, MethodRefData, Opcode};

use crate::error::{IndexError, Result};

/// Operand of an instruction that carries a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    None,
    /// `const-string` value.
    Str(String),
    /// Type descriptor (`Lpkg/Cls;`, `[I`).
    Type(String),
    Field(FieldRefData),
    Method(MethodRefData),
}

/// One instruction with a cross reference.
#[derive(Debug, Clone)]
pub struct Insn {
    /// Offset in code units.
    pub offset: u32,
    pub opcode: Opcode,
    /// Literal operand (sign-extended 32-bit values for `const`, full value for `const-wide`).
    pub literal: i64,
    pub operand: Operand,
}

/// Decoded code of the methods of a loaded input.
pub trait CodeSource: Sync {
    /// Call `f` for every instruction of the method (code of class `class_idx` at
    /// `code_offset`, same numbering as the root) for which [`SiteKind::of`] is `Some`.
    fn visit_method(
        &self,
        class_idx: usize,
        code_offset: u32,
        f: &mut dyn FnMut(&Insn),
    ) -> Result<()>;
}

/// Kind of a cross reference site (stored in the posting records, so the values are stable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SiteKind {
    InvokeVirtual = 0,
    InvokeSuper = 1,
    InvokeDirect = 2,
    InvokeStatic = 3,
    InvokeInterface = 4,
    InvokePolymorphic = 5,
    InvokeCustom = 6,
    FieldRead = 7,
    FieldWrite = 8,
    StringLiteral = 9,
    IntLiteral = 10,
    NewInstance = 11,
    ConstClass = 12,
    CheckCast = 13,
    InstanceOf = 14,
    NewArray = 15,
}

impl SiteKind {
    pub fn from_u8(v: u8) -> Option<SiteKind> {
        use SiteKind::*;
        Some(match v {
            0 => InvokeVirtual,
            1 => InvokeSuper,
            2 => InvokeDirect,
            3 => InvokeStatic,
            4 => InvokeInterface,
            5 => InvokePolymorphic,
            6 => InvokeCustom,
            7 => FieldRead,
            8 => FieldWrite,
            9 => StringLiteral,
            10 => IntLiteral,
            11 => NewInstance,
            12 => ConstClass,
            13 => CheckCast,
            14 => InstanceOf,
            15 => NewArray,
            _ => return None,
        })
    }

    /// Site kind of an opcode, `None` for instructions that carry no cross reference.
    pub fn of(op: Opcode) -> Option<SiteKind> {
        use Opcode::*;
        Some(match op {
            InvokeVirtual | InvokeVirtualRange => SiteKind::InvokeVirtual,
            InvokeSuper | InvokeSuperRange => SiteKind::InvokeSuper,
            InvokeDirect | InvokeDirectRange | InvokeSpecial => SiteKind::InvokeDirect,
            InvokeStatic | InvokeStaticRange => SiteKind::InvokeStatic,
            InvokeInterface | InvokeInterfaceRange => SiteKind::InvokeInterface,
            InvokePolymorphic | InvokePolymorphicRange => SiteKind::InvokePolymorphic,
            InvokeCustom | InvokeCustomRange => SiteKind::InvokeCustom,
            Iget | Sget => SiteKind::FieldRead,
            Iput | Sput => SiteKind::FieldWrite,
            ConstString => SiteKind::StringLiteral,
            Const | ConstWide | AddIntLit | RsubInt | MulIntLit | DivIntLit | RemIntLit
            | AndIntLit | OrIntLit | XorIntLit | ShlIntLit | ShrIntLit | UshrIntLit => {
                SiteKind::IntLiteral
            }
            NewInstance => SiteKind::NewInstance,
            ConstClass => SiteKind::ConstClass,
            CheckCast => SiteKind::CheckCast,
            InstanceOf => SiteKind::InstanceOf,
            NewArray | FilledNewArray | FilledNewArrayRange => SiteKind::NewArray,
            _ => return None,
        })
    }

    pub fn is_invoke(self) -> bool {
        (self as u8) <= SiteKind::InvokeCustom as u8
    }

    pub fn is_type_use(self) -> bool {
        matches!(
            self,
            SiteKind::NewInstance
                | SiteKind::ConstClass
                | SiteKind::CheckCast
                | SiteKind::InstanceOf
                | SiteKind::NewArray
        )
    }

    pub fn name(self) -> &'static str {
        use SiteKind::*;
        match self {
            InvokeVirtual => "invoke-virtual",
            InvokeSuper => "invoke-super",
            InvokeDirect => "invoke-direct",
            InvokeStatic => "invoke-static",
            InvokeInterface => "invoke-interface",
            InvokePolymorphic => "invoke-polymorphic",
            InvokeCustom => "invoke-custom",
            FieldRead => "field-read",
            FieldWrite => "field-write",
            StringLiteral => "string",
            IntLiteral => "int-literal",
            NewInstance => "new-instance",
            ConstClass => "const-class",
            CheckCast => "check-cast",
            InstanceOf => "instance-of",
            NewArray => "new-array",
        }
    }
}

/// Position of the implementation method handle in the values of a call site
/// (`[linker, name, type, bootstrap arguments...]`).
const CALL_SITE_IMPL_HANDLE: usize = 4;
/// Method handle kinds that refer to a method (`invoke-static` and following).
const FIRST_METHOD_HANDLE_KIND: u16 = 0x04;

/// [`CodeSource`] over loaded dex files; `class_dex[i]` is the dex file of root class `i`.
pub struct DexCodeSource<'a> {
    pub dexes: &'a [DexFile],
    pub class_dex: &'a [usize],
}

fn src_err(e: impl std::fmt::Display) -> IndexError {
    IndexError::Source(e.to_string())
}

impl DexCodeSource<'_> {
    fn dex(&self, class_idx: usize) -> Result<&DexFile> {
        self.class_dex
            .get(class_idx)
            .and_then(|d| self.dexes.get(*d))
            .ok_or_else(|| IndexError::Source(format!("no dex file for class #{class_idx}")))
    }

    fn method_data(dex: &DexFile, idx: u32) -> Result<MethodRefData> {
        let m = dex.method_ref(idx).map_err(src_err)?;
        Ok(MethodRefData {
            parent_class: m.class,
            name: m.name,
            arg_types: m.arg_types,
            return_type: m.return_type,
        })
    }
}

impl CodeSource for DexCodeSource<'_> {
    fn visit_method(
        &self,
        class_idx: usize,
        code_offset: u32,
        f: &mut dyn FnMut(&Insn),
    ) -> Result<()> {
        let dex = self.dex(class_idx)?;
        let code = dex.code_item(code_offset).map_err(src_err)?;
        for insn in decode_insn_data(&code.insns).map_err(src_err)? {
            if SiteKind::of(insn.opcode).is_none() {
                continue;
            }
            let operand = match insn.index_type {
                InsnIndexType::TypeRef => {
                    Operand::Type(dex.type_descriptor(insn.index).map_err(src_err)?)
                }
                InsnIndexType::StringRef => Operand::Str(dex.string(insn.index).map_err(src_err)?),
                InsnIndexType::FieldRef => {
                    let r = dex.field_ref(insn.index).map_err(src_err)?;
                    Operand::Field(FieldRefData {
                        parent_class: r.class,
                        name: r.name,
                        ty: r.ty,
                    })
                }
                InsnIndexType::MethodRef => Operand::Method(Self::method_data(dex, insn.index)?),
                InsnIndexType::CallSite => {
                    let values = dex.call_site(insn.index).map_err(src_err)?;
                    match values.get(CALL_SITE_IMPL_HANDLE) {
                        Some(EncodedValue::MethodHandle(h)) => {
                            let (kind, ref_idx) = dex.method_handle(*h).map_err(src_err)?;
                            if kind >= FIRST_METHOD_HANDLE_KIND {
                                Operand::Method(Self::method_data(dex, ref_idx)?)
                            } else {
                                Operand::None
                            }
                        }
                        _ => Operand::None,
                    }
                }
                InsnIndexType::None => Operand::None,
            };
            f(&Insn {
                offset: insn.offset,
                opcode: insn.opcode,
                literal: insn.literal,
                operand,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_kinds_roundtrip_and_classify() {
        for v in 0..=15u8 {
            assert_eq!(SiteKind::from_u8(v).unwrap() as u8, v);
        }
        assert!(SiteKind::from_u8(16).is_none());
        assert_eq!(SiteKind::of(Opcode::Iput), Some(SiteKind::FieldWrite));
        assert_eq!(SiteKind::of(Opcode::Sget), Some(SiteKind::FieldRead));
        assert_eq!(SiteKind::of(Opcode::AddIntLit), Some(SiteKind::IntLiteral));
        assert_eq!(SiteKind::of(Opcode::Goto), None);
        assert!(SiteKind::InvokeCustom.is_invoke());
        assert!(!SiteKind::FieldRead.is_invoke());
        assert!(SiteKind::CheckCast.is_type_use());
    }
}
