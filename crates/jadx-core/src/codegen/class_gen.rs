//! Class level helpers of the code generator (`ClassGen`): type and class names, import
//! bookkeeping and the short/full name decision.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use super::code_writer::CodeWriter;
use super::string_utils::StringUtils;
use super::CodegenError;
use crate::model::info::ClassInfo;
use crate::model::node::Root;
use crate::model::types::{ArgType, PrimitiveType, WildcardBound};

/// Data shared by all generators of one root.
pub struct GenEnv<'a> {
    pub root: &'a Root,
    /// Inner classes per class index (`ClassNode.getInnerClasses`).
    pub inner_classes: Vec<Vec<usize>>,
    /// Names of the root packages (`CacheStorage.getRootPkgs`).
    pub root_pkgs: HashSet<String>,
    pub string_utils: StringUtils,
    /// `JadxArgs.isUseImports`
    pub use_imports: bool,
    /// `JadxArgs.isFallbackMode`
    pub fallback: bool,
}

impl<'a> GenEnv<'a> {
    pub fn new(root: &'a Root) -> Self {
        let mut inner_classes: Vec<Vec<usize>> = vec![Vec::new(); root.classes.len()];
        let mut root_pkgs = HashSet::new();
        for (i, c) in root.classes.iter().enumerate() {
            if let Some(p) = root.parent_index(i) {
                inner_classes[p].push(i);
            }
            let pkg = c.info.package();
            if !pkg.is_empty() {
                root_pkgs.insert(pkg.split('.').next().unwrap_or(pkg).to_string());
            }
        }
        GenEnv {
            root,
            inner_classes,
            root_pkgs,
            string_utils: StringUtils::default(),
            use_imports: true,
            fallback: false,
        }
    }

    /// `ClassInfo.fromType`
    pub fn class_info(&self, ty: &ArgType) -> Result<ClassInfo, CodegenError> {
        self.root
            .ctx
            .class_info(ty)
            .map_err(|e| CodegenError::new(format!("Not class type: {e}")))
    }

    /// `ClassInfo.fromName`
    pub fn class_info_by_name(&self, name: &str) -> Result<ClassInfo, CodegenError> {
        self.class_info(&ArgType::object(name))
    }

    /// `RootNode.resolveClass(ClassInfo)`
    pub fn resolve_class_info(&self, ci: &ClassInfo) -> Option<usize> {
        self.root.class_index(&ci.raw_name())
    }
}

/// `ClassGen`
pub struct ClassGen<'a> {
    env: &'a GenEnv<'a>,
    cls: usize,
    parent: Option<&'a ClassGen<'a>>,
    imports: RefCell<HashSet<ClassInfo>>,
    body_gen_started: Cell<bool>,
    /// Own member classes are not in scope in the class header (JLS 6.3).
    in_class_header: Cell<bool>,
}

impl<'a> ClassGen<'a> {
    pub fn new(env: &'a GenEnv<'a>, cls: usize) -> Self {
        ClassGen {
            env,
            cls,
            parent: None,
            imports: RefCell::new(HashSet::new()),
            body_gen_started: Cell::new(false),
            in_class_header: Cell::new(false),
        }
    }

    /// Generator of an inner class sharing the imports of its parent generator.
    pub fn with_parent(env: &'a GenEnv<'a>, cls: usize, parent: &'a ClassGen<'a>) -> Self {
        ClassGen {
            parent: Some(parent),
            ..ClassGen::new(env, cls)
        }
    }

    pub fn env(&self) -> &'a GenEnv<'a> {
        self.env
    }

    pub fn class_index(&self) -> usize {
        self.cls
    }

    pub fn class_info(&self) -> &ClassInfo {
        &self.env.root.classes[self.cls].info
    }

    pub fn is_fallback_mode(&self) -> bool {
        self.env.fallback
    }

    pub fn is_body_gen_started(&self) -> bool {
        self.body_gen_started.get()
    }

    pub fn set_body_gen_started(&self, started: bool) {
        self.body_gen_started.set(started);
    }

    pub fn set_in_class_header(&self, in_header: bool) {
        self.in_class_header.set(in_header);
    }

    pub fn parent_gen(&self) -> &ClassGen<'a> {
        self.parent.unwrap_or(self)
    }

    /// Collected imports (of the top level generator).
    pub fn imports(&self) -> Vec<ClassInfo> {
        match self.parent {
            Some(p) => p.imports(),
            None => self.imports.borrow().iter().cloned().collect(),
        }
    }

    fn add_import(&self, ci: &ClassInfo) {
        match self.parent {
            Some(p) => p.add_import(ci),
            None => {
                self.imports.borrow_mut().insert(ci.clone());
            }
        }
    }

    fn has_import_with_short_name(&self, ext: &ClassInfo, short: &str) -> bool {
        match self.parent {
            Some(p) => p.has_import_with_short_name(ext, short),
            None => self
                .imports
                .borrow()
                .iter()
                .any(|i| i != ext && i.short_name() == short),
        }
    }

    /// `useType`
    pub fn use_type(&self, code: &mut CodeWriter, ty: &ArgType) -> Result<(), CodegenError> {
        match ty.primitive_type() {
            None => {
                code.add(&ty.to_string());
            }
            Some(PrimitiveType::Object) => {
                if let ArgType::GenericType { name, .. } = ty {
                    code.add(name);
                } else {
                    self.use_class(code, ty)?;
                }
            }
            Some(PrimitiveType::Array) => {
                if let Some(e) = ty.array_element() {
                    self.use_type(code, e)?;
                }
                code.add("[]");
            }
            Some(p) => {
                code.add(p.long_name());
            }
        }
        Ok(())
    }

    /// `useClass(ICodeWriter, ArgType)`
    pub fn use_class(&self, code: &mut CodeWriter, ty: &ArgType) -> Result<(), CodegenError> {
        if let Some(outer) = ty.outer_type() {
            self.use_class(code, outer)?;
            code.add(".");
            return self.add_inner_type(code, ty);
        }
        let ci = self.env.class_info(ty)?;
        self.use_class_info(code, &ci)?;
        self.add_generics(code, ty)
    }

    fn add_inner_type(&self, code: &mut CodeWriter, base: &ArgType) -> Result<(), CodegenError> {
        let Some(inner) = base.inner_type() else {
            return Ok(());
        };
        if let Some(outer) = inner.outer_type() {
            self.use_class_with_short_name(code, base, outer)?;
            code.add(".");
            return self.add_inner_type(code, inner);
        }
        self.use_class_with_short_name(code, base, inner)
    }

    fn use_class_with_short_name(
        &self,
        code: &mut CodeWriter,
        base: &ArgType,
        ty: &ArgType,
    ) -> Result<(), CodegenError> {
        let ty_name = ty.object_name().unwrap_or_default();
        let full = if ty_name.contains('.') {
            ty_name
        } else {
            base.object_name().unwrap_or_default()
        };
        let ci = self.env.class_info_by_name(&full)?;
        code.add(ci.short_name());
        self.add_generics(code, ty)
    }

    fn add_generics(&self, code: &mut CodeWriter, ty: &ArgType) -> Result<(), CodegenError> {
        let Some(generics) = ty.generic_types() else {
            return Ok(());
        };
        code.add("<");
        for (i, gt) in generics.iter().enumerate() {
            if i != 0 {
                code.add(", ");
            }
            match gt.wildcard_type() {
                Some(wt) => {
                    let bound = gt.wildcard_bound().unwrap_or(WildcardBound::Unbound);
                    code.add(bound.as_str());
                    if bound != WildcardBound::Unbound {
                        self.use_type(code, wt)?;
                    }
                }
                None => self.use_type(code, gt)?,
            }
        }
        code.add(">");
        Ok(())
    }

    /// `useClass(ICodeWriter, ClassInfo)`
    pub fn use_class_info(
        &self,
        code: &mut CodeWriter,
        ci: &ClassInfo,
    ) -> Result<(), CodegenError> {
        self.add_cls_name(code, ci);
        Ok(())
    }

    /// `addClsName`
    pub fn add_cls_name(&self, code: &mut CodeWriter, ci: &ClassInfo) {
        let name = self.use_class_internal(self.class_info(), ci);
        code.add(&name);
    }

    /// `addClsShortNameForced`
    pub fn add_cls_short_name_forced(&self, code: &mut CodeWriter, ci: &ClassInfo) {
        code.add(ci.short_name());
        if !is_both_classes_in_one_top_class(self.class_info(), ci) {
            self.add_import(ci);
        }
    }

    fn use_class_internal(&self, use_cls: &ClassInfo, ext: &ClassInfo) -> String {
        let mut full_name = ext.full_name().to_string();
        if self.env.fallback || !self.env.use_imports {
            return full_name;
        }
        let short_name = ext.short_name();
        if use_cls == ext {
            return short_name.to_string();
        }
        if ext.package().is_empty() {
            // omit import for default package
            return short_name.to_string();
        }
        if is_class_inner_for(use_cls, ext) {
            return short_name.to_string();
        }
        if ext.is_inner() {
            return self.expand_inner_class_name(use_cls, ext);
        }
        if self.check_inner_collision(Some(use_cls), ext)
            || self.check_in_package_collision(use_cls, ext)
        {
            return full_name;
        }
        if is_both_classes_in_one_top_class(use_cls, ext) {
            return short_name.to_string();
        }
        // don't add import for top classes from 'java.lang' package (subpackages excluded)
        if ext.package() == "java.lang" && ext.parent_class().is_none() {
            return short_name.to_string();
        }
        if ext.package() == use_cls.package() {
            if !ext.is_inner() {
                // don't add import if this class from same package
                return short_name.to_string();
            }
            full_name = name_without_package(ext);
        }
        if self.has_import_with_short_name(ext, short_name) {
            return if let Some(parent) = ext.parent_class().filter(|_| ext.is_inner()) {
                format!("{}.{short_name}", self.use_class_internal(use_cls, parent))
            } else {
                full_name
            };
        }
        self.add_import(ext);
        short_name.to_string()
    }

    fn expand_inner_class_name(&self, use_cls: &ClassInfo, ext: &ClassInfo) -> String {
        let mut cls_list: Vec<&ClassInfo> = vec![ext];
        let mut parent = ext.parent_class();
        let mut add_import = true;
        while let Some(p) = parent {
            if p == use_cls || is_class_inner_for(use_cls, p) {
                if self.in_class_header.get() && p == use_cls {
                    cls_list.push(p);
                }
                add_import = false;
                break;
            }
            cls_list.push(p);
            parent = p.parent_class();
        }
        cls_list.reverse();
        if add_import {
            let top = cls_list[0];
            if top != ext {
                let used = self.use_class_internal(use_cls, top);
                if used != top.short_name() {
                    // short top name can't be used, use full name as fallback
                    return ext.full_name().to_string();
                }
            } else {
                self.add_import(top);
            }
        }
        cls_list
            .iter()
            .map(|c| c.short_name())
            .collect::<Vec<_>>()
            .join(".")
    }

    fn check_inner_collision(&self, use_cls: Option<&ClassInfo>, search: &ClassInfo) -> bool {
        let Some(use_cls) = use_cls else {
            return false;
        };
        let short = search.short_name();
        if use_cls.short_name() == short {
            return true;
        }
        if let Some(idx) = self.env.resolve_class_info(use_cls) {
            for &inner in &self.env.inner_classes[idx] {
                let ic = &self.env.root.classes[inner].info;
                if ic.short_name() == short && ic.full_name() != search.full_name() {
                    return true;
                }
            }
        }
        self.check_inner_collision(use_cls.parent_class(), search)
    }

    /// Check if a class with the same name exists in the current package.
    fn check_in_package_collision(&self, use_cls: &ClassInfo, search: &ClassInfo) -> bool {
        let current_pkg = use_cls.package();
        if current_pkg == search.package() {
            // search class already from current package
            return false;
        }
        self.env
            .root
            .clsp
            .is_cls_known(&format!("{current_pkg}.{}", search.short_name()))
    }
}

/// `ClassInfo.getAliasNameWithoutPackage`
pub fn name_without_package(ci: &ClassInfo) -> String {
    match ci.parent_class() {
        None => ci.short_name().to_string(),
        Some(p) => format!("{}.{}", name_without_package(p), ci.short_name()),
    }
}

fn top_parent(ci: &ClassInfo) -> Option<&ClassInfo> {
    let p = ci.parent_class()?;
    Some(top_parent(p).unwrap_or(p))
}

fn is_both_classes_in_one_top_class(use_cls: &ClassInfo, ext: &ClassInfo) -> bool {
    let a = top_parent(use_cls);
    let b = top_parent(ext);
    match a {
        Some(a) => Some(a) == b,
        // use class is a top class
        None => Some(use_cls) == b,
    }
}

fn is_class_inner_for(inner: &ClassInfo, parent: &ClassInfo) -> bool {
    match inner.parent_class() {
        Some(p) => p == parent || is_class_inner_for(p, parent),
        None => false,
    }
}
