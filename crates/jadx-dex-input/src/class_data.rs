//! `class_data_item`, `code_item` (instructions, tries, catch handlers).

use crate::error::{DexError, Result};
use crate::file::{ClassDef, DexFile};
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedField {
    pub field_idx: u32,
    pub access_flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedMethod {
    pub method_idx: u32,
    pub access_flags: u32,
    /// Offset of the `code_item`, 0 for abstract/native methods.
    pub code_off: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ClassData {
    pub static_fields: Vec<EncodedField>,
    pub instance_fields: Vec<EncodedField>,
    pub direct_methods: Vec<EncodedMethod>,
    pub virtual_methods: Vec<EncodedMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchHandler {
    /// `(type_idx, handler address)` pairs.
    pub handlers: Vec<(u32, u32)>,
    pub catch_all: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TryItem {
    pub start_addr: u32,
    pub insn_count: u16,
    /// Index into [`CodeItem::handlers`].
    pub handler_idx: usize,
}

#[derive(Debug, Clone)]
pub struct CodeItem {
    pub registers_size: u16,
    pub ins_size: u16,
    pub outs_size: u16,
    pub debug_info_off: u32,
    pub insns: Vec<u16>,
    pub tries: Vec<TryItem>,
    pub handlers: Vec<CatchHandler>,
}

impl DexFile {
    /// Parse the class data of `def`; `None` if the class has no members.
    pub fn class_data(&self, def: &ClassDef) -> Result<Option<ClassData>> {
        if def.class_data_off == 0 {
            return Ok(None);
        }
        let mut r = Reader::new(self.bytes(), def.class_data_off as usize);
        let static_n = r.uleb128()?;
        let instance_n = r.uleb128()?;
        let direct_n = r.uleb128()?;
        let virtual_n = r.uleb128()?;
        let mut cd = ClassData::default();
        read_fields(&mut r, static_n, &mut cd.static_fields)?;
        read_fields(&mut r, instance_n, &mut cd.instance_fields)?;
        read_methods(&mut r, direct_n, &mut cd.direct_methods)?;
        read_methods(&mut r, virtual_n, &mut cd.virtual_methods)?;
        Ok(Some(cd))
    }

    /// Parse the `code_item` at `off`.
    pub fn code_item(&self, off: u32) -> Result<CodeItem> {
        let data = self.bytes();
        let mut r = Reader::new(data, off as usize);
        let registers_size = r.u16()?;
        let ins_size = r.u16()?;
        let outs_size = r.u16()?;
        let tries_size = r.u16()? as usize;
        let debug_info_off = r.u32()?;
        let insns_size = r.u32()? as usize;
        if insns_size.saturating_mul(2) > data.len() {
            return Err(DexError::Truncated {
                offset: r.pos(),
                wanted: insns_size * 2,
                size: data.len(),
            });
        }
        let raw = r.bytes(insns_size * 2)?;
        let insns: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut code = CodeItem {
            registers_size,
            ins_size,
            outs_size,
            debug_info_off,
            insns,
            tries: Vec::new(),
            handlers: Vec::new(),
        };
        if tries_size == 0 {
            return Ok(code);
        }
        if insns_size % 2 == 1 {
            r.skip(2)?; // padding
        }
        let mut raw_tries = Vec::with_capacity(tries_size.min(4096));
        for _ in 0..tries_size {
            let start = r.u32()?;
            let count = r.u16()?;
            let handler_off = r.u16()? as usize;
            raw_tries.push((start, count, handler_off));
        }
        // encoded_catch_handler_list
        let list_start = r.pos();
        let size = r.uleb128()? as usize;
        let mut byte_offsets = Vec::with_capacity(size.min(4096));
        for _ in 0..size {
            byte_offsets.push(r.pos() - list_start);
            let n = r.sleb128()?;
            let mut handlers = Vec::new();
            for _ in 0..n.unsigned_abs() {
                let type_idx = r.uleb128()?;
                let addr = r.uleb128()?;
                handlers.push((type_idx, addr));
            }
            let catch_all = if n <= 0 { Some(r.uleb128()?) } else { None };
            code.handlers.push(CatchHandler {
                handlers,
                catch_all,
            });
        }
        for (start_addr, insn_count, handler_off) in raw_tries {
            let handler_idx = byte_offsets
                .iter()
                .position(|o| *o == handler_off)
                .ok_or_else(|| {
                    DexError::BadContainer(format!(
                        "catch handler not found by byte offset {handler_off}"
                    ))
                })?;
            code.tries.push(TryItem {
                start_addr,
                insn_count,
                handler_idx,
            });
        }
        Ok(code)
    }
}

fn read_fields(r: &mut Reader<'_>, n: u32, out: &mut Vec<EncodedField>) -> Result<()> {
    let mut idx = 0u32;
    for _ in 0..n {
        idx = idx.wrapping_add(r.uleb128()?);
        out.push(EncodedField {
            field_idx: idx,
            access_flags: r.uleb128()?,
        });
    }
    Ok(())
}

fn read_methods(r: &mut Reader<'_>, n: u32, out: &mut Vec<EncodedMethod>) -> Result<()> {
    let mut idx = 0u32;
    for _ in 0..n {
        idx = idx.wrapping_add(r.uleb128()?);
        let access_flags = r.uleb128()?;
        let code_off = r.uleb128()?;
        out.push(EncodedMethod {
            method_idx: idx,
            access_flags,
            code_off,
        });
    }
    Ok(())
}

impl DexFile {
    /// Try blocks of a code item in the input API form.
    pub fn tries_api(&self, code: &CodeItem) -> Result<Vec<jadx_input_api::TryData>> {
        use jadx_input_api::{CatchData, TryData};
        let mut out = Vec::with_capacity(code.tries.len());
        for t in &code.tries {
            let h = &code.handlers[t.handler_idx];
            let mut handlers = Vec::new();
            let mut types = Vec::new();
            for (type_idx, addr) in &h.handlers {
                types.push(self.type_descriptor(*type_idx)?);
                handlers.push(*addr);
            }
            out.push(TryData {
                start: t.start_addr,
                end: t
                    .start_addr
                    .wrapping_add(t.insn_count as u32)
                    .wrapping_sub(1),
                catch: CatchData {
                    handlers,
                    types,
                    catch_all: h.catch_all.map(i64::from).unwrap_or(-1),
                },
            });
        }
        Ok(out)
    }
}
