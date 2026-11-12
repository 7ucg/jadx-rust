//! The [`Project`] abstraction the tools run against.
//!
//! The server never touches the engine crates directly: every tool works on this trait, which
//! hands out plain data. [`crate::engine::EngineProject`] implements it over `jadx-core`,
//! `jadx-dex-input` and `jadx-resources`; a different implementation (for example one backed by
//! the `jadx-api` crate and its generated class code) only has to implement the same trait and
//! can be installed through a [`ProjectLoader`].

use std::path::Path;
use std::sync::Arc;

use crate::error::ToolError;

/// Kind of the loaded input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Dex,
    Apk,
    Xapk,
    Jar,
    Class,
}

impl InputKind {
    pub fn name(self) -> &'static str {
        match self {
            InputKind::Dex => "dex",
            InputKind::Apk => "apk",
            InputKind::Xapk => "xapk",
            InputKind::Jar => "jar",
            InputKind::Class => "class",
        }
    }
}

/// What [`Project::class_source`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Declarations only: method bodies are placeholders.
    Skeleton,
    /// Decompiled Java code.
    Decompiled,
}

impl SourceKind {
    pub fn name(self) -> &'static str {
        match self {
            SourceKind::Skeleton => "skeleton",
            SourceKind::Decompiled => "decompiled",
        }
    }
}

/// Class or method source text.
#[derive(Debug, Clone)]
pub struct SourceText {
    pub text: String,
    pub kind: SourceKind,
}

/// Facts about a loaded project.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub kind: InputKind,
    pub path: String,
    pub size_bytes: u64,
    pub class_count: usize,
    pub method_count: usize,
    pub field_count: usize,
    /// Number of DEX files (zero for JVM input).
    pub dex_count: usize,
    pub has_resources: bool,
    pub has_smali: bool,
    pub has_strings: bool,
    pub source_kind: SourceKind,
    pub warnings: Vec<String>,
}

/// One class of the project. `id` is the dotted raw name (`a.b.Outer$Inner`).
#[derive(Debug, Clone)]
pub struct ClassEntry {
    pub id: String,
    /// Dotted name with `.` between inner and outer class (`a.b.Outer.Inner`).
    pub full_name: String,
    pub short_name: String,
    pub package: String,
    pub access: u32,
    pub super_class: Option<String>,
    pub interfaces: Vec<String>,
    pub is_inner: bool,
    pub method_count: usize,
    pub field_count: usize,
}

impl ClassEntry {
    /// Descriptor form of the class id (`La/b/Outer$Inner;`).
    pub fn descriptor(&self) -> String {
        format!("L{};", self.id.replace('.', "/"))
    }
}

#[derive(Debug, Clone)]
pub struct MethodEntry {
    /// `class_id.name(args)ret` with descriptor argument and return types.
    pub id: String,
    pub class_id: String,
    pub name: String,
    /// `(args)ret` in descriptor form.
    pub descriptor: String,
    pub access: u32,
    /// Java names of the argument types.
    pub arg_types: Vec<String>,
    pub return_type: String,
    pub signature: Option<String>,
    pub has_code: bool,
}

#[derive(Debug, Clone)]
pub struct FieldEntry {
    /// `class_id.name:descriptor`.
    pub id: String,
    pub class_id: String,
    pub name: String,
    pub descriptor: String,
    pub ty: String,
    pub access: u32,
    pub signature: Option<String>,
    /// Constant value of a static field, rendered as Java.
    pub const_value: Option<String>,
}

/// A symbol whose usages are requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Symbol {
    Class(String),
    Method(String),
    Field(String),
}

/// Places that use a symbol.
#[derive(Debug, Clone, Default)]
pub struct Usages {
    /// Methods using the symbol (method ids).
    pub methods: Vec<String>,
    /// Classes using the symbol (class ids), only filled for class usages.
    pub classes: Vec<String>,
}

/// Methods called by a method.
#[derive(Debug, Clone, Default)]
pub struct Callees {
    /// Callees that are part of the project (method ids).
    pub resolved: Vec<String>,
    /// Callees outside the project (`class.name(args)ret`).
    pub external: Vec<String>,
}

/// A string literal used by a method.
#[derive(Debug, Clone)]
pub struct StringUse {
    pub class_id: String,
    pub method_id: String,
    /// Offset of the instruction in the method, in 16-bit code units.
    pub offset: u32,
    pub value: String,
}

/// A file of the input archive.
#[derive(Debug, Clone)]
pub struct ResourceFile {
    pub path: String,
    pub size: u64,
    /// The decoded text of this file can be read with [`Project::resource_text`].
    pub text: bool,
}

/// Callback polled by long running scans; returns `true` once the request was cancelled.
pub type CancelCheck<'a> = &'a dyn Fn() -> bool;

/// A loaded input. All methods take `&self`; implementations may cache internally.
pub trait Project: Send + Sync {
    fn info(&self) -> &ProjectInfo;

    /// All classes in input order.
    fn classes(&self) -> &[ClassEntry];

    fn methods(&self, class_id: &str) -> Option<Vec<MethodEntry>>;

    fn fields(&self, class_id: &str) -> Option<Vec<FieldEntry>>;

    /// Java source of a class. The returned [`SourceKind`] tells whether it is decompiled code
    /// or a declaration skeleton.
    fn class_source(&self, class_id: &str) -> Result<SourceText, ToolError>;

    /// Java source of one method (`method_id` as in [`MethodEntry::id`]).
    fn method_source(&self, class_id: &str, method_id: &str) -> Result<SourceText, ToolError>;

    /// Smali of a class.
    fn class_smali(&self, class_id: &str) -> Result<String, ToolError>;

    /// Decoded `AndroidManifest.xml`, `None` when the input has no manifest.
    fn manifest(&self) -> Result<Option<String>, ToolError>;

    fn resource_files(&self) -> Result<Vec<ResourceFile>, ToolError>;

    /// Decoded text of a resource file, `None` when the path is unknown or not decodable.
    fn resource_text(&self, path: &str) -> Result<Option<String>, ToolError>;

    /// Strings of the string tables, optionally only those of the DEX file(s) defining a class.
    fn strings(&self, class_id: Option<&str>) -> Result<Arc<Vec<String>>, ToolError>;

    /// String literals used by code whose value satisfies `matches`; stops after `max` hits.
    fn string_uses(
        &self,
        matches: &dyn Fn(&str) -> bool,
        max: usize,
        cancel: CancelCheck<'_>,
    ) -> Result<Vec<StringUse>, ToolError>;

    fn usages(&self, symbol: &Symbol) -> Result<Usages, ToolError>;

    fn callees(&self, method_id: &str) -> Result<Callees, ToolError>;
}

/// Options of [`ProjectLoader::load`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Verify DEX checksums (the loader is lenient by default).
    pub verify_checksum: bool,
}

/// Opens projects. The default loader is [`crate::engine::EngineLoader`].
pub trait ProjectLoader: Send + Sync {
    fn load(&self, path: &Path, options: &LoadOptions) -> Result<Arc<dyn Project>, ToolError>;
}
