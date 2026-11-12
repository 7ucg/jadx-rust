//! Conversion of raw Dalvik instructions to the normalised [`InsnData`].

use jadx_input_api::{
    ArrayData, ArrayPayload as ApiArrayPayload, InsnData, InsnIndexType, Opcode,
    Payload as ApiPayload, SwitchPayload,
};

use super::decode::{decode_at, Insn, Payload};
use super::jadx_map_gen::JADX_MAP;
use super::{lookup, Format};
use crate::error::Result;
use crate::file::DexFile;

fn map_entry(value: u16) -> Option<(Opcode, InsnIndexType)> {
    static MAP: std::sync::OnceLock<Vec<Option<(Opcode, InsnIndexType)>>> =
        std::sync::OnceLock::new();
    let map = MAP.get_or_init(|| {
        let mut v = vec![None; 0x400];
        for (val, op, idx) in JADX_MAP {
            v[*val as usize] = Some((*op, *idx));
        }
        v
    });
    map.get(value as usize).copied().flatten()
}

fn convert(insn: &Insn) -> InsnData {
    let (opcode, index_type) =
        map_entry(insn.info.value).unwrap_or((Opcode::Unknown, InsnIndexType::None));
    let offset = insn.offset as i64;
    let mut regs: Vec<u32> = Vec::new();
    let mut literal = insn.literal;
    let mut target: i64 = 0;
    let mut index = insn.index;
    use Format::*;
    match insn.info.format {
        Format3rc | Format4rcc => {
            let start = insn.range_start();
            regs.extend((0..insn.literal as u32).map(|i| start + i));
            literal = 0;
        }
        _ => regs.extend_from_slice(insn.regs()),
    }
    match insn.info.format {
        Format10t | Format20t | Format21t | Format22t | Format30t | Format31t => {
            target = offset + insn.target as i64;
        }
        Format45cc | Format4rcc => target = insn.index2 as i64,
        _ => {}
    }
    if !matches!(
        insn.info.format,
        Format21c | Format22c | Format31c | Format35c | Format3rc | Format45cc | Format4rcc
    ) {
        index = 0;
    }
    let payload = insn.payload.as_ref().map(|p| match p {
        Payload::PackedSwitch { first_key, targets } => ApiPayload::Switch(SwitchPayload {
            keys: (0..targets.len() as i32)
                .map(|i| first_key.wrapping_add(i))
                .collect(),
            targets: targets.clone(),
        }),
        Payload::SparseSwitch { keys, targets } => ApiPayload::Switch(SwitchPayload {
            keys: keys.clone(),
            targets: targets.clone(),
        }),
        Payload::ArrayData {
            element_width,
            data,
        } => {
            let w = *element_width as usize;
            let size = data.len().checked_div(w).unwrap_or(0) as u32;
            let data = match w {
                1 => ArrayData::Bytes(data.clone()),
                2 => ArrayData::Shorts(
                    data.chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]))
                        .collect(),
                ),
                4 => ArrayData::Ints(
                    data.chunks_exact(4)
                        .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect(),
                ),
                8 => ArrayData::Longs(
                    data.chunks_exact(8)
                        .map(|c| {
                            i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])
                        })
                        .collect(),
                ),
                _ => ArrayData::Empty,
            };
            ApiPayload::Array(ApiArrayPayload {
                size,
                element_size: *element_width,
                data,
            })
        }
    });
    InsnData {
        offset: insn.offset,
        length: insn.len,
        opcode,
        raw_opcode_unit: 0,
        index_type,
        regs,
        literal,
        target,
        index,
        result_reg: None,
        payload,
    }
}

/// Decode all instructions of a code item into normalised form. Unknown opcodes become
/// `Opcode::Unknown` instructions of length 1, like the reference reader.
pub fn decode_insn_data(units: &[u16]) -> Result<Vec<InsnData>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < units.len() {
        let unit = units[pos];
        let mut data = if lookup(unit).is_none() {
            InsnData {
                offset: pos as u32,
                length: 1,
                opcode: Opcode::Unknown,
                raw_opcode_unit: unit,
                index_type: InsnIndexType::None,
                regs: Vec::new(),
                literal: 0,
                target: 0,
                index: 0,
                result_reg: None,
                payload: None,
            }
        } else {
            let insn = decode_at(units, pos)?;
            let mut d = convert(&insn);
            d.raw_opcode_unit = unit;
            d
        };
        data.offset = pos as u32;
        pos += data.length as usize;
        out.push(data);
    }
    Ok(out)
}

impl jadx_input_api::InsnResolver for crate::file::DexFile {
    fn string(&self, idx: u32) -> std::result::Result<String, String> {
        DexFile::string(self, idx).map_err(|e| e.to_string())
    }
    fn string_utf16(&self, idx: u32) -> std::result::Result<Vec<u16>, String> {
        DexFile::string_utf16(self, idx).map_err(|e| e.to_string())
    }
    fn type_desc(&self, idx: u32) -> std::result::Result<String, String> {
        self.type_descriptor(idx).map_err(|e| e.to_string())
    }
    fn field(&self, idx: u32) -> std::result::Result<jadx_input_api::FieldRefData, String> {
        let f = self.field_ref(idx).map_err(|e| e.to_string())?;
        Ok(jadx_input_api::FieldRefData {
            parent_class: f.class,
            name: f.name,
            ty: f.ty,
        })
    }
    fn method(&self, idx: u32) -> std::result::Result<jadx_input_api::MethodRefData, String> {
        let m = self.method_ref(idx).map_err(|e| e.to_string())?;
        Ok(jadx_input_api::MethodRefData {
            parent_class: m.class,
            name: m.name,
            arg_types: m.arg_types,
            return_type: m.return_type,
        })
    }
    fn proto(&self, idx: u32) -> std::result::Result<jadx_input_api::MethodProtoData, String> {
        let p = self.proto_id(idx).map_err(|e| e.to_string())?;
        Ok(jadx_input_api::MethodProtoData {
            arg_types: self
                .type_list(p.parameters_off)
                .map_err(|e| e.to_string())?,
            return_type: self
                .type_descriptor(p.return_type_idx)
                .map_err(|e| e.to_string())?,
        })
    }
}
