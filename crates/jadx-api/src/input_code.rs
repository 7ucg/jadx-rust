//! Method code of the loaded inputs: dex files and JVM class files behind one interface.

use std::collections::HashMap;

use jadx_dex_input::DexFile;
use jadx_input_api::{CodeRef, CodeRefProvider, InsnData, InsnResolver, TryData};
use jadx_java_input::JavaClass as JavaInputClass;

use crate::generator::GenContext;

/// Where a class of the class tree was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassOrigin {
    /// Index into the loaded dex files.
    Dex(usize),
    /// Index into the loaded JVM classes.
    Java(usize),
    /// Class created by the decompiler.
    Synthetic,
}

/// Code of one method in the normalised instruction form, plus the resolver of its operands.
pub struct MethodCode<'a> {
    pub insns: Vec<InsnData>,
    /// Code size in code units.
    pub units: usize,
    pub tries: Vec<TryData>,
    /// Number of registers of the method frame.
    pub registers: usize,
    /// Register of the first argument if the input defines it (class files).
    pub args_start_reg: Option<usize>,
    pub resolver: &'a dyn InsnResolver,
}

impl GenContext<'_> {
    /// Origin of the class `idx` of the class tree.
    pub fn origin_of(&self, idx: usize) -> ClassOrigin {
        self.class_origin
            .get(idx)
            .copied()
            .unwrap_or(ClassOrigin::Synthetic)
    }

    /// Load the code of the method whose code is at `code_offset` of class `cls`.
    pub fn method_code(&self, cls: usize, code_offset: u32) -> Result<MethodCode<'_>, String> {
        match self.origin_of(cls) {
            ClassOrigin::Dex(d) => {
                let dex = &self.dexes[d];
                let code = dex.code_item(code_offset).map_err(|e| e.to_string())?;
                let insns = jadx_dex_input::insns::decode_insn_data(&code.insns)
                    .map_err(|e| e.to_string())?;
                let tries = dex.tries_api(&code).map_err(|e| e.to_string())?;
                Ok(MethodCode {
                    insns,
                    units: code.insns.len(),
                    tries,
                    registers: code.registers_size as usize,
                    args_start_reg: None,
                    resolver: dex,
                })
            }
            ClassOrigin::Java(j) => {
                let class = &self.java_classes[j];
                let info = class.code_info(code_offset).map_err(|e| e.to_string())?;
                let insns = class
                    .decode_core_insns(code_offset)
                    .map_err(|e| e.to_string())?;
                let tries = class.tries(code_offset).map_err(|e| e.to_string())?;
                Ok(MethodCode {
                    insns,
                    units: info.code_length as usize,
                    tries,
                    registers: info.registers_count() as usize,
                    args_start_reg: Some(info.args_start_reg() as usize),
                    resolver: class,
                })
            }
            ClassOrigin::Synthetic => Err("class without code".to_string()),
        }
    }

    /// `DebugInfoAttachVisitor.setMethodSourceLine`: line of the first instruction with a line
    /// number, minus one (0 if the method has no line numbers).
    pub fn method_source_line(&self, cls: usize, code_offset: u32) -> u32 {
        let first_line = |lines: Vec<(u32, u32)>, insns: &[InsnData]| -> u32 {
            let map: HashMap<u32, u32> = lines.into_iter().collect();
            for insn in insns {
                if let Some(&line) = map.get(&insn.offset) {
                    if line != 0 {
                        return line - 1;
                    }
                }
            }
            0
        };
        match self.origin_of(cls) {
            ClassOrigin::Dex(d) => {
                let dex = &self.dexes[d];
                let Ok(code) = dex.code_item(code_offset) else {
                    return 0;
                };
                if code.debug_info_off == 0 {
                    return 0;
                }
                let Ok(lines) = dex.debug_lines(code.debug_info_off, code.insns.len() as u32)
                else {
                    return 0;
                };
                let Ok(raw) = jadx_dex_input::insns::decode_insn_data(&code.insns) else {
                    return 0;
                };
                first_line(lines, &raw)
            }
            ClassOrigin::Java(j) => {
                let class = &self.java_classes[j];
                let Ok(Some(debug)) = class.debug_info(code_offset) else {
                    return 0;
                };
                let Ok(raw) = class.decode_insns(code_offset) else {
                    return 0;
                };
                first_line(debug.lines, &raw)
            }
            ClassOrigin::Synthetic => 0,
        }
    }
}

/// [`CodeRefProvider`] over dex files and JVM classes; `origin[i]` locates class `i` of the
/// class tree.
pub struct CombinedCodeRefs<'a> {
    pub dexes: &'a [DexFile],
    pub java_classes: &'a [JavaInputClass],
    pub origin: &'a [ClassOrigin],
}

impl CodeRefProvider for CombinedCodeRefs<'_> {
    fn code_refs(&self, class_idx: usize, code_offset: u32) -> Result<Vec<CodeRef>, String> {
        match self.origin.get(class_idx) {
            Some(ClassOrigin::Dex(d)) => self
                .dexes
                .get(*d)
                .ok_or_else(|| format!("no dex file for class #{class_idx}"))?
                .code_refs(code_offset)
                .map_err(|e| e.to_string()),
            Some(ClassOrigin::Java(j)) => self
                .java_classes
                .get(*j)
                .ok_or_else(|| format!("no class file for class #{class_idx}"))?
                .code_refs(code_offset)
                .map_err(|e| e.to_string()),
            _ => Err(format!("no code for class #{class_idx}")),
        }
    }
}
