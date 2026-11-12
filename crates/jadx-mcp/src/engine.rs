//! [`Project`] implementation over the engine crates: `jadx-dex-input` / `jadx-java-input` for
//! reading, `jadx-core` for the class model and the pre-decompile usage information,
//! `jadx-resources` for the manifest and resources.
//!
//! Class source is currently the declaration skeleton of [`crate::skeleton`]; usages come from
//! the pre-decompile passes (`MethodNode::use_in`, `FieldNode::use_in`, `ClassNode::use_in`).

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use jadx_core::model::info::{make_short_id, type_signature};
use jadx_core::model::node::{ClassNode, FieldNode, MethodId, MethodNode, Root};
use jadx_core::passes::pre_decompile::{run_pre_decompile, PreDecompileOptions};
use jadx_dex_input::{DexCodeRefs, DexFile};
use jadx_input_api::{ClassData, ConstValue, InsnIndexType};
use jadx_resources::{decode_apk, ApkOptions, ApkResources};

use crate::error::ToolError;
use crate::project::{
    Callees, CancelCheck, ClassEntry, FieldEntry, InputKind, LoadOptions, MethodEntry, Project,
    ProjectInfo, ProjectLoader, ResourceFile, SourceKind, SourceText, StringUse, Symbol, Usages,
};
use crate::skeleton;

const DEX_MAGIC: &[u8; 4] = b"dex\n";
const CLASS_MAGIC: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];
const ZIP_MAGIC: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];
const MANIFEST: &str = "AndroidManifest.xml";

/// Loads projects with the engine crates.
#[derive(Debug, Default, Clone, Copy)]
pub struct EngineLoader;

impl ProjectLoader for EngineLoader {
    fn load(&self, path: &Path, options: &LoadOptions) -> Result<Arc<dyn Project>, ToolError> {
        let project = EngineProject::open(path, options)?;
        Ok(Arc::new(project))
    }
}

/// Where the resources of the project can be decoded from.
enum ResourceSource {
    None,
    /// An archive on disk. The temporary copy of an archive extracted from a bundle is removed
    /// when the project is dropped.
    Archive {
        path: PathBuf,
        temporary: bool,
    },
}

impl Drop for ResourceSource {
    fn drop(&mut self) {
        if let ResourceSource::Archive {
            path,
            temporary: true,
        } = self
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub struct EngineProject {
    info: ProjectInfo,
    root: Root,
    entries: Vec<ClassEntry>,
    dexes: Vec<DexFile>,
    /// `(dex index, class definition index)` per class; empty for JVM input.
    locations: Vec<(usize, u32)>,
    resources: ResourceSource,
    decoded: OnceLock<Result<Arc<ApkResources>, String>>,
    strings: Mutex<HashMap<usize, Arc<Vec<String>>>>,
}

/// Everything read from the input before the class model is built.
struct Loaded {
    kind: InputKind,
    dexes: Vec<DexFile>,
    java: Vec<jadx_java_input::JavaClass>,
    resources: ResourceSource,
    warnings: Vec<String>,
    size: u64,
}

impl EngineProject {
    pub fn open(path: &Path, options: &LoadOptions) -> Result<Self, ToolError> {
        let loaded = read_input(path, options)?;
        // The engine does not panic on malformed input by design; keep the server alive if it
        // ever does.
        std::panic::catch_unwind(AssertUnwindSafe(|| Self::build(path, loaded))).map_err(|_| {
            ToolError::new("LOAD_FAILED", "The engine failed while analysing the input")
        })?
    }

    fn build(path: &Path, loaded: Loaded) -> Result<Self, ToolError> {
        let Loaded {
            kind,
            dexes,
            java,
            resources,
            mut warnings,
            size,
        } = loaded;
        let mut data: Vec<ClassData> = Vec::new();
        let mut locations = Vec::new();
        if java.is_empty() {
            for (di, dex) in dexes.iter().enumerate() {
                for (ci, def) in dex.class_defs().enumerate() {
                    match def.and_then(|d| dex.class_api(&d)) {
                        Ok(class) => {
                            data.push(class);
                            locations.push((di, ci as u32));
                        }
                        Err(e) => {
                            warnings.push(format!("class #{ci} of {} skipped: {e}", dex.name()))
                        }
                    }
                }
            }
        } else {
            for class in &java {
                match class.class_api() {
                    Ok(c) => data.push(c),
                    Err(e) => warnings.push(format!("class skipped: {e}")),
                }
            }
        }
        if data.is_empty() {
            let mut message = String::from("The input contains no classes");
            if let Some(first) = warnings.first() {
                message.push_str(&format!(" ({first})"));
            }
            return Err(ToolError::new("LOAD_FAILED", message));
        }
        let mut root = Root::build(data, true).map_err(|e| ToolError::new("LOAD_FAILED", e))?;
        let opts = PreDecompileOptions::default();
        if java.is_empty() {
            let class_dex: Vec<usize> = locations.iter().map(|l| l.0).collect();
            let refs = DexCodeRefs {
                dexes: &dexes,
                class_dex: &class_dex,
            };
            run_pre_decompile(&mut root, &refs, &opts);
        } else {
            let refs = jadx_java_input::JavaCodeRefs { classes: &java };
            run_pre_decompile(&mut root, &refs, &opts);
        }
        let entries: Vec<ClassEntry> = root.classes.iter().map(class_entry).collect();
        let info = ProjectInfo {
            kind,
            path: path.to_string_lossy().into_owned(),
            size_bytes: size,
            class_count: entries.len(),
            method_count: root.classes.iter().map(|c| c.methods.len()).sum(),
            field_count: root.classes.iter().map(|c| c.fields.len()).sum(),
            dex_count: dexes.len(),
            has_resources: !matches!(resources, ResourceSource::None),
            has_smali: java.is_empty(),
            has_strings: java.is_empty(),
            source_kind: SourceKind::Skeleton,
            warnings,
        };
        Ok(EngineProject {
            info,
            root,
            entries,
            dexes,
            locations,
            resources,
            decoded: OnceLock::new(),
            strings: Mutex::new(HashMap::new()),
        })
    }

    fn class_node(&self, class_id: &str) -> Result<(usize, &ClassNode), ToolError> {
        let index = self.root.class_index(class_id).ok_or_else(|| {
            ToolError::new("SYMBOL_NOT_FOUND", format!("Unknown class {class_id}"))
        })?;
        Ok((index, &self.root.classes[index]))
    }

    fn method_id(&self, (c, m): MethodId) -> String {
        let class = &self.root.classes[c];
        method_id_of(class, &class.methods[m])
    }

    fn method_node(&self, method_id: &str) -> Result<(MethodId, &MethodNode), ToolError> {
        let (class_id, short) = split_method_id(method_id)
            .ok_or_else(|| ToolError::invalid(format!("Not a method id: {method_id}")))?;
        let (ci, class) = self.class_node(class_id)?;
        let mi = class
            .methods
            .iter()
            .position(|m| m.info.short_id() == short)
            .ok_or_else(|| {
                ToolError::new("SYMBOL_NOT_FOUND", format!("Unknown method {method_id}"))
            })?;
        Ok(((ci, mi), &class.methods[mi]))
    }

    fn decoded_resources(&self) -> Result<Option<Arc<ApkResources>>, ToolError> {
        let ResourceSource::Archive { path, .. } = &self.resources else {
            return Ok(None);
        };
        let result = self.decoded.get_or_init(|| {
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                decode_apk(path, &ApkOptions::default())
            }))
            .map_err(|_| "resource decoder failed".to_string())
            .and_then(|r| r.map_err(|e| e.to_string()))
            .map(Arc::new)
        });
        match result {
            Ok(res) => Ok(Some(res.clone())),
            Err(e) => Err(ToolError::failed(format!("Resource decoding failed: {e}"))),
        }
    }

    fn dex_strings(&self, dex_index: usize) -> Arc<Vec<String>> {
        let mut cache = self.strings.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = cache.get(&dex_index) {
            return found.clone();
        }
        let dex = &self.dexes[dex_index];
        let list: Vec<String> = (0..dex.string_count())
            .filter_map(|i| dex.string(i).ok())
            .collect();
        let list = Arc::new(list);
        cache.insert(dex_index, list.clone());
        list
    }
}

fn class_entry(class: &ClassNode) -> ClassEntry {
    let raw =
        |t: &jadx_core::model::types::ArgType| t.object_name().unwrap_or_else(|| t.to_string());
    ClassEntry {
        id: class.info.raw_name(),
        full_name: class.info.full_name().to_string(),
        short_name: class.info.short_name().to_string(),
        package: class.info.package().to_string(),
        access: class.access,
        super_class: class.super_class.as_ref().map(raw),
        interfaces: class.interfaces.iter().map(raw).collect(),
        is_inner: class.is_inner(),
        method_count: class.methods.len(),
        field_count: class.fields.len(),
    }
}

fn method_id_of(class: &ClassNode, method: &MethodNode) -> String {
    format!("{}.{}", class.info.raw_name(), method.info.short_id())
}

/// Split `class.name(args)ret` into class id and short id.
pub fn split_method_id(id: &str) -> Option<(&str, &str)> {
    let paren = id.find('(')?;
    let dot = id[..paren].rfind('.')?;
    Some((&id[..dot], &id[dot + 1..]))
}

/// Split `class.name:descriptor` into class id, name and descriptor.
pub fn split_field_id(id: &str) -> Option<(&str, &str, &str)> {
    let (head, descriptor) = id.rsplit_once(':')?;
    let dot = head.rfind('.')?;
    Some((&head[..dot], &head[dot + 1..], descriptor))
}

fn method_entry(class: &ClassNode, method: &MethodNode) -> MethodEntry {
    let short = method.info.short_id();
    MethodEntry {
        id: format!("{}.{short}", class.info.raw_name()),
        class_id: class.info.raw_name(),
        name: method.info.name.clone(),
        descriptor: short[method.info.name.len()..].to_string(),
        access: method.access,
        arg_types: method.arg_types.iter().map(|t| t.to_string()).collect(),
        return_type: method.return_type.to_string(),
        signature: method.signature.clone(),
        has_code: method.code_offset.is_some(),
    }
}

fn field_entry(class: &ClassNode, field: &FieldNode) -> FieldEntry {
    let descriptor = type_signature(&field.info.ty);
    FieldEntry {
        id: format!("{}.{}:{descriptor}", class.info.raw_name(), field.info.name),
        class_id: class.info.raw_name(),
        name: field.info.name.clone(),
        descriptor,
        ty: field.ty.to_string(),
        access: field.access,
        signature: field.signature.clone(),
        const_value: field.const_value.as_ref().map(render_const),
    }
}

fn escape_java(units: &[u16], quote: char) -> String {
    let mut out = String::new();
    for ch in char::decode_utf16(units.iter().copied()) {
        match ch {
            Ok('\n') => out.push_str("\\n"),
            Ok('\r') => out.push_str("\\r"),
            Ok('\t') => out.push_str("\\t"),
            Ok('\\') => out.push_str("\\\\"),
            Ok(c) if c == quote => {
                out.push('\\');
                out.push(c);
            }
            Ok(c) if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            Ok(c) => out.push(c),
            Err(e) => out.push_str(&format!("\\u{:04x}", e.unpaired_surrogate())),
        }
    }
    out
}

fn render_const(value: &ConstValue) -> String {
    match value {
        ConstValue::Boolean(b) => b.to_string(),
        ConstValue::Byte(v) => v.to_string(),
        ConstValue::Short(v) => v.to_string(),
        ConstValue::Int(v) => v.to_string(),
        ConstValue::Long(v) => format!("{v}L"),
        ConstValue::Char(c) => format!("'{}'", escape_java(&[*c], '\'')),
        ConstValue::Float(bits) => format!("{:?}f", f32::from_bits(*bits)),
        ConstValue::Double(bits) => format!("{:?}", f64::from_bits(*bits)),
        ConstValue::Str(units) => format!("\"{}\"", escape_java(units, '"')),
    }
}

impl Project for EngineProject {
    fn info(&self) -> &ProjectInfo {
        &self.info
    }

    fn classes(&self) -> &[ClassEntry] {
        &self.entries
    }

    fn methods(&self, class_id: &str) -> Option<Vec<MethodEntry>> {
        let (_, class) = self.class_node(class_id).ok()?;
        Some(
            class
                .methods
                .iter()
                .map(|m| method_entry(class, m))
                .collect(),
        )
    }

    fn fields(&self, class_id: &str) -> Option<Vec<FieldEntry>> {
        let (_, class) = self.class_node(class_id).ok()?;
        Some(class.fields.iter().map(|f| field_entry(class, f)).collect())
    }

    fn class_source(&self, class_id: &str) -> Result<SourceText, ToolError> {
        let (index, class) = self.class_node(class_id)?;
        let fields: Vec<FieldEntry> = class.fields.iter().map(|f| field_entry(class, f)).collect();
        let methods: Vec<MethodEntry> = class
            .methods
            .iter()
            .map(|m| method_entry(class, m))
            .collect();
        Ok(SourceText {
            text: skeleton::class_skeleton(&self.entries[index], &fields, &methods),
            kind: SourceKind::Skeleton,
        })
    }

    fn method_source(&self, class_id: &str, method_id: &str) -> Result<SourceText, ToolError> {
        let (index, class) = self.class_node(class_id)?;
        let ((_, mi), _) = self.method_node(method_id)?;
        let method = method_entry(class, &class.methods[mi]);
        Ok(SourceText {
            text: skeleton::method_skeleton(&self.entries[index], &method),
            kind: SourceKind::Skeleton,
        })
    }

    fn class_smali(&self, class_id: &str) -> Result<String, ToolError> {
        let (index, _) = self.class_node(class_id)?;
        let Some((di, def)) = self.locations.get(index) else {
            return Err(ToolError::unsupported(
                "Smali is only available for DEX input",
            ));
        };
        self.dexes[*di]
            .class_smali(*def)
            .map_err(|e| ToolError::failed(format!("Disassembly failed: {e}")))
    }

    fn manifest(&self) -> Result<Option<String>, ToolError> {
        let Some(res) = self.decoded_resources()? else {
            return Ok(None);
        };
        Ok(res
            .files
            .iter()
            .rev()
            .find(|f| f.path == MANIFEST)
            .map(|f| f.text.clone()))
    }

    fn resource_files(&self) -> Result<Vec<ResourceFile>, ToolError> {
        let ResourceSource::Archive { path, .. } = &self.resources else {
            return Ok(Vec::new());
        };
        let decoded = self.decoded_resources()?;
        let decoded_paths: HashMap<&str, usize> = decoded
            .as_ref()
            .map(|d| {
                d.files
                    .iter()
                    .map(|f| (f.path.as_str(), f.text.len()))
                    .collect()
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if let Ok(file) = std::fs::File::open(path) {
            if let Ok(mut zip) = zip::ZipArchive::new(file) {
                for i in 0..zip.len() {
                    let Ok(entry) = zip.by_index_raw(i) else {
                        continue;
                    };
                    if entry.is_dir() {
                        continue;
                    }
                    let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
                    out.push(ResourceFile {
                        text: decoded_paths.contains_key(name.as_str()),
                        size: entry.size(),
                        path: name.clone(),
                    });
                    seen.insert(name);
                }
            }
        }
        // generated files (values/*.xml, public.xml) have no archive entry
        let mut extra: Vec<(&str, usize)> = decoded_paths
            .iter()
            .filter(|(p, _)| !seen.contains(**p))
            .map(|(p, n)| (*p, *n))
            .collect();
        extra.sort_unstable();
        out.extend(extra.into_iter().map(|(p, n)| ResourceFile {
            path: p.to_string(),
            size: n as u64,
            text: true,
        }));
        Ok(out)
    }

    fn resource_text(&self, path: &str) -> Result<Option<String>, ToolError> {
        let Some(res) = self.decoded_resources()? else {
            return Ok(None);
        };
        Ok(res
            .files
            .iter()
            .rev()
            .find(|f| f.path == path)
            .map(|f| f.text.clone()))
    }

    fn strings(&self, class_id: Option<&str>) -> Result<Arc<Vec<String>>, ToolError> {
        if self.dexes.is_empty() {
            return Err(ToolError::unsupported(
                "String tables are only available for DEX input",
            ));
        }
        if let Some(id) = class_id {
            let (index, _) = self.class_node(id)?;
            let (di, _) = self.locations[index];
            return Ok(self.dex_strings(di));
        }
        if self.dexes.len() == 1 {
            return Ok(self.dex_strings(0));
        }
        let mut seen = HashSet::new();
        let mut all = Vec::new();
        for di in 0..self.dexes.len() {
            for s in self.dex_strings(di).iter() {
                if seen.insert(s.clone()) {
                    all.push(s.clone());
                }
            }
        }
        Ok(Arc::new(all))
    }

    fn string_uses(
        &self,
        matches: &dyn Fn(&str) -> bool,
        max: usize,
        cancel: CancelCheck<'_>,
    ) -> Result<Vec<StringUse>, ToolError> {
        if self.dexes.is_empty() {
            return Err(ToolError::unsupported(
                "String references are only available for DEX input",
            ));
        }
        let mut out = Vec::new();
        let mut verdicts: HashMap<(usize, u32), Option<String>> = HashMap::new();
        for (ci, class) in self.root.classes.iter().enumerate() {
            if cancel() {
                return Err(ToolError::cancelled());
            }
            let (di, _) = self.locations[ci];
            let dex = &self.dexes[di];
            for method in &class.methods {
                let Some(code_off) = method.code_offset else {
                    continue;
                };
                let Ok(code) = dex.code_item(code_off) else {
                    continue;
                };
                let Ok(insns) = jadx_dex_input::insns::decode_insn_data(&code.insns) else {
                    continue;
                };
                for insn in insns {
                    if insn.index_type != InsnIndexType::StringRef {
                        continue;
                    }
                    let verdict = verdicts
                        .entry((di, insn.index))
                        .or_insert_with(|| dex.string(insn.index).ok().filter(|s| matches(s)));
                    let Some(value) = verdict else {
                        continue;
                    };
                    out.push(StringUse {
                        class_id: class.info.raw_name(),
                        method_id: method_id_of(class, method),
                        offset: insn.offset,
                        value: value.clone(),
                    });
                    if out.len() >= max {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }

    fn usages(&self, symbol: &Symbol) -> Result<Usages, ToolError> {
        match symbol {
            Symbol::Class(id) => {
                let (_, class) = self.class_node(id)?;
                Ok(Usages {
                    methods: class
                        .use_in_mth
                        .iter()
                        .map(|m| self.method_id(*m))
                        .collect(),
                    classes: class
                        .use_in
                        .iter()
                        .map(|c| self.root.classes[*c].info.raw_name())
                        .collect(),
                })
            }
            Symbol::Method(id) => {
                let (_, method) = self.method_node(id)?;
                Ok(Usages {
                    methods: method.use_in.iter().map(|m| self.method_id(*m)).collect(),
                    classes: Vec::new(),
                })
            }
            Symbol::Field(id) => {
                let (class_id, name, descriptor) = split_field_id(id)
                    .ok_or_else(|| ToolError::invalid(format!("Not a field id: {id}")))?;
                let (_, class) = self.class_node(class_id)?;
                let field = class
                    .fields
                    .iter()
                    .find(|f| f.info.name == name && type_signature(&f.info.ty) == descriptor)
                    .ok_or_else(|| {
                        ToolError::new("SYMBOL_NOT_FOUND", format!("Unknown field {id}"))
                    })?;
                Ok(Usages {
                    methods: field.use_in.iter().map(|m| self.method_id(*m)).collect(),
                    classes: Vec::new(),
                })
            }
        }
    }

    fn callees(&self, method_id: &str) -> Result<Callees, ToolError> {
        let (_, method) = self.method_node(method_id)?;
        Ok(Callees {
            resolved: method.used.iter().map(|m| self.method_id(*m)).collect(),
            external: method
                .unresolved_used
                .iter()
                .map(|m| {
                    format!(
                        "{}.{}",
                        m.decl_class.raw_name(),
                        make_short_id(&m.name, &m.arg_types, Some(&m.return_type))
                    )
                })
                .collect(),
        })
    }
}

fn io_error(path: &Path, e: &std::io::Error) -> ToolError {
    ToolError::new(
        "LOAD_FAILED",
        format!("Cannot read {}: {e}", path.display()),
    )
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "input".to_string())
}

/// Read the input and detect its kind from the content.
fn read_input(path: &Path, options: &LoadOptions) -> Result<Loaded, ToolError> {
    let bytes = std::fs::read(path).map_err(|e| io_error(path, &e))?;
    let size = bytes.len() as u64;
    let name = display_name(path);
    let dex_opts = jadx_dex_input::LoadOptions {
        verify_checksum: options.verify_checksum,
    };
    let load_err = |e: jadx_dex_input::DexError| ToolError::new("LOAD_FAILED", e.to_string());
    if bytes.starts_with(DEX_MAGIC) {
        let res = jadx_dex_input::load_bytes(&name, bytes, &dex_opts, false).map_err(load_err)?;
        return Ok(Loaded {
            kind: InputKind::Dex,
            dexes: res.dex_files,
            java: Vec::new(),
            resources: ResourceSource::None,
            warnings: res.warnings,
            size,
        });
    }
    if bytes.starts_with(&CLASS_MAGIC) {
        let res = jadx_java_input::load_bytes(&name, bytes);
        return Ok(Loaded {
            kind: InputKind::Class,
            dexes: Vec::new(),
            java: res.classes,
            resources: ResourceSource::None,
            warnings: res.warnings,
            size,
        });
    }
    if !bytes.starts_with(&ZIP_MAGIC) {
        return Err(ToolError::new(
            "LOAD_FAILED",
            "Unsupported input: expected a dex, class, jar, apk or xapk file",
        ));
    }
    let names: Vec<String> = {
        let archive = zip::ZipArchive::new(Cursor::new(&bytes[..]))
            .map_err(|e| ToolError::new("LOAD_FAILED", format!("Invalid archive: {e}")))?;
        archive
            .file_names()
            .filter_map(|n| n.ok().map(|n| n.into_owned()))
            .collect()
    };
    let has_manifest = names.iter().any(|n| n == MANIFEST);
    let nested_apks: Vec<&String> = names.iter().filter(|n| n.ends_with(".apk")).collect();
    if !has_manifest && !nested_apks.is_empty() {
        return load_bundle(&bytes, &nested_apks, &dex_opts, size);
    }
    let res =
        jadx_dex_input::load_bytes(&name, bytes.clone(), &dex_opts, true).map_err(load_err)?;
    if !res.dex_files.is_empty() {
        let resources = if has_manifest {
            ResourceSource::Archive {
                path: path.to_path_buf(),
                temporary: false,
            }
        } else {
            ResourceSource::None
        };
        return Ok(Loaded {
            kind: if has_manifest {
                InputKind::Apk
            } else {
                InputKind::Jar
            },
            dexes: res.dex_files,
            java: Vec::new(),
            resources,
            warnings: res.warnings,
            size,
        });
    }
    let java = jadx_java_input::load_bytes(&name, bytes);
    let mut warnings = res.warnings;
    warnings.extend(java.warnings);
    Ok(Loaded {
        kind: InputKind::Jar,
        dexes: Vec::new(),
        java: java.classes,
        resources: ResourceSource::None,
        warnings,
        size,
    })
}

/// A bundle (xapk) holding one or more apks: code from all of them, resources of the apk with the
/// most code (the base apk).
fn load_bundle(
    bytes: &[u8],
    apks: &[&String],
    dex_opts: &jadx_dex_input::LoadOptions,
    size: u64,
) -> Result<Loaded, ToolError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| ToolError::new("LOAD_FAILED", format!("Invalid archive: {e}")))?;
    let mut dexes = Vec::new();
    let mut warnings = Vec::new();
    let mut best: Option<(usize, Vec<u8>)> = None;
    for apk in apks {
        let mut content = Vec::new();
        let read = archive
            .by_name(apk)
            .map_err(|e| e.to_string())
            .and_then(|mut f| f.read_to_end(&mut content).map_err(|e| e.to_string()));
        if let Err(e) = read {
            warnings.push(format!("Cannot read {apk}: {e}"));
            continue;
        }
        let name = apk.rsplit('/').next().unwrap_or(apk).to_string();
        match jadx_dex_input::load_bytes(&name, content.clone(), dex_opts, true) {
            Ok(res) => {
                warnings.extend(res.warnings);
                let count = res.dex_files.len();
                if best.as_ref().is_none_or(|(n, _)| count > *n) {
                    best = Some((count, content));
                }
                dexes.extend(res.dex_files);
            }
            Err(e) => warnings.push(format!("Cannot load {apk}: {e}")),
        }
    }
    let resources = match best {
        Some((_, content)) => match write_temporary_apk(&content) {
            Ok(path) => ResourceSource::Archive {
                path,
                temporary: true,
            },
            Err(e) => {
                warnings.push(format!("Resources unavailable: {e}"));
                ResourceSource::None
            }
        },
        None => ResourceSource::None,
    };
    Ok(Loaded {
        kind: InputKind::Xapk,
        dexes,
        java: Vec::new(),
        resources,
        warnings,
        size,
    })
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn write_temporary_apk(content: &[u8]) -> std::io::Result<PathBuf> {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("jadx-mcp-{}-{n}.apk", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    if let Err(e) = file.write_all(content) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_splitting() {
        assert_eq!(
            split_method_id("a.b.Foo$Bar.<init>(ILjava/lang/String;)V"),
            Some(("a.b.Foo$Bar", "<init>(ILjava/lang/String;)V"))
        );
        assert_eq!(split_method_id("a.b.Foo"), None);
        assert_eq!(
            split_field_id("a.b.Foo.x:Ljava/lang/String;"),
            Some(("a.b.Foo", "x", "Ljava/lang/String;"))
        );
        assert_eq!(split_field_id("a.b.Foo"), None);
    }

    #[test]
    fn constants_render_as_java() {
        assert_eq!(render_const(&ConstValue::Long(-1)), "-1L");
        assert_eq!(render_const(&ConstValue::Char(0x71)), "'q'");
        assert_eq!(render_const(&ConstValue::from_f32(1.5)), "1.5f");
        assert_eq!(
            render_const(&ConstValue::Str("a\"b\n".encode_utf16().collect())),
            "\"a\\\"b\\n\""
        );
        assert_eq!(render_const(&ConstValue::Boolean(true)), "true");
    }
}
