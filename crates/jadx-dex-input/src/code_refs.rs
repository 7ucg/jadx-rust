//! Entities referenced by the instructions of a method.

use jadx_input_api::{
    CodeRef, CodeRefProvider, FieldRefData, InsnIndexType, MethodRefData, Opcode,
};

use crate::annotations::EncodedValue;
use crate::error::Result;
use crate::file::DexFile;
use crate::insns::decode_insn_data;

/// Method handle kinds that refer to a method (`invoke-static` and following).
const FIRST_METHOD_HANDLE_KIND: u16 = 0x04;
/// Position of the implementation method handle in the values of a call site
/// (`[linker, name, type, bootstrap arguments...]`).
const CALL_SITE_IMPL_HANDLE: usize = 4;

impl DexFile {
    /// Type, field and method operands of all instructions of the code item at `code_off`.
    pub fn code_refs(&self, code_off: u32) -> Result<Vec<CodeRef>> {
        let code = self.code_item(code_off)?;
        let mut out = Vec::new();
        for insn in decode_insn_data(&code.insns)? {
            if insn.opcode == Opcode::Unknown {
                continue;
            }
            match insn.index_type {
                InsnIndexType::TypeRef => {
                    out.push(CodeRef::Type(self.type_descriptor(insn.index)?));
                }
                InsnIndexType::FieldRef => {
                    let f = self.field_ref(insn.index)?;
                    out.push(CodeRef::Field(FieldRefData {
                        parent_class: f.class,
                        name: f.name,
                        ty: f.ty,
                    }));
                }
                InsnIndexType::MethodRef => {
                    out.push(CodeRef::Method(self.method_ref_data(insn.index)?));
                }
                InsnIndexType::CallSite => {
                    let values = self.call_site(insn.index)?;
                    if let Some(EncodedValue::MethodHandle(h)) = values.get(CALL_SITE_IMPL_HANDLE) {
                        let (kind, ref_idx) = self.method_handle(*h)?;
                        if kind >= FIRST_METHOD_HANDLE_KIND {
                            out.push(CodeRef::Method(self.method_ref_data(ref_idx)?));
                        }
                    }
                }
                InsnIndexType::None | InsnIndexType::StringRef => {}
            }
        }
        Ok(out)
    }

    fn method_ref_data(&self, idx: u32) -> Result<MethodRefData> {
        let m = self.method_ref(idx)?;
        Ok(MethodRefData {
            parent_class: m.class,
            name: m.name,
            arg_types: m.arg_types,
            return_type: m.return_type,
        })
    }
}

/// [`CodeRefProvider`] over the loaded dex files; `class_dex[i]` is the dex file of class `i`.
pub struct DexCodeRefs<'a> {
    pub dexes: &'a [DexFile],
    pub class_dex: &'a [usize],
}

impl CodeRefProvider for DexCodeRefs<'_> {
    fn code_refs(
        &self,
        class_idx: usize,
        code_offset: u32,
    ) -> std::result::Result<Vec<CodeRef>, String> {
        let dex = self
            .class_dex
            .get(class_idx)
            .and_then(|d| self.dexes.get(*d))
            .ok_or_else(|| format!("no dex file for class #{class_idx}"))?;
        dex.code_refs(code_offset).map_err(|e| e.to_string())
    }
}
