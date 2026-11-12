//! DEX header and map list.

use crate::error::{DexError, Result};
use crate::reader::Reader;

pub const DEX_MAGIC: [u8; 4] = *b"dex\n";
pub const ZIP_MAGIC: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];
pub const ENDIAN_CONSTANT: u32 = 0x1234_5678;
/// Size of the standard (pre-container) header.
pub const STD_HEADER_SIZE: u32 = 0x70;
/// Smallest header size that carries container fields (DEX v41+).
pub const CONTAINER_HEADER_SIZE: u32 = 120;
/// `NO_INDEX` as stored in a u32 field.
pub const NO_INDEX: u32 = 0xFFFF_FFFF;

pub const TYPE_CALL_SITE_ID: u16 = 0x0007;
pub const TYPE_METHOD_HANDLE: u16 = 0x0008;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapItem {
    pub kind: u16,
    pub size: u32,
    pub offset: u32,
}

impl MapItem {
    /// Human readable name of the map item type (matches the DEX format specification).
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            0x0000 => "header_item",
            0x0001 => "string_id_item",
            0x0002 => "type_id_item",
            0x0003 => "proto_id_item",
            0x0004 => "field_id_item",
            0x0005 => "method_id_item",
            0x0006 => "class_def_item",
            0x0007 => "call_site_id_item",
            0x0008 => "method_handle_item",
            0x1000 => "map_list",
            0x1001 => "type_list",
            0x1002 => "annotation_set_ref_list",
            0x1003 => "annotation_set_item",
            0x2000 => "class_data_item",
            0x2001 => "code_item",
            0x2002 => "string_data_item",
            0x2003 => "debug_info_item",
            0x2004 => "annotation_item",
            0x2005 => "encoded_array_item",
            0x2006 => "annotations_directory_item",
            0xF000 => "hiddenapi_class_data_item",
            _ => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DexHeader {
    /// Three ASCII digits, e.g. `"035"`, `"039"`, `"041"`.
    pub version: String,
    pub checksum: u32,
    pub signature: [u8; 20],
    pub file_size: u32,
    pub header_size: u32,
    pub endian_tag: u32,
    pub link_size: u32,
    pub link_off: u32,
    pub map_off: u32,
    pub string_ids_size: u32,
    pub string_ids_off: u32,
    pub type_ids_size: u32,
    pub type_ids_off: u32,
    pub proto_ids_size: u32,
    pub proto_ids_off: u32,
    pub field_ids_size: u32,
    pub field_ids_off: u32,
    pub method_ids_size: u32,
    pub method_ids_off: u32,
    pub class_defs_size: u32,
    pub class_defs_off: u32,
    pub data_size: u32,
    pub data_off: u32,
    /// DEX v41 container fields, present when `header_size >= 120`.
    pub container_size: Option<u32>,
    pub header_offset: Option<u32>,
    pub call_site_ids_off: u32,
    pub call_site_ids_size: u32,
    pub method_handles_off: u32,
    pub method_handles_size: u32,
    pub map: Vec<MapItem>,
}

impl DexHeader {
    /// Parse the header located at `base` inside `data`. All offsets stored in the header are
    /// absolute within `data` (for v41 containers they are container relative).
    pub fn parse(data: &[u8], base: usize) -> Result<Self> {
        let mut r = Reader::new(data, base);
        let magic: [u8; 4] = r.bytes(4)?.try_into().expect("4 bytes");
        if magic != DEX_MAGIC {
            return Err(DexError::BadMagic(magic));
        }
        let ver = r.bytes(3)?;
        let nul = r.u8()?;
        if !ver.iter().all(u8::is_ascii_digit) || nul != 0 {
            return Err(DexError::BadVersion(
                String::from_utf8_lossy(ver).into_owned(),
            ));
        }
        let version = String::from_utf8_lossy(ver).into_owned();
        let checksum = r.u32()?;
        let signature: [u8; 20] = r.bytes(20)?.try_into().expect("20 bytes");
        let file_size = r.u32()?;
        let header_size = r.u32()?;
        let endian_tag = r.u32()?;
        if endian_tag != ENDIAN_CONSTANT {
            return Err(DexError::BadEndianTag(endian_tag));
        }
        let link_size = r.u32()?;
        let link_off = r.u32()?;
        let map_off = r.u32()?;
        let string_ids_size = r.u32()?;
        let string_ids_off = r.u32()?;
        let type_ids_size = r.u32()?;
        let type_ids_off = r.u32()?;
        let proto_ids_size = r.u32()?;
        let proto_ids_off = r.u32()?;
        let field_ids_size = r.u32()?;
        let field_ids_off = r.u32()?;
        let method_ids_size = r.u32()?;
        let method_ids_off = r.u32()?;
        let class_defs_size = r.u32()?;
        let class_defs_off = r.u32()?;
        let data_size = r.u32()?;
        let data_off = r.u32()?;

        let (container_size, header_offset) = if header_size >= CONTAINER_HEADER_SIZE {
            let mut c = Reader::new(data, base + 112);
            (Some(c.u32()?), Some(c.u32()?))
        } else {
            (None, None)
        };

        let mut h = DexHeader {
            version,
            checksum,
            signature,
            file_size,
            header_size,
            endian_tag,
            link_size,
            link_off,
            map_off,
            string_ids_size,
            string_ids_off,
            type_ids_size,
            type_ids_off,
            proto_ids_size,
            proto_ids_off,
            field_ids_size,
            field_ids_off,
            method_ids_size,
            method_ids_off,
            class_defs_size,
            class_defs_off,
            data_size,
            data_off,
            container_size,
            header_offset,
            call_site_ids_off: 0,
            call_site_ids_size: 0,
            method_handles_off: 0,
            method_handles_size: 0,
            map: Vec::new(),
        };
        h.read_map_list(data)?;
        h.validate_tables(data.len())?;
        Ok(h)
    }

    fn read_map_list(&mut self, data: &[u8]) -> Result<()> {
        if self.map_off == 0 {
            return Ok(());
        }
        let mut r = Reader::new(data, self.map_off as usize);
        let size = r.u32()?;
        // Each entry is 12 bytes; reject sizes that cannot possibly fit.
        if (size as usize).saturating_mul(12) > data.len() {
            return Err(DexError::TableOutOfBounds {
                what: "map_list",
                off: self.map_off,
                size,
                file_size: data.len(),
            });
        }
        for _ in 0..size {
            let kind = r.u16()?;
            r.skip(2)?; // unused
            let item_size = r.u32()?;
            let offset = r.u32()?;
            match kind {
                TYPE_CALL_SITE_ID => {
                    self.call_site_ids_off = offset;
                    self.call_site_ids_size = item_size;
                }
                TYPE_METHOD_HANDLE => {
                    self.method_handles_off = offset;
                    self.method_handles_size = item_size;
                }
                _ => {}
            }
            self.map.push(MapItem {
                kind,
                size: item_size,
                offset,
            });
        }
        Ok(())
    }

    /// Every fixed-size id table must lie inside the file.
    fn validate_tables(&self, file_size: usize) -> Result<()> {
        let tables: [(&'static str, u32, u32, usize); 6] = [
            ("string_ids", self.string_ids_off, self.string_ids_size, 4),
            ("type_ids", self.type_ids_off, self.type_ids_size, 4),
            ("proto_ids", self.proto_ids_off, self.proto_ids_size, 12),
            ("field_ids", self.field_ids_off, self.field_ids_size, 8),
            ("method_ids", self.method_ids_off, self.method_ids_size, 8),
            ("class_defs", self.class_defs_off, self.class_defs_size, 32),
        ];
        for (what, off, size, entry) in tables {
            if size == 0 {
                continue;
            }
            let end = (off as usize).saturating_add((size as usize).saturating_mul(entry));
            if end > file_size {
                return Err(DexError::TableOutOfBounds {
                    what,
                    off,
                    size,
                    file_size,
                });
            }
        }
        Ok(())
    }
}
