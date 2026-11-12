//! Lightweight views of the loaded classes, methods, fields and packages (`JavaClass`,
//! `JavaMethod`, `JavaField`, `JavaPackage`). They borrow the decompiler and are cheap to copy.

use std::fmt;
use std::sync::Arc;

use jadx_core::model::node::{ClassNode, FieldNode, MethodNode};
use jadx_core::model::types::ArgType;

use crate::access::{AccessInfo, AccessKind};
use crate::code::ICodeInfo;
use crate::decompiler::{ClassResult, JadxDecompiler};

/// A class of the input.
#[derive(Clone, Copy)]
pub struct JavaClass<'a> {
    dec: &'a JadxDecompiler,
    idx: usize,
}

impl<'a> JavaClass<'a> {
    pub(crate) fn new(dec: &'a JadxDecompiler, idx: usize) -> Self {
        JavaClass { dec, idx }
    }

    /// Index of the class in `Root::classes`.
    pub fn index(&self) -> usize {
        self.idx
    }

    pub fn class_node(&self) -> &'a ClassNode {
        &self.dec.loaded_root().classes[self.idx]
    }

    /// Short name without package and outer classes.
    pub fn name(&self) -> &'a str {
        self.class_node().info.short_name()
    }

    /// Full name with `.` as separator, also for member classes.
    pub fn full_name(&self) -> &'a str {
        self.class_node().info.full_name()
    }

    /// Name as in the input, member classes joined with `$`, dotted package.
    pub fn raw_name(&self) -> String {
        self.class_node().info.raw_name()
    }

    pub fn package(&self) -> &'a str {
        self.class_node().info.package()
    }

    pub fn java_package(&self) -> Option<JavaPackage<'a>> {
        self.dec.package_of_class(self.idx)
    }

    pub fn access_info(&self) -> AccessInfo {
        AccessInfo::new(self.class_node().access, AccessKind::Class)
    }

    pub fn super_class(&self) -> Option<&'a ArgType> {
        self.class_node().super_class.as_ref()
    }

    pub fn interfaces(&self) -> &'a [ArgType] {
        &self.class_node().interfaces
    }

    pub fn is_inner(&self) -> bool {
        self.dec.loaded_root().parent_index(self.idx).is_some()
    }

    /// The class has no code of its own (inlined into another class).
    pub fn is_no_code(&self) -> bool {
        self.class_node().dont_generate
    }

    pub fn declaring_class(&self) -> Option<JavaClass<'a>> {
        self.dec
            .loaded_root()
            .parent_index(self.idx)
            .map(|p| JavaClass::new(self.dec, p))
    }

    pub fn top_parent_class(&self) -> JavaClass<'a> {
        JavaClass::new(self.dec, self.dec.loaded_root().top_parent_index(self.idx))
    }

    /// Member classes written inside this class.
    pub fn inner_classes(&self) -> Vec<JavaClass<'a>> {
        self.dec
            .inner_class_indexes(self.idx)
            .iter()
            .map(|&i| JavaClass::new(self.dec, i))
            .collect()
    }

    /// Anonymous classes inlined into this class.
    pub fn inlined_classes(&self) -> Vec<JavaClass<'a>> {
        self.class_node()
            .inlined_classes
            .iter()
            .map(|&i| JavaClass::new(self.dec, i))
            .collect()
    }

    pub fn fields(&self) -> Vec<JavaField<'a>> {
        (0..self.class_node().fields.len())
            .map(|f| JavaField { cls: *self, idx: f })
            .collect()
    }

    pub fn methods(&self) -> Vec<JavaMethod<'a>> {
        (0..self.class_node().methods.len())
            .map(|m| JavaMethod { cls: *self, idx: m })
            .collect()
    }

    pub fn search_method_by_short_id(&self, short_id: &str) -> Option<JavaMethod<'a>> {
        let pos = self
            .class_node()
            .methods
            .iter()
            .position(|m| m.info.short_id() == short_id)?;
        Some(JavaMethod {
            cls: *self,
            idx: pos,
        })
    }

    pub fn search_field_by_short_id(&self, short_id: &str) -> Option<JavaField<'a>> {
        let pos = self
            .class_node()
            .fields
            .iter()
            .position(|f| f.info.short_id() == short_id)?;
        Some(JavaField {
            cls: *self,
            idx: pos,
        })
    }

    /// Top level classes this class depends on.
    pub fn dependencies(&self) -> Vec<JavaClass<'a>> {
        self.class_node()
            .dependencies
            .iter()
            .map(|&i| JavaClass::new(self.dec, i))
            .collect()
    }

    /// Classes using this class.
    pub fn use_in_classes(&self) -> Vec<JavaClass<'a>> {
        self.class_node()
            .use_in
            .iter()
            .map(|&i| JavaClass::new(self.dec, i))
            .collect()
    }

    /// Generate the code of the class (kept for later calls).
    pub fn decompile(&self) -> Arc<ClassResult> {
        self.dec.decompile_class(self.idx)
    }

    pub fn code_info(&self) -> Arc<ClassResult> {
        self.decompile()
    }

    /// Generated code text.
    pub fn code(&self) -> String {
        self.decompile().code.code_str().to_string()
    }

    /// Drop the kept code.
    pub fn unload(&self) {
        self.dec.unload_class(self.idx);
    }
}

impl PartialEq for JavaClass<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.dec, other.dec) && self.idx == other.idx
    }
}
impl Eq for JavaClass<'_> {}

impl std::hash::Hash for JavaClass<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.idx.hash(state);
    }
}

impl fmt::Display for JavaClass<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.full_name())
    }
}

impl fmt::Debug for JavaClass<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JavaClass({})", self.full_name())
    }
}

/// A method of a class.
#[derive(Clone, Copy)]
pub struct JavaMethod<'a> {
    cls: JavaClass<'a>,
    idx: usize,
}

impl<'a> JavaMethod<'a> {
    pub fn method_node(&self) -> &'a MethodNode {
        &self.cls.class_node().methods[self.idx]
    }

    pub fn name(&self) -> &'a str {
        &self.method_node().info.name
    }

    pub fn full_name(&self) -> String {
        format!("{}.{}", self.cls.full_name(), self.name())
    }

    pub fn raw_name(&self) -> &'a str {
        &self.method_node().info.name
    }

    /// `name(args)return` with type descriptors.
    pub fn short_id(&self) -> String {
        self.method_node().info.short_id()
    }

    pub fn declaring_class(&self) -> JavaClass<'a> {
        self.cls
    }

    pub fn top_parent_class(&self) -> JavaClass<'a> {
        self.cls.top_parent_class()
    }

    pub fn access_flags(&self) -> AccessInfo {
        AccessInfo::new(self.method_node().access, AccessKind::Method)
    }

    pub fn arguments(&self) -> &'a [ArgType] {
        &self.method_node().arg_types
    }

    pub fn return_type(&self) -> &'a ArgType {
        &self.method_node().return_type
    }

    pub fn is_constructor(&self) -> bool {
        self.method_node().info.is_constructor()
    }

    pub fn is_class_init(&self) -> bool {
        self.method_node().info.is_class_init()
    }

    pub fn calls_self(&self) -> bool {
        self.method_node().calls_self
    }

    /// Methods using this method.
    pub fn use_in(&self) -> Vec<JavaMethod<'a>> {
        self.cls.dec.methods_of(&self.method_node().use_in)
    }

    /// Methods of the input called by this method.
    pub fn used(&self) -> Vec<JavaMethod<'a>> {
        self.cls.dec.methods_of(&self.method_node().used)
    }
}

impl PartialEq for JavaMethod<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cls == other.cls && self.idx == other.idx
    }
}
impl Eq for JavaMethod<'_> {}

impl fmt::Display for JavaMethod<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.method_node().info)
    }
}

impl fmt::Debug for JavaMethod<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JavaMethod({})", self.method_node().info)
    }
}

/// A field of a class.
#[derive(Clone, Copy)]
pub struct JavaField<'a> {
    cls: JavaClass<'a>,
    idx: usize,
}

impl<'a> JavaField<'a> {
    pub fn field_node(&self) -> &'a FieldNode {
        &self.cls.class_node().fields[self.idx]
    }

    pub fn name(&self) -> &'a str {
        &self.field_node().info.name
    }

    pub fn full_name(&self) -> String {
        format!("{}.{}", self.cls.full_name(), self.name())
    }

    pub fn raw_name(&self) -> &'a str {
        &self.field_node().info.name
    }

    /// `name:descriptor`
    pub fn short_id(&self) -> String {
        self.field_node().info.short_id()
    }

    pub fn declaring_class(&self) -> JavaClass<'a> {
        self.cls
    }

    pub fn top_parent_class(&self) -> JavaClass<'a> {
        self.cls.top_parent_class()
    }

    pub fn access_flags(&self) -> AccessInfo {
        AccessInfo::new(self.field_node().access, AccessKind::Field)
    }

    pub fn field_type(&self) -> &'a ArgType {
        &self.field_node().ty
    }
}

impl PartialEq for JavaField<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cls == other.cls && self.idx == other.idx
    }
}
impl Eq for JavaField<'_> {}

impl fmt::Debug for JavaField<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JavaField({})", self.field_node().info)
    }
}

/// A package with its classes and sub packages.
#[derive(Clone, Copy)]
pub struct JavaPackage<'a> {
    dec: &'a JadxDecompiler,
    idx: usize,
}

impl<'a> JavaPackage<'a> {
    pub(crate) fn new(dec: &'a JadxDecompiler, idx: usize) -> Self {
        JavaPackage { dec, idx }
    }

    /// Last part of the package name.
    pub fn name(&self) -> &'a str {
        let full = self.full_name();
        full.rsplit('.').next().unwrap_or(full)
    }

    pub fn full_name(&self) -> &'a str {
        self.dec.package_data(self.idx).full_name.as_str()
    }

    pub fn raw_name(&self) -> &'a str {
        self.name()
    }

    pub fn raw_full_name(&self) -> &'a str {
        self.full_name()
    }

    pub fn sub_packages(&self) -> Vec<JavaPackage<'a>> {
        self.dec
            .package_data(self.idx)
            .sub_packages
            .iter()
            .map(|&i| JavaPackage::new(self.dec, i))
            .collect()
    }

    /// Top level classes of the package.
    pub fn classes(&self) -> Vec<JavaClass<'a>> {
        self.dec
            .package_data(self.idx)
            .classes
            .iter()
            .map(|&i| JavaClass::new(self.dec, i))
            .collect()
    }

    pub fn is_root(&self) -> bool {
        self.dec.package_data(self.idx).parent.is_none()
    }

    pub fn is_leaf(&self) -> bool {
        self.dec.package_data(self.idx).sub_packages.is_empty()
    }

    pub fn is_default(&self) -> bool {
        self.full_name().is_empty()
    }
}

impl PartialEq for JavaPackage<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.dec, other.dec) && self.idx == other.idx
    }
}
impl Eq for JavaPackage<'_> {}

impl fmt::Display for JavaPackage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.full_name())
    }
}

impl fmt::Debug for JavaPackage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JavaPackage({})", self.full_name())
    }
}

impl ICodeInfo for ClassResult {
    fn code_str(&self) -> &str {
        self.code.code_str()
    }
}
