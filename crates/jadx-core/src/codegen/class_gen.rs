//! Class level helpers of the code generator (`ClassGen`): type and class names, import
//! bookkeeping and the short/full name decision.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::code_writer::CodeWriter;
use super::name_gen::NameGen;
use super::string_utils::StringUtils;
use super::{CodegenError, CommentsLevel, DecompilationMode, MethodLoader};
use crate::model::cfg::MethodCfg;
use crate::model::info::ClassInfo;
use crate::model::node::Root;
use crate::model::types::{ArgType, PrimitiveType, WildcardBound};

/// Processed code of a method or the reason it could not be loaded.
type LoadedCfg = Rc<Result<MethodCfg, String>>;

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
    /// `JadxArgs.getCommentsLevel`
    pub comments_level: CommentsLevel,
    /// `JadxArgs.isShowInconsistentCode`
    pub show_inconsistent_code: bool,
    /// `JadxArgs.getDecompilationMode`
    pub decompilation_mode: DecompilationMode,
    /// `JadxArgs.getCodeNewLineStr`
    pub code_new_line_str: String,
    /// `JadxArgs.getCodeIndentStr`
    pub code_indent_str: String,
    /// Source of the processed code of the methods.
    pub loader: Option<&'a dyn MethodLoader>,
    cfgs: RefCell<HashMap<(usize, usize), LoadedCfg>>,
    errors: RefCell<Vec<String>>,
    warnings: RefCell<Vec<String>>,
    /// Classes whose body is currently being inlined at a singleton field read
    /// ([`inline_instance_field_constructor`](super::insn_gen::InsnGen::inline_instance_field_constructor)),
    /// guarding against an indirect cycle through several singleton fields.
    inlining: RefCell<HashSet<usize>>,
    /// Name decided for a variable's first declaration, keyed by the `(cls, mth, CodeVarId)` of
    /// the method whose code is being named (the anonymous class's own method when its body is
    /// inlined, not the enclosing class/method doing the inlining). The same anonymous class's
    /// constructor/method can be inlined at several call sites, each building its own
    /// [`NameGen`](super::name_gen::NameGen); without this, each site picks a name independently
    /// and the same `CodeVar` can end up printed under different names at different sites. Only
    /// a `NameGen`'s *first* `assign_arg` call for a given `CodeVarId` consults/updates this
    /// cache; a repeat call for the same `CodeVarId` within the same `NameGen` pass (duplicated
    /// bytecode redeclaring the same variable at several text positions in one method) must stay
    /// on the existing purely local dedup path instead.
    global_var_names: RefCell<HashMap<(usize, usize, crate::model::ssa::CodeVarId), String>>,
}

/// Removes its class from [`GenEnv::inlining`] when dropped, even if generation returns early.
pub struct InliningGuard<'a, 'e> {
    env: &'a GenEnv<'e>,
    cls: usize,
}

impl Drop for InliningGuard<'_, '_> {
    fn drop(&mut self) {
        self.env.inlining.borrow_mut().remove(&self.cls);
    }
}

impl<'a> GenEnv<'a> {
    /// Environment with the member classes and root packages computed by the caller.
    pub fn with_inner_classes(
        root: &'a Root,
        inner_classes: Vec<Vec<usize>>,
        root_pkgs: HashSet<String>,
    ) -> Self {
        GenEnv {
            inner_classes,
            root_pkgs,
            ..GenEnv::empty(root)
        }
    }

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
            inner_classes,
            root_pkgs,
            ..GenEnv::empty(root)
        }
    }

    fn empty(root: &'a Root) -> Self {
        GenEnv {
            root,
            inner_classes: Vec::new(),
            root_pkgs: HashSet::new(),
            string_utils: StringUtils::default(),
            use_imports: true,
            fallback: false,
            comments_level: CommentsLevel::Info,
            show_inconsistent_code: false,
            decompilation_mode: DecompilationMode::Auto,
            code_new_line_str: "\n".to_string(),
            code_indent_str: "    ".to_string(),
            loader: None,
            cfgs: RefCell::new(HashMap::new()),
            errors: RefCell::new(Vec::new()),
            warnings: RefCell::new(Vec::new()),
            inlining: RefCell::new(HashSet::new()),
            global_var_names: RefCell::new(HashMap::new()),
        }
    }

    /// Enters `cls` as currently being inlined, failing if it already is (a cycle through
    /// several singleton field reads); the returned guard leaves it again when dropped.
    pub fn enter_inlining(&self, cls: usize) -> Result<InliningGuard<'_, 'a>, String> {
        if !self.inlining.borrow_mut().insert(cls) {
            return Err(format!(
                "Anonymous inner class unlimited recursion detected. Convert class to inner: {}",
                self.root.classes[cls].info.full_name()
            ));
        }
        Ok(InliningGuard { env: self, cls })
    }

    /// Record an error of the code generation (reported with the class).
    pub fn add_error(&self, msg: String) {
        self.errors.borrow_mut().push(msg);
    }

    /// Record a warning of the code generation (reported with the class).
    pub fn add_warning(&self, msg: String) {
        self.warnings.borrow_mut().push(msg);
    }

    /// Warnings recorded so far.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.borrow().clone()
    }

    /// Errors recorded so far.
    pub fn errors(&self) -> Vec<String> {
        self.errors.borrow().clone()
    }

    /// `checkCommentsLevel`
    pub fn check_comments_level(&self, required: CommentsLevel) -> bool {
        required.filter(self.comments_level)
    }

    /// Processed code of a method (loaded once).
    pub fn method_cfg(&self, cls: usize, mth: usize) -> LoadedCfg {
        if let Some(c) = self.cfgs.borrow().get(&(cls, mth)) {
            return c.clone();
        }
        let loaded = match self.loader {
            Some(l) => l.load(cls, mth),
            None => Err("method code is not available".to_string()),
        };
        let rc = Rc::new(loaded);
        self.cfgs.borrow_mut().insert((cls, mth), rc.clone());
        rc
    }

    /// Name already decided (at another inlining site) for this `(cls, mth)`'s `cv`, if any. See
    /// [`GenEnv::global_var_names`].
    pub fn global_var_name(
        &self,
        cls: usize,
        mth: usize,
        cv: crate::model::ssa::CodeVarId,
    ) -> Option<String> {
        self.global_var_names.borrow().get(&(cls, mth, cv)).cloned()
    }

    /// Records the name decided for this `(cls, mth)`'s `cv` at its first declaration in a
    /// `NameGen` pass, for reuse by a later pass over the same method (e.g. another inlining
    /// site of the same anonymous class). See [`GenEnv::global_var_names`].
    pub fn set_global_var_name(
        &self,
        cls: usize,
        mth: usize,
        cv: crate::model::ssa::CodeVarId,
        name: String,
    ) {
        self.global_var_names
            .borrow_mut()
            .entry((cls, mth, cv))
            .or_insert(name);
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

    /// Output short name of a class (`ClassInfo.getAliasShortName`).
    pub fn short_name(&self, ci: &ClassInfo) -> String {
        self.root.class_short_name(ci)
    }

    /// Output package of a class (`ClassInfo.getAliasPkg`).
    pub fn pkg(&self, ci: &ClassInfo) -> String {
        self.root.class_package(ci)
    }

    /// Output full name of a class (`ClassInfo.getAliasFullName`).
    pub fn full_name(&self, ci: &ClassInfo) -> String {
        self.root.class_full_name(ci)
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
    fallback: bool,
    use_imports: bool,
    /// Names used by the method that contains this (anonymous) class.
    outer_name_gen: RefCell<Option<NameGen>>,
    /// Length of the code at the start of the class body.
    pub(super) cls_decl_offset: Cell<usize>,
    /// The class body is written inside the code of another class (anonymous class).
    inlined: Cell<bool>,
    /// Display name for a `FieldReplace::Var` field of this (anonymous) class, by field index:
    /// the already-resolved, de-duplicated name of the captured variable as it is named at the
    /// single constructor-call site, not a name independently re-derived (and re-deduplicated)
    /// from the constructor's own, unrelated `CodeVarId` space. Filled once when the class body
    /// is inlined (`inline_anonymous_constructor`/`inline_instance_field_constructor`), read by
    /// every one of its methods afterwards (they all share this `ClassGen`).
    field_replace_names: RefCell<HashMap<usize, String>>,
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
            fallback: env.fallback,
            use_imports: env.use_imports,
            outer_name_gen: RefCell::new(None),
            cls_decl_offset: Cell::new(0),
            inlined: Cell::new(false),
            field_replace_names: RefCell::new(HashMap::new()),
        }
    }

    /// Generator of an inner class sharing the imports of its parent generator.
    pub fn with_parent(env: &'a GenEnv<'a>, cls: usize, parent: &'a ClassGen<'a>) -> Self {
        ClassGen {
            parent: Some(parent),
            fallback: parent.fallback,
            use_imports: parent.use_imports,
            ..ClassGen::new(env, cls)
        }
    }

    /// `MethodGen.getFallbackMethodGen`: generator without imports in fallback mode.
    pub fn new_fallback(env: &'a GenEnv<'a>, cls: usize) -> Self {
        ClassGen {
            fallback: true,
            use_imports: false,
            ..ClassGen::new(env, cls)
        }
    }

    pub fn set_inlined(&self, inlined: bool) {
        self.inlined.set(inlined);
    }

    pub fn is_inlined(&self) -> bool {
        self.inlined.get()
    }

    pub fn use_imports(&self) -> bool {
        self.use_imports
    }

    /// `getOuterNameGen`
    pub fn outer_name_gen(&self) -> Option<NameGen> {
        self.outer_name_gen.borrow().clone()
    }

    /// `setOuterNameGen`
    pub fn set_outer_name_gen(&self, ng: NameGen) {
        *self.outer_name_gen.borrow_mut() = Some(ng);
    }

    /// Names resolved for this class's `FieldReplace::Var` fields, by field index: see
    /// `field_replace_names`.
    pub fn set_field_replace_names(&self, names: HashMap<usize, String>) {
        *self.field_replace_names.borrow_mut() = names;
    }

    pub fn field_replace_name(&self, field_idx: usize) -> Option<String> {
        self.field_replace_names.borrow().get(&field_idx).cloned()
    }

    pub fn comments_level_info(&self) -> bool {
        self.env.check_comments_level(CommentsLevel::Info)
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
        self.fallback
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

    pub(super) fn add_import(&self, ci: &ClassInfo) {
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
                .any(|i| i != ext && self.env.short_name(i) == short),
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
        code.add(&self.env.short_name(&ci));
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
        code.add(&self.env.short_name(ci));
        if !is_both_classes_in_one_top_class(self.class_info(), ci) {
            self.add_import(ci);
        }
    }

    fn use_class_internal(&self, use_cls: &ClassInfo, ext: &ClassInfo) -> String {
        let env = self.env;
        let mut full_name = env.full_name(ext);
        if self.fallback || !self.use_imports {
            return full_name;
        }
        let short_name = env.short_name(ext);
        if use_cls == ext {
            return short_name;
        }
        if env.pkg(ext).is_empty() {
            // omit import for default package
            return short_name;
        }
        if is_class_inner_for(use_cls, ext) {
            return short_name;
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
            return short_name;
        }
        // don't add import for top classes from 'java.lang' package (subpackages excluded)
        if ext.package() == "java.lang" && ext.parent_class().is_none() {
            return short_name;
        }
        if env.pkg(ext) == env.pkg(use_cls) {
            if !ext.is_inner() {
                // don't add import if this class from same package
                return short_name;
            }
            full_name = env.root.class_name_without_package(ext);
        }
        if self.has_import_with_short_name(ext, &short_name) {
            return full_name;
        }
        self.add_import(ext);
        short_name
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
                if used != self.env.short_name(top) {
                    // short top name can't be used, use full name as fallback
                    return self.env.full_name(ext);
                }
            } else {
                self.add_import(top);
            }
        }
        cls_list
            .iter()
            .map(|c| self.env.short_name(c))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn check_inner_collision(&self, use_cls: Option<&ClassInfo>, search: &ClassInfo) -> bool {
        let Some(use_cls) = use_cls else {
            return false;
        };
        let short = self.env.short_name(search);
        if self.env.short_name(use_cls) == short {
            return true;
        }
        if let Some(idx) = self.env.resolve_class_info(use_cls) {
            for &inner in &self.env.inner_classes[idx] {
                let ic = &self.env.root.classes[inner].info;
                if ic.short_name() == short && ic.full_name() != self.env.full_name(search) {
                    return true;
                }
            }
        }
        self.check_inner_collision(use_cls.parent_class(), search)
    }

    /// Check if a class with the same name exists in the current package.
    fn check_in_package_collision(&self, use_cls: &ClassInfo, search: &ClassInfo) -> bool {
        let current_pkg = self.env.pkg(use_cls);
        if current_pkg == self.env.pkg(search) {
            // search class already from current package
            return false;
        }
        self.env
            .root
            .clsp
            .is_cls_known(&format!("{current_pkg}.{}", self.env.short_name(search)))
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
