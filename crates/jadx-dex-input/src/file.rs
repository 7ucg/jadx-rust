//! A parsed DEX file: header plus on-demand access to the id tables.

use std::sync::Arc;

use crate::error::{DexError, Result};
use crate::header::{DexHeader, NO_INDEX};
use crate::mutf8;
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtoId {
    pub shorty_idx: u32,
    pub return_type_idx: u32,
    pub parameters_off: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldId {
    pub class_idx: u16,
    pub type_idx: u16,
    pub name_idx: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodId {
    pub class_idx: u16,
    pub proto_idx: u16,
    pub name_idx: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassDef {
    pub class_idx: u32,
    pub access_flags: u32,
    pub superclass_idx: u32,
    pub interfaces_off: u32,
    pub source_file_idx: u32,
    pub annotations_off: u32,
    pub class_data_off: u32,
    pub static_values_off: u32,
}

/// Resolved method reference (`Lcls;->name(args)ret`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodRef {
    pub class: String,
    pub name: String,
    pub arg_types: Vec<String>,
    pub return_type: String,
}

/// Resolved field reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRef {
    pub class: String,
    pub name: String,
    pub ty: String,
}

/// One DEX file (or one sub-file of a v41 container). Cheap to clone: the bytes are shared.
#[derive(Clone)]
pub struct DexFile {
    name: String,
    data: Arc<[u8]>,
    base: usize,
    header: DexHeader,
}

impl std::fmt::Debug for DexFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DexFile")
            .field("name", &self.name)
            .field("base", &self.base)
            .field("version", &self.header.version)
            .finish()
    }
}

impl DexFile {
    /// Parse the DEX header found at `base` inside `data`.
    pub fn parse(name: impl Into<String>, data: Arc<[u8]>, base: usize) -> Result<Self> {
        let header = DexHeader::parse(&data, base)?;
        Ok(DexFile {
            name: name.into(),
            data,
            base,
            header,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn header(&self) -> &DexHeader {
        &self.header
    }

    /// Offset of this dex inside its container buffer (0 for plain dex files).
    pub fn base_offset(&self) -> usize {
        self.base
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn string_count(&self) -> u32 {
        self.header.string_ids_size
    }
    pub fn type_count(&self) -> u32 {
        self.header.type_ids_size
    }
    pub fn proto_count(&self) -> u32 {
        self.header.proto_ids_size
    }
    pub fn field_count(&self) -> u32 {
        self.header.field_ids_size
    }
    pub fn method_count(&self) -> u32 {
        self.header.method_ids_size
    }
    pub fn class_count(&self) -> u32 {
        self.header.class_defs_size
    }

    fn check(&self, what: &'static str, idx: u32, count: u32) -> Result<()> {
        if idx >= count {
            return Err(DexError::IndexOutOfRange { what, idx, count });
        }
        Ok(())
    }

    fn reader_at(&self, off: usize) -> Reader<'_> {
        Reader::new(&self.data, off)
    }

    /// String as UTF-16 code units (exact, including unpaired surrogates).
    pub fn string_utf16(&self, idx: u32) -> Result<Vec<u16>> {
        self.check("string", idx, self.header.string_ids_size)?;
        let mut r = self.reader_at(self.header.string_ids_off as usize + idx as usize * 4);
        let off = r.u32()? as usize;
        r.set_pos(off);
        mutf8::decode_utf16(&mut r)
    }

    /// String as a Rust string (unpaired surrogates become U+FFFD).
    pub fn string(&self, idx: u32) -> Result<String> {
        self.string_utf16(idx).map(|u| mutf8::utf16_to_string(&u))
    }

    pub fn opt_string(&self, idx: u32) -> Result<Option<String>> {
        if idx == NO_INDEX {
            return Ok(None);
        }
        self.string(idx).map(Some)
    }

    /// Type descriptor, e.g. `Ljava/lang/String;` or `[I`.
    pub fn type_descriptor(&self, idx: u32) -> Result<String> {
        self.check("type", idx, self.header.type_ids_size)?;
        let mut r = self.reader_at(self.header.type_ids_off as usize + idx as usize * 4);
        let str_idx = r.u32()?;
        self.string(str_idx)
    }

    pub fn opt_type_descriptor(&self, idx: u32) -> Result<Option<String>> {
        if idx == NO_INDEX {
            return Ok(None);
        }
        self.type_descriptor(idx).map(Some)
    }

    pub fn proto_id(&self, idx: u32) -> Result<ProtoId> {
        self.check("proto", idx, self.header.proto_ids_size)?;
        let mut r = self.reader_at(self.header.proto_ids_off as usize + idx as usize * 12);
        Ok(ProtoId {
            shorty_idx: r.u32()?,
            return_type_idx: r.u32()?,
            parameters_off: r.u32()?,
        })
    }

    pub fn field_id(&self, idx: u32) -> Result<FieldId> {
        self.check("field", idx, self.header.field_ids_size)?;
        let mut r = self.reader_at(self.header.field_ids_off as usize + idx as usize * 8);
        Ok(FieldId {
            class_idx: r.u16()?,
            type_idx: r.u16()?,
            name_idx: r.u32()?,
        })
    }

    pub fn method_id(&self, idx: u32) -> Result<MethodId> {
        self.check("method", idx, self.header.method_ids_size)?;
        let mut r = self.reader_at(self.header.method_ids_off as usize + idx as usize * 8);
        Ok(MethodId {
            class_idx: r.u16()?,
            proto_idx: r.u16()?,
            name_idx: r.u32()?,
        })
    }

    pub fn class_def(&self, idx: u32) -> Result<ClassDef> {
        self.check("class_def", idx, self.header.class_defs_size)?;
        let mut r = self.reader_at(self.header.class_defs_off as usize + idx as usize * 32);
        Ok(ClassDef {
            class_idx: r.u32()?,
            access_flags: r.u32()?,
            superclass_idx: r.u32()?,
            interfaces_off: r.u32()?,
            source_file_idx: r.u32()?,
            annotations_off: r.u32()?,
            class_data_off: r.u32()?,
            static_values_off: r.u32()?,
        })
    }

    /// Iterate all class definitions in file order.
    pub fn class_defs(&self) -> impl Iterator<Item = Result<ClassDef>> + '_ {
        (0..self.header.class_defs_size).map(move |i| self.class_def(i))
    }

    /// Read a `type_list` at `off` (0 means empty) and resolve each entry to its descriptor.
    pub fn type_list(&self, off: u32) -> Result<Vec<String>> {
        if off == 0 {
            return Ok(Vec::new());
        }
        let mut r = self.reader_at(off as usize);
        let size = r.u32()?;
        let mut out = Vec::with_capacity((size as usize).min(1024));
        for _ in 0..size {
            out.push(self.type_descriptor(r.u16()? as u32)?);
        }
        Ok(out)
    }

    pub fn method_ref(&self, idx: u32) -> Result<MethodRef> {
        let m = self.method_id(idx)?;
        let p = self.proto_id(m.proto_idx as u32)?;
        Ok(MethodRef {
            class: self.type_descriptor(m.class_idx as u32)?,
            name: self.string(m.name_idx)?,
            arg_types: self.type_list(p.parameters_off)?,
            return_type: self.type_descriptor(p.return_type_idx)?,
        })
    }

    pub fn field_ref(&self, idx: u32) -> Result<FieldRef> {
        let f = self.field_id(idx)?;
        Ok(FieldRef {
            class: self.type_descriptor(f.class_idx as u32)?,
            name: self.string(f.name_idx)?,
            ty: self.type_descriptor(f.type_idx as u32)?,
        })
    }

    /// `(handle type, field/method index)` of a method handle item.
    pub fn method_handle(&self, idx: u32) -> Result<(u16, u32)> {
        self.check("method_handle", idx, self.header.method_handles_size)?;
        let mut r = self.reader_at(self.header.method_handles_off as usize + idx as usize * 8);
        let kind = r.u16()?;
        r.skip(2)?;
        let ref_idx = r.u16()? as u32;
        Ok((kind, ref_idx))
    }

    /// Verify the Adler-32 checksum stored in the header (covers bytes `12..file_size`).
    pub fn verify_checksum(&self) -> Result<()> {
        verify_checksum(&self.name, &self.data, self.base)
    }
}

/// Verify the checksum of the dex starting at `base`.
pub fn verify_checksum(name: &str, data: &[u8], base: usize) -> Result<()> {
    if base.saturating_add(36) > data.len() {
        return Err(DexError::FileTruncated(format!(
            "can't read file length, file: {name}"
        )));
    }
    let mut r = Reader::new(data, base + 8);
    let expected = r.u32()?;
    r.set_pos(base + 32);
    let len = r.u32()? as usize;
    if len < 12 || base.saturating_add(len) > data.len() {
        return Err(DexError::FileTruncated(format!(
            "length in header: {len}, file: {name}"
        )));
    }
    let actual = adler2::adler32_slice(&data[base + 12..base + len]);
    if actual != expected {
        return Err(DexError::BadChecksum {
            actual,
            expected,
            file: name.to_string(),
        });
    }
    Ok(())
}
