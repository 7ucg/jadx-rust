//! `resources.arsc` parsing: resource entries, name assignment and the resource id to name map.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::android_map::{AttrDef, AttrKind};
use crate::config::{EntryConfig, RawConfig};
use crate::consts::*;
use crate::error::{ResError, Result};
use crate::names::{
    better_name, is_valid_entry_name, is_valid_full_identifier, sanitize_as_resource_name,
    to_r_field_name, ResourceNameSource,
};
use crate::stream::ParserStream;
use crate::strings::{parse_string_pool_no_size, parse_string_pool_no_type, BinaryXmlStrings};
use crate::values::{RawNamedValue, RawValue, ValuesParser};

/// Naming options of the resource table decoder.
#[derive(Debug, Clone)]
pub struct ResOptions {
    pub name_source: ResourceNameSource,
    /// Rename entries whose name is not a valid resource/field name.
    pub rename_valid: bool,
    /// Treat non printable characters as invalid in names.
    pub rename_printable: bool,
    /// Names of the constant fields the app code declares for resource ids (the fields of the
    /// app `R` classes), by resource id. Synthetic classes must not be included.
    pub code_names: HashMap<i32, String>,
    /// Keep the names of the table untouched (no renaming, no collision handling).
    pub use_raw_names: bool,
}

impl Default for ResOptions {
    fn default() -> Self {
        ResOptions {
            name_source: ResourceNameSource::Auto,
            rename_valid: true,
            rename_printable: true,
            code_names: HashMap::new(),
            use_raw_names: false,
        }
    }
}

/// Resource id constant of the app `R` class: `R.<type>.<name> = id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RField {
    pub id: i32,
    pub type_name: String,
    /// Field name (`.` replaced by `_`).
    pub name: String,
}

/// One resource of one configuration.
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub id: i32,
    pub pkg_name: String,
    pub type_name: String,
    pub key_name: String,
    pub config: String,
    pub parent_ref: i32,
    pub simple_value: Option<RawValue>,
    pub named_values: Option<Vec<RawNamedValue>>,
}

impl ResourceEntry {
    fn new(id: i32, pkg: &str, type_name: &str, key_name: &str, config: &str) -> Self {
        ResourceEntry {
            id,
            pkg_name: pkg.to_string(),
            type_name: type_name.to_string(),
            key_name: key_name.to_string(),
            config: config.to_string(),
            parent_ref: 0,
            simple_value: None,
            named_values: None,
        }
    }

    fn copy_with_id(&self, res_name: &str) -> Self {
        let mut c = self.clone();
        c.key_name = format!("{res_name}_res_0x{:08x}", self.id as u32);
        c
    }
}

type NameKey = (String, String, String);

/// All entries of a table in id order plus the helper maps of the name assignment.
#[derive(Debug, Default)]
pub struct ResourceStorage {
    arena: Vec<ResourceEntry>,
    list: Vec<usize>,
    uniq_names: HashMap<NameKey, usize>,
    renames: HashMap<i32, String>,
    app_package: Option<String>,
}

impl ResourceStorage {
    fn add(&mut self, e: ResourceEntry) -> usize {
        let key = (e.config.clone(), e.type_name.clone(), e.key_name.clone());
        let idx = self.arena.len();
        self.arena.push(e);
        self.list.push(idx);
        self.uniq_names.insert(key, idx);
        idx
    }

    fn replace(&mut self, prev: usize, new: ResourceEntry) {
        let idx = self.arena.len();
        self.arena.push(new);
        if let Some(p) = self.list.iter().position(|&i| i == prev) {
            self.list[p] = idx;
        }
        // the old name stays occupied
    }

    fn search_same_name(&self, e: &ResourceEntry) -> Option<usize> {
        let key = (e.config.clone(), e.type_name.clone(), e.key_name.clone());
        self.uniq_names.get(&key).copied()
    }

    fn finish(&mut self) {
        let arena = &self.arena;
        self.list.sort_by_key(|&i| arena[i].id);
        self.uniq_names.clear();
        self.renames.clear();
    }

    /// Entries ordered by resource id (the order of the table within one id).
    pub fn resources(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.list.iter().map(move |&i| &self.arena[i])
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Package name of the table.
    pub fn app_package(&self) -> Option<&str> {
        self.app_package.as_deref()
    }

    /// Resource id to `type/name` map.
    pub fn resource_names(&self) -> HashMap<i32, String> {
        let mut map = HashMap::new();
        for e in self.resources() {
            map.insert(e.id, format!("{}/{}", e.type_name, e.key_name));
        }
        map
    }
}

struct PackageChunk {
    id: i32,
    name: String,
    type_strings: Option<BinaryXmlStrings>,
    key_strings: Option<BinaryXmlStrings>,
}

struct EntryOffset {
    idx: i32,
    offset: i32,
}

/// A parsed resource table.
#[derive(Debug)]
pub struct ResTable {
    storage: ResourceStorage,
    strings: BinaryXmlStrings,
    field_renames: BTreeMap<i32, String>,
}

impl ResTable {
    /// Parses the bytes of a `resources.arsc` file.
    pub fn parse(data: &[u8], opts: &ResOptions) -> Result<ResTable> {
        let mut p = TableParser {
            is: ParserStream::new(data),
            opts,
            storage: ResourceStorage::default(),
            strings: BinaryXmlStrings::empty(),
            field_renames: BTreeMap::new(),
        };
        p.decode_table_chunk()?;
        p.storage.finish();
        Ok(ResTable {
            storage: p.storage,
            strings: p.strings,
            field_renames: p.field_renames,
        })
    }

    pub fn storage(&self) -> &ResourceStorage {
        &self.storage
    }

    /// The global value string pool.
    pub fn strings(&self) -> &BinaryXmlStrings {
        &self.strings
    }

    /// Resource id to `type/name` map (the names used in decoded XML references and for the
    /// resource id constant fields of the app `R` class).
    pub fn resource_names(&self) -> HashMap<i32, String> {
        self.storage.resource_names()
    }

    /// New names for the code fields that carry a resource id (id, R class field name), for the
    /// ids in `ResOptions::code_names` whose name was changed.
    pub fn field_renames(&self) -> &BTreeMap<i32, String> {
        &self.field_renames
    }

    /// The resource id constants for the app `R` class, one per type and field name (the first
    /// id wins), ordered by id.
    pub fn r_fields(&self) -> Vec<RField> {
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut out = Vec::new();
        for e in self.storage.resources() {
            let name = to_r_field_name(&e.key_name);
            if seen.insert((e.type_name.clone(), name.clone())) {
                out.push(RField {
                    id: e.id,
                    type_name: e.type_name.clone(),
                    name,
                });
            }
        }
        out
    }

    /// Enum and flag attributes declared by the app, keyed by attribute name.
    pub fn app_attrs(&self) -> HashMap<String, AttrDef> {
        let names = self.resource_names();
        let vp = ValuesParser::new(&self.strings, &names);
        let mut map = HashMap::new();
        for ri in self.storage.resources() {
            if ri.type_name != "attr" {
                continue;
            }
            let Some(named) = ri.named_values.as_ref().filter(|n| n.len() > 1) else {
                continue;
            };
            let type_val = named[0].value.data & 0xff_0000;
            let kind = if type_val == ATTR_TYPE_FLAGS {
                AttrKind::Flag
            } else if type_val == ATTR_TYPE_ENUM {
                AttrKind::Enum
            } else {
                continue;
            };
            let mut def = AttrDef {
                kind,
                values: Default::default(),
            };
            for rv in &named[1..] {
                let value = vp.decode_name_ref(rv.name_ref).unwrap_or_default();
                let value = value.strip_prefix("id.").unwrap_or(&value).to_string();
                def.values.insert(i64::from(rv.value.data), value);
            }
            map.insert(ri.key_name.clone(), def);
        }
        map
    }
}

struct TableParser<'a, 'o> {
    is: ParserStream<'a>,
    opts: &'o ResOptions,
    storage: ResourceStorage,
    strings: BinaryXmlStrings,
    field_renames: BTreeMap<i32, String>,
}

impl TableParser<'_, '_> {
    fn decode_table_chunk(&mut self) -> Result<()> {
        self.is.check_i16(RES_TABLE_TYPE, "Not a table chunk")?;
        self.is.check_i16(0x000c, "Unexpected table header size")?;
        let size = i64::from(self.is.read_i32());
        let _pkg_count = self.is.read_i32();
        while (self.is.pos() as i64) < size {
            let chunk_start = self.is.pos();
            let ty = self.is.read_i16();
            let header_size = self.is.read_i16();
            let chunk_size = u64::from(self.is.read_u32());
            let chunk_end = chunk_start + chunk_size;
            match ty {
                RES_NULL_TYPE => {}
                RES_STRING_POOL_TYPE => {
                    self.strings = parse_string_pool_no_size(&mut self.is, chunk_start, chunk_end)?;
                }
                RES_TABLE_PACKAGE_TYPE => {
                    self.parse_package(chunk_start, header_size, chunk_end)?;
                }
                _ => {}
            }
            self.is.skip_to_pos(chunk_end, "Skip to table chunk end")?;
        }
        Ok(())
    }

    fn parse_package(&mut self, pkg_start: u64, header_size: i32, pkg_end: u64) -> Result<()> {
        if header_size < 0x011c {
            return Err(self.die("Package header size too small"));
        }
        let id = self.is.read_i32();
        let name = self.is.read_string16_fixed(128)?;
        let type_strings_offset = pkg_start as i64 + i64::from(self.is.read_i32());
        let _last_public_type = self.is.read_i32();
        let key_strings_offset = pkg_start as i64 + i64::from(self.is.read_i32());
        let _last_public_key = self.is.read_i32();
        if header_size >= 0x0120 {
            let _type_id_offset = self.is.read_i32();
        }
        self.is
            .skip_to_pos(pkg_start + header_size as u64, "package header end")?;

        let mut type_strings = None;
        if type_strings_offset != 0 {
            self.is.skip_to_pos(
                type_strings_offset as u64,
                "Expected typeStrings string pool",
            )?;
            type_strings = Some(self.parse_string_pool()?);
        }
        let mut key_strings = None;
        if key_strings_offset != 0 {
            self.is
                .skip_to_pos(key_strings_offset as u64, "Expected keyStrings string pool")?;
            key_strings = Some(self.parse_string_pool()?);
        }
        let pkg = PackageChunk {
            id,
            name,
            type_strings,
            key_strings,
        };
        self.storage.app_package = Some(verify_app_package(&pkg.name));

        while self.is.pos() < pkg_end {
            let chunk_start = self.is.pos();
            let ty = self.is.read_i16();
            match ty {
                RES_NULL_TYPE => {}
                RES_TABLE_TYPE_TYPE => self.parse_type_chunk(chunk_start, &pkg)?,
                RES_TABLE_TYPE_SPEC_TYPE => self.parse_type_spec_chunk(chunk_start)?,
                RES_TABLE_TYPE_LIBRARY => self.parse_library_type_chunk(chunk_start)?,
                RES_TABLE_TYPE_OVERLAY => self.parse_overlay_type_chunk(chunk_start)?,
                RES_TABLE_TYPE_OVERLAY_POLICY => {
                    return Err(ResError::new(format!(
                        "Encountered unsupported chunk type RES_TABLE_TYPE_OVERLAY_POLICY at offset 0x{chunk_start:x} "
                    )));
                }
                RES_TABLE_TYPE_STAGED_ALIAS => self.parse_staged_alias_chunk(chunk_start)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn parse_string_pool(&mut self) -> Result<BinaryXmlStrings> {
        self.is
            .check_i16(RES_STRING_POOL_TYPE, "String pool expected")?;
        parse_string_pool_no_type(&mut self.is)
    }

    fn die(&self, msg: &str) -> ResError {
        ResError::new(format!(
            "Decode error: {msg}, position: 0x{:x}",
            self.is.pos()
        ))
    }

    fn parse_type_spec_chunk(&mut self, chunk_start: u64) -> Result<()> {
        self.is
            .check_i16(0x0010, "Unexpected type spec header size")?;
        let chunk_size = self.is.read_i32();
        let expected_end = chunk_start.wrapping_add(i64::from(chunk_size) as u64);
        let _id = self.is.read_i8();
        self.is.skip(3)?;
        let entry_count = self.is.read_i32();
        for _ in 0..entry_count.max(0) {
            if self.is.pos() > expected_end {
                break;
            }
            let _flag = self.is.read_i32();
        }
        if self.is.pos() != expected_end {
            return Err(ResError::new(format!(
                "Error reading type spec chunk at offset 0x{chunk_start:x}"
            )));
        }
        Ok(())
    }

    fn parse_library_type_chunk(&mut self, chunk_start: u64) -> Result<()> {
        self.is.check_i16(12, "Unexpected header size")?;
        let chunk_size = self.is.read_i32();
        let expected_end = chunk_start.wrapping_add(i64::from(chunk_size) as u64);
        let count = self.is.read_i32();
        for _ in 0..count.max(0) {
            let _package_id = self.is.read_i32();
            let _name = self.is.read_string16_fixed(128)?;
            if self.is.pos() > expected_end {
                return Err(ResError::new("reading after chunk end"));
            }
        }
        if self.is.pos() != expected_end {
            return Err(ResError::new(format!(
                "Error reading library chunk at offset 0x{chunk_start:x}"
            )));
        }
        Ok(())
    }

    fn parse_overlay_type_chunk(&mut self, chunk_start: u64) -> Result<()> {
        let _header_size = self.is.read_i16();
        let chunk_size = self.is.read_i32();
        let expected_end = chunk_start.wrapping_add(i64::from(chunk_size) as u64);
        let _name = self.is.read_string16_fixed(256)?;
        let _actor = self.is.read_string16_fixed(256)?;
        self.is.skip_to_pos(expected_end, "overlay chunk end")
    }

    fn parse_staged_alias_chunk(&mut self, chunk_start: u64) -> Result<()> {
        let _header_size = self.is.read_i16();
        let chunk_size = self.is.read_i32();
        let expected_end = chunk_start.wrapping_add(i64::from(chunk_size) as u64);
        let count = self.is.read_i32();
        for _ in 0..count.max(0) {
            let _staged = self.is.read_i32();
            let _finalized = self.is.read_i32();
        }
        self.is.skip_to_pos(expected_end, "staged alias chunk end")
    }

    fn parse_type_chunk(&mut self, start: u64, pkg: &PackageChunk) -> Result<()> {
        let _header_size = self.is.read_i16();
        let chunk_size = u64::from(self.is.read_u32());
        let chunk_end = start + chunk_size;
        self.is.mark();

        let type_id = self.is.read_i8();
        let type_strings = pkg
            .type_strings
            .as_ref()
            .ok_or_else(|| ResError::new("Type strings are missing"))?;
        let _type_name = type_strings.get(type_id - 1);

        let flags = self.is.read_i8();
        let is_sparse = flags & FLAG_SPARSE != 0;
        let is_offset16 = flags & FLAG_OFFSET16 != 0;

        self.is.read_i16(); // reserved, not always zero
        let entry_count = self.is.read_i32();
        let entries_start = start as i64 + i64::from(self.is.read_i32());

        let config = self.parse_config()?;

        if entry_count < 0 || i64::from(entry_count) * 2 > self.is.remaining() {
            return Err(self.die("Invalid entry count"));
        }
        let mut offsets: Vec<EntryOffset> = Vec::with_capacity(entry_count as usize);
        if is_sparse {
            for _ in 0..entry_count {
                let idx = self.is.read_i16();
                let offset = self.is.read_i16() * 4;
                offsets.push(EntryOffset { idx, offset });
            }
        } else if is_offset16 {
            for i in 0..entry_count {
                let offset = self.is.read_i16();
                if offset != 0xFFFF {
                    offsets.push(EntryOffset {
                        idx: i,
                        offset: offset * 4,
                    });
                }
            }
        } else {
            for i in 0..entry_count {
                offsets.push(EntryOffset {
                    idx: i,
                    offset: self.is.read_i32(),
                });
            }
        }
        self.is
            .skip_to_pos(entries_start as u64, "Failed to skip to entries start")?;
        let mut processed: HashSet<i32> = HashSet::new();
        for eo in &offsets {
            if eo.offset == NO_ENTRY {
                continue;
            }
            if is_sparse && !processed.insert(eo.idx) {
                // sparse chunks may repeat an index, only the first one counts
                continue;
            }
            let entry_start = entries_start + i64::from(eo.offset);
            if entry_start >= chunk_end as i64 {
                // entries after the end of the chunk are ignored
                continue;
            }
            if (entry_start as u64) < self.is.pos() {
                self.is.reset();
            }
            if entry_start < 0 {
                return Err(self.die(&format!("Expected start of entry {}", eo.idx)));
            }
            self.is.skip_to_pos(
                entry_start as u64,
                &format!("Expected start of entry {}", eo.idx),
            )?;
            self.parse_entry(pkg, type_id, eo.idx, config.qualifiers())?;
        }
        self.is.skip_to_pos(chunk_end, "End of chunk")
    }

    fn parse_entry(
        &mut self,
        pkg: &PackageChunk,
        type_id: i32,
        entry_id: i32,
        config: &str,
    ) -> Result<()> {
        let size = self.is.read_i16();
        let flags = self.is.read_i16();
        let is_complex = flags & FLAG_COMPLEX != 0;
        let is_compact = flags & FLAG_COMPACT != 0;

        let key = if is_compact { size } else { self.is.read_i32() };
        if key == -1 {
            return Ok(());
        }
        let res_id = (pkg.id << 24) | (type_id << 16) | entry_id;
        let type_strings = pkg
            .type_strings
            .as_ref()
            .ok_or_else(|| ResError::new("Type strings are missing"))?;
        let key_strings = pkg
            .key_strings
            .as_ref()
            .ok_or_else(|| ResError::new("Key strings are missing"))?;
        let type_name = type_strings.get(type_id - 1);
        let orig_key_name = key_strings.get(key);

        let idx = self.build_resource_entry(pkg, config, res_id, &type_name, &orig_key_name);
        if is_compact {
            let data_type = flags >> 8;
            let data = self.is.read_i32();
            if let Some(i) = idx {
                self.storage.arena[i].simple_value = Some(RawValue { data_type, data });
            }
        } else if is_complex || size == 16 {
            let parent_ref = self.is.read_i32();
            let count = self.is.read_i32();
            if count < 0 {
                return Err(self.die("Invalid value count"));
            }
            let mut values = Vec::new();
            for _ in 0..count {
                let name_ref = self.is.read_i32();
                let value = self.parse_value()?;
                values.push(RawNamedValue { name_ref, value });
            }
            if let Some(i) = idx {
                let e = &mut self.storage.arena[i];
                e.parent_ref = parent_ref;
                e.named_values = Some(values);
            }
        } else {
            let v = self.parse_value()?;
            if let Some(i) = idx {
                self.storage.arena[i].simple_value = Some(v);
            }
        }
        Ok(())
    }

    fn parse_value(&mut self) -> Result<RawValue> {
        self.is.check_i16(8, "value size")?;
        self.is.check_i8(0, "value res0 not 0")?;
        let data_type = self.is.read_i8();
        let data = self.is.read_i32();
        Ok(RawValue { data_type, data })
    }

    /// Adds the entry to the storage; `None` for entries with an invalid name (ignored).
    fn build_resource_entry(
        &mut self,
        pkg: &PackageChunk,
        config: &str,
        res_id: i32,
        type_name: &str,
        orig_key_name: &str,
    ) -> Option<usize> {
        if !is_valid_entry_name(orig_key_name) {
            return None;
        }
        let new_entry = if self.opts.use_raw_names {
            ResourceEntry::new(res_id, &pkg.name, type_name, orig_key_name, config)
        } else {
            let mut res_name = self.get_res_name(res_id, orig_key_name);
            let mut entry = ResourceEntry::new(res_id, &pkg.name, type_name, &res_name, config);
            if let Some(prev_idx) = self.storage.search_same_name(&entry) {
                let prev_id = self.storage.arena[prev_idx].id;
                if prev_id == entry.id {
                    // the same resource id twice: use the original name, not the renamed one
                    res_name = orig_key_name.to_string();
                }
                entry = entry.copy_with_id(&res_name);
                // rename also the previous entry for consistency
                let replacement = self.storage.arena[prev_idx].copy_with_id(&res_name);
                let rename_id = replacement.id;
                let rename_key = replacement.key_name.clone();
                self.storage.replace(prev_idx, replacement);
                self.storage.renames.insert(rename_id, rename_key);
            }
            if orig_key_name != entry.key_name {
                self.storage
                    .renames
                    .insert(entry.id, entry.key_name.clone());
            }
            entry
        };
        Some(self.storage.add(new_entry))
    }

    fn get_res_name(&mut self, res_ref: i32, orig_key_name: &str) -> String {
        if let Some(r) = self.storage.renames.get(&res_ref) {
            return r.clone();
        }
        let const_field = self.opts.code_names.get(&res_ref);
        let new_res_name = self.get_new_res_name(res_ref, orig_key_name, const_field);
        if orig_key_name != new_res_name {
            self.storage.renames.insert(res_ref, new_res_name.clone());
        }
        if const_field.is_some() {
            self.field_renames
                .insert(res_ref, to_r_field_name(&new_res_name));
        }
        new_res_name
    }

    fn get_new_res_name(
        &self,
        res_ref: i32,
        orig_key_name: &str,
        const_field: Option<&String>,
    ) -> String {
        let mut name = match const_field {
            None => orig_key_name.to_string(),
            Some(code) => better_name(self.opts.name_source, orig_key_name, code).to_string(),
        };
        if self.opts.rename_valid {
            name = sanitize_as_resource_name(
                &name,
                &format!("_res_0x{:08x}", res_ref as u32),
                !self.opts.rename_printable,
            );
        }
        name
    }

    fn parse_config(&mut self) -> Result<EntryConfig> {
        let start = self.is.pos();
        let size = self.is.read_i32();
        if size < 4 {
            return Err(ResError::new("Config size < 4"));
        }
        // the structure is zero filled, only the present bytes are read
        let present = self.is.read_bytes((size - 4) as usize)?;
        let mut data = present;
        if data.len() < 52 {
            data.resize(52, 0);
        }
        let mut c = ParserStream::new(&data);

        let mcc = c.read_i16() as i16;
        let mnc = c.read_i16() as i16;
        let language = unpack_locale_or_region(c.read_i8() as i8, c.read_i8() as i8, b'a');
        let region = unpack_locale_or_region(c.read_i8() as i8, c.read_i8() as i8, b'0');
        let orientation = c.read_i8() as i8;
        let touchscreen = c.read_i8() as i8;
        let density = c.read_i16();
        let keyboard = c.read_i8() as i8;
        let navigation = c.read_i8() as i8;
        let input_flags = c.read_i8() as i8;
        let grammatical_inflection = c.read_i8() as i8;
        let screen_width = c.read_i16() as i16;
        let screen_height = c.read_i16() as i16;
        let sdk_version = c.read_i16() as i16;
        c.read_i16(); // minor version, always 0
        let screen_layout = c.read_i8() as i8;
        let ui_mode = c.read_i8() as i8;
        let smallest_screen_width_dp = c.read_i16() as i16;
        let screen_width_dp = c.read_i16() as i16;
        let screen_height_dp = c.read_i16() as i16;
        let locale_script = read_script_or_variant(4, &mut c)?;
        let locale_variant = read_script_or_variant(8, &mut c)?;
        let screen_layout2 = c.read_i8() as i8;
        let color_mode = c.read_i8() as i8;

        self.is
            .check_pos(start + size as u64, "Config skip trailing bytes")?;

        let to_opt = |s: Vec<u16>| if s.is_empty() { None } else { Some(s) };
        Ok(EntryConfig::from_raw(RawConfig {
            mcc,
            mnc,
            language,
            region,
            orientation,
            touchscreen,
            density,
            keyboard,
            navigation,
            input_flags,
            grammatical_inflection,
            screen_width,
            screen_height,
            sdk_version,
            screen_layout,
            ui_mode,
            smallest_screen_width_dp,
            screen_width_dp,
            screen_height_dp,
            locale_script: to_opt(locale_script),
            locale_variant: to_opt(locale_variant),
            screen_layout2,
            color_mode,
            size,
        }))
    }
}

fn verify_app_package(name: &str) -> String {
    if is_valid_full_identifier(name) {
        name.to_string()
    } else {
        "INVALID_PACKAGE".to_string()
    }
}

fn unpack_locale_or_region(in0: i8, in1: i8, base: u8) -> Vec<u16> {
    // a set high bit marks a packed three letter code
    if (in0 >> 7) & 1 == 1 {
        let first = i32::from(in1) & 0x1F;
        let second = ((i32::from(in1) & 0xE0) >> 5) + ((i32::from(in0) & 0x03) << 3);
        let third = (i32::from(in0) & 0x7C) >> 2;
        let b = i32::from(base);
        return vec![(first + b) as u16, (second + b) as u16, (third + b) as u16];
    }
    vec![in0 as i16 as u16, in1 as i16 as u16]
}

fn read_script_or_variant(len: usize, ps: &mut ParserStream) -> Result<Vec<u16>> {
    let start = ps.pos();
    let mut out = Vec::new();
    for _ in 0..len {
        let ch = ps.read_i8() as i16;
        if ch == 0 {
            break;
        }
        out.push(ch as u16);
    }
    ps.skip_to_pos(start + len as u64, "readScriptOrVariantChar")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_locale() {
        // "fil" packed: f=5,i=8,l=11
        let in0 = (0x80 | (11 << 2) | (8 >> 3)) as u8 as i8;
        let in1 = 5u8 as i8;
        assert_eq!(unpack_locale_or_region(in0, in1, b'a'), vec![102, 105, 108]);
    }

    #[test]
    fn plain_locale() {
        assert_eq!(
            unpack_locale_or_region(b'd' as i8, b'e' as i8, b'a'),
            vec![100, 101]
        );
    }
}
