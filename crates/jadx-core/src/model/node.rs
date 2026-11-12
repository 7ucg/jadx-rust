//! Class, field and method nodes of the loaded input plus the root that indexes them.

use std::collections::HashMap;
use std::sync::Arc;

use jadx_input_api::{
    AnnotationRef, ClassAttrs, ClassData, ConstValue, FieldAttrs, InnerClassData, MethodAttrs,
};

use super::const_storage::ConstStorage;
use super::info::{ClassInfo, FieldInfo, MethodInfo};
use super::types::ArgType;
use crate::root::RootCtx;

pub const ACC_PUBLIC: u32 = 0x1;
pub const ACC_STATIC: u32 = 0x8;
pub const ACC_FINAL: u32 = 0x10;
pub const ACC_SYNTHETIC: u32 = 0x1000;
pub const ACC_ENUM: u32 = 0x4000;
pub const ACC_CONSTRUCTOR: u32 = 0x10000;

/// Method of a class: `(class index, method index)` into [`Root::classes`].
pub type MethodId = (usize, usize);

#[derive(Debug, Clone)]
pub struct FieldNode {
    pub info: FieldInfo,
    pub access: u32,
    /// Current type (declared type, replaced by the generic type from the signature).
    pub ty: ArgType,
    pub signature: Option<String>,
    /// Initial value of a static field.
    pub const_value: Option<ConstValue>,
    pub annotation_refs: Vec<AnnotationRef>,
    /// Methods using this field (sorted by method info), set by the usage scan.
    pub use_in: Vec<MethodId>,
    /// Marked by the anonymous class processing: the instance field holding the single
    /// instance of an inlinable class.
    pub inline_instance_field: bool,
    /// Name used in the output when it differs from the name in the bytecode.
    pub alias: Option<String>,
    /// Annotations of the input.
    pub attrs: FieldAttrs,
    /// `AFlag.DONT_GENERATE`: the field is not part of the output.
    pub dont_generate: bool,
    /// `AType.FIELD_REPLACE`
    pub replace: Option<FieldReplace>,
    /// Why the field was renamed (`RenameReasonAttr`).
    pub alias_reason: Option<String>,
    /// `AType.FIELD_INIT_INSN`: instruction that initializes the field.
    pub init_insn: Option<FieldInitInsn>,
    /// Explicit zero initialization (`EncodedValue.NULL` constant value): the bytecode can omit
    /// the initialization of static final fields to 0 (`ClassNode.initStaticValues`). Removed
    /// when the field initialization is found in the class initializer.
    pub zero_init: bool,
}

/// `FieldInitInsnAttr`: the initializer instruction lives in the code state of method `mth` of
/// the class that declares the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldInitInsn {
    pub mth: usize,
    pub insn: usize,
}

/// `FieldReplaceAttr`: what replaces the use of a field in the code.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldReplace {
    /// The argument `arg` of the constructor `ctor` (method index) of the same class.
    Var { ctor: usize, arg: usize },
    /// The instance of a class.
    ClassInstance(ClassInfo),
}

#[derive(Debug, Clone)]
pub struct MethodNode {
    pub info: MethodInfo,
    pub access: u32,
    pub arg_types: Vec<ArgType>,
    pub return_type: ArgType,
    pub type_params: Vec<ArgType>,
    pub signature: Option<String>,
    pub code_offset: Option<u32>,
    /// Declaring class type of the single base method this method overrides
    /// (`MethodOverrideAttr`, used by `getMethodOriginDeclClass`).
    pub override_base_decl: Option<ArgType>,
    /// `AType.BRIDGED_BY`: index (same class) of the method this one collides with after the
    /// argument types of the other were fixed.
    pub bridged_by: Option<usize>,
    pub annotation_refs: Vec<AnnotationRef>,
    /// Methods calling this method (sorted by method info), set by the usage scan.
    pub use_in: Vec<MethodId>,
    /// Resolved methods called from this method (sorted by method info).
    pub used: Vec<MethodId>,
    /// Called methods that are not part of the input (sorted by method info).
    pub unresolved_used: Vec<MethodInfo>,
    pub calls_self: bool,
    /// Constructor of an anonymous class that is inlined at its single use.
    pub anonymous_constructor: bool,
    /// `AFlag.NO_SKIP_ARGS`: the anonymous constructor has no arguments to skip.
    pub no_skip_args: bool,
    /// `AType.SKIP_MTH_ARGS`: arguments not generated for the method (by argument index).
    pub skip_args: Option<Vec<bool>>,
    /// Synthetic method (or constructor) that may be inlined at its usage.
    pub inline_candidate: bool,
    /// `AFlag.SKIP_FIRST_ARG`: the first argument (outer instance) is not generated.
    pub skip_first_arg: bool,
    /// `AFlag.DONT_GENERATE`: the method is not part of the output.
    pub dont_generate: bool,
    /// Name used in the output when it differs from the name in the bytecode.
    pub alias: Option<String>,
    /// `AType.METHOD_OVERRIDE`: index into [`Root::override_sets`] of the set of all methods of
    /// the override hierarchy (this method included).
    pub override_set: Option<usize>,
    /// Declaring classes of all methods overridden by this method (`getOverrideList`).
    pub override_decls: Vec<ArgType>,
    /// The method is one of the base methods of its override hierarchy (`getBaseMethods`).
    pub override_is_base: bool,
    /// `AFlag.DONT_RENAME`
    pub dont_rename: bool,
    /// Annotations, declared exceptions and defaults of the input.
    pub attrs: MethodAttrs,
    /// Why the method was renamed (`RenameReasonAttr`).
    pub alias_reason: Option<String>,
    /// `AType.METHOD_REPLACE`: the method this method is merged with.
    pub replace: Option<MethodId>,
    /// First method of the override list (`getOverrideList().get(0)`).
    pub override_first: Option<OverrideFirst>,
    /// `AType.METHOD_THROWS`: exceptions the method throws (set by `MethodThrowsVisitor`).
    pub throws_attr: Option<ThrowsAttr>,
    /// `JadxCommentsAttr`: comments added by the passes, by level.
    pub comments: Vec<(crate::codegen::CommentsLevel, String)>,
    /// `AType.METHOD_INLINE`: how calls of the method are inlined (set by `MarkMethodsForInline`).
    pub inline: Option<InlineAttr>,
}

/// `MethodThrowsAttr`: a `HashSet<String>` of the thrown exceptions, kept in insertion order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThrowsAttr {
    items: Vec<String>,
    /// Largest size of the set (the table of the reference never shrinks).
    max_size: usize,
    pub visited: bool,
}

impl ThrowsAttr {
    pub fn add(&mut self, name: &str) {
        if !self.items.iter().any(|i| i == name) {
            self.items.push(name.to_string());
            self.max_size = self.max_size.max(self.items.len());
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.items.iter().any(|i| i == name)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn retain(&mut self, f: impl Fn(&str) -> bool) {
        self.items.retain(|i| f(i));
    }

    /// Elements in insertion order.
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Elements in the iteration order of the `HashSet` of the reference.
    pub fn java_order(&self) -> Vec<String> {
        let mut cap = 16usize;
        while self.max_size > cap * 3 / 4 {
            cap *= 2;
        }
        let bucket = |s: &String| {
            let h = super::bitset::java_string_hash(s) as u32;
            ((h ^ (h >> 16)) as usize) & (cap - 1)
        };
        let mut v = self.items.clone();
        v.sort_by_key(bucket);
        v
    }
}

/// A method of the override list: a method of the input or the raw access flags of a framework
/// method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideFirst {
    App(MethodId),
    Clsp(u32),
}

/// `MethodInlineAttr`: the instruction that replaces a call of the method, stored with its own
/// instruction arena (the argument registers are not bound to variables).
#[derive(Debug, Clone, PartialEq)]
pub struct InlineAttr {
    /// Instructions of the inlined code (wrapped instructions refer to indexes of this list).
    pub insns: Vec<super::insn::InsnNode>,
    /// The instruction to inline, `None` if the method must not be inlined.
    pub root: Option<super::insn::InsnId>,
    /// Register numbers of the method arguments (`this` first).
    pub args_regs: Vec<u32>,
    /// Register count of the method code (`getRegsCount`).
    pub regs_count: usize,
}

impl InlineAttr {
    pub fn not_needed() -> InlineAttr {
        InlineAttr {
            insns: Vec::new(),
            root: None,
            args_regs: Vec::new(),
            regs_count: 0,
        }
    }

    pub fn is_not_needed(&self) -> bool {
        self.root.is_none()
    }
}

impl MethodNode {
    /// Constructor: `<init>` carrying the constructor access flag.
    pub fn is_constructor(&self) -> bool {
        self.access & ACC_CONSTRUCTOR != 0 && self.info.is_constructor()
    }
}

/// How an anonymous class is inlined into its outer class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineType {
    Constructor,
    InstanceField,
}

impl InlineType {
    /// Name of the reference enum constant.
    pub fn name(self) -> &'static str {
        match self {
            InlineType::Constructor => "CONSTRUCTOR",
            InlineType::InstanceField => "INSTANCE_FIELD",
        }
    }
}

/// `AnonymousClassAttr`.
#[derive(Debug, Clone, PartialEq)]
pub struct AnonymousClassAttr {
    pub outer_class: usize,
    pub base_type: ArgType,
    pub inline_type: InlineType,
}

#[derive(Debug, Clone)]
pub struct ClassNode {
    pub info: ClassInfo,
    pub access: u32,
    pub super_class: Option<ArgType>,
    pub interfaces: Vec<ArgType>,
    pub generics: Vec<ArgType>,
    pub signature: Option<String>,
    pub fields: Vec<FieldNode>,
    pub methods: Vec<MethodNode>,
    /// `InnerClass` annotation data of the input.
    pub inner_class: Option<InnerClassData>,
    pub annotation_refs: Vec<AnnotationRef>,
    /// Top level classes used by this class (only set for top level classes).
    pub dependencies: Vec<usize>,
    /// Top level classes needed for code generation.
    pub codegen_deps: Vec<usize>,
    /// Classes using this class.
    pub use_in: Vec<usize>,
    /// Methods using this class (definition excluded).
    pub use_in_mth: Vec<MethodId>,
    /// `JadxCommentsAttr`: comments added by the passes, by level.
    pub comments: Vec<(crate::codegen::CommentsLevel, String)>,
    pub anonymous: Option<AnonymousClassAttr>,
    /// Anonymous classes inlined into this class.
    pub inlined_classes: Vec<usize>,
    /// Class this class is inlined into (`InlinedAttr`).
    pub inlined_into: Option<usize>,
    /// Class is not generated as separate class.
    pub dont_generate: bool,
    /// Display order of the fields (indexes into `fields`) when it differs from the order of the
    /// input (`ExtractFieldInit`).
    pub fields_order: Option<Vec<usize>>,
    /// Short name used in the output when it differs from the name in the bytecode.
    pub alias: Option<String>,
    /// Source file, input file and annotations of the input.
    pub attrs: ClassAttrs,
    /// `AType.ENUM_CLASS`
    pub enum_class: Option<EnumClassAttr>,
    /// `AFlag.REMOVE_SUPER_CLASS`
    pub remove_super_class: bool,
    /// `AFlag.PACKAGE_INFO`: the class is a `package-info` holder.
    pub package_info: bool,
    /// Why the class was renamed (`RenameReasonAttr`).
    pub alias_reason: Option<String>,
}

/// `EnumClassAttr.EnumField`: an enum constant with the constructor call that creates it. The
/// constructor instruction lives in the code of the static initializer (`ctr_insn` is an
/// instruction id in the code state of method `static_method`).
#[derive(Debug, Clone, PartialEq)]
pub struct EnumFieldAttr {
    /// Field index in the class.
    pub field: usize,
    pub ctr_insn: usize,
    /// Class of the constant body (`getCls`), if any.
    pub cls: Option<usize>,
}

/// `EnumClassAttr`
#[derive(Debug, Clone, PartialEq)]
pub struct EnumClassAttr {
    pub fields: Vec<EnumFieldAttr>,
    /// Index of the static initializer method holding the constructor instructions.
    pub static_method: usize,
}

impl ClassNode {
    pub fn is_inner(&self) -> bool {
        self.info.is_inner()
    }

    pub fn is_anonymous(&self) -> bool {
        self.anonymous.is_some()
    }

    pub fn is_enum(&self) -> bool {
        self.access & ACC_ENUM != 0
            && self
                .super_class
                .as_ref()
                .and_then(|s| s.object_name())
                .is_some_and(|n| n == super::types::CLASS_ENUM)
    }
}

/// All classes of the input.
#[derive(Debug, Clone)]
pub struct Root {
    pub ctx: Arc<RootCtx>,
    pub classes: Vec<ClassNode>,
    pub clsp: crate::clsp::ClspGraph,
    pub const_storage: ConstStorage,
    by_raw: HashMap<String, usize>,
    /// Sets of related methods of override hierarchies (`MethodOverrideAttr.getRelatedMthNodes`).
    pub override_sets: Vec<Vec<MethodId>>,
    /// Renamed packages: full package name to the new name of its last segment.
    pub pkg_aliases: HashMap<String, String>,
    /// Shadowed fields per class index (`ShadowFieldVisitor.init`), computed on first use.
    pub inner_cache: std::sync::OnceLock<Vec<Vec<usize>>>,
    pub shadow_fields: std::sync::OnceLock<HashMap<usize, HashMap<FieldInfo, ShadowFix>>>,
    /// Kotlin intrinsic methods that carry variable names (`ProcessKotlinInternals.init`).
    pub kotlin_name_methods: std::sync::OnceLock<Option<std::collections::HashSet<MethodInfo>>>,
    /// `EnumMapAttr`: ordinal to enum constant of the switch map fields, by map field
    /// (`FixSwitchOverEnum`).
    pub enum_maps: HashMap<(usize, usize), HashMap<i32, FieldInfo>>,
}

/// `ShadowFieldVisitor.FieldFixType`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowFix {
    Super,
    Cast,
}

fn parse(desc: &str) -> ArgType {
    ArgType::parse(desc).unwrap_or_else(|_| ArgType::object_type())
}

impl Root {
    pub fn build(data: Vec<ClassData>, move_inner_classes: bool) -> Result<Root, String> {
        let mut ctx = RootCtx::new(move_inner_classes);
        for c in &data {
            ctx.add_class(&c.type_desc);
            for m in &c.methods {
                ctx.add_method_name(&c.type_desc, &m.name);
            }
        }
        let ctx = Arc::new(ctx);
        let mut classes = Vec::with_capacity(data.len());
        let mut by_raw = HashMap::new();
        for c in data {
            let info = ctx.class_info(&ArgType::object(&c.type_desc))?;
            let mut fields = Vec::with_capacity(c.fields.len());
            for f in c.fields {
                let ty = ArgType::parse(&f.ty).map_err(|e| e.to_string())?;
                fields.push(FieldNode {
                    info: FieldInfo {
                        decl_class: info.clone(),
                        name: f.name,
                        ty: ty.clone(),
                    },
                    access: f.access_flags,
                    ty,
                    signature: f.signature,
                    const_value: f.const_value,
                    annotation_refs: f.annotation_refs,
                    use_in: Vec::new(),
                    inline_instance_field: false,
                    alias: None,
                    attrs: f.attrs,
                    dont_generate: false,
                    replace: None,
                    alias_reason: None,
                    init_insn: None,
                    zero_init: false,
                });
            }
            let mut methods = Vec::with_capacity(c.methods.len());
            for m in c.methods {
                let args: Vec<ArgType> = m
                    .arg_types
                    .iter()
                    .map(|a| ArgType::parse(a).map_err(|e| e.to_string()))
                    .collect::<Result<_, _>>()?;
                let ret = ArgType::parse(&m.return_type).map_err(|e| e.to_string())?;
                methods.push(MethodNode {
                    info: MethodInfo {
                        decl_class: info.clone(),
                        name: m.name,
                        arg_types: args.clone(),
                        return_type: ret.clone(),
                    },
                    access: m.access_flags,
                    arg_types: args,
                    return_type: ret,
                    type_params: Vec::new(),
                    signature: m.signature,
                    code_offset: m.code_offset,
                    override_base_decl: None,
                    bridged_by: None,
                    annotation_refs: m.annotation_refs,
                    use_in: Vec::new(),
                    used: Vec::new(),
                    unresolved_used: Vec::new(),
                    calls_self: false,
                    anonymous_constructor: false,
                    no_skip_args: false,
                    skip_args: None,
                    inline_candidate: false,
                    skip_first_arg: false,
                    dont_generate: false,
                    alias: None,
                    override_set: None,
                    override_decls: Vec::new(),
                    override_is_base: false,
                    dont_rename: false,
                    attrs: m.attrs,
                    alias_reason: None,
                    replace: None,
                    inline: None,
                    throws_attr: None,
                    comments: Vec::new(),
                    override_first: None,
                });
            }
            by_raw.insert(info.raw_name(), classes.len());
            classes.push(ClassNode {
                access: c.access_flags,
                super_class: c.super_type.as_deref().map(parse),
                interfaces: c.interfaces.iter().map(|i| parse(i)).collect(),
                generics: Vec::new(),
                signature: c.signature,
                fields,
                methods,
                info,
                inner_class: c.inner_class,
                annotation_refs: c.annotation_refs,
                dependencies: Vec::new(),
                codegen_deps: Vec::new(),
                use_in: Vec::new(),
                use_in_mth: Vec::new(),
                comments: Vec::new(),
                anonymous: None,
                inlined_classes: Vec::new(),
                inlined_into: None,
                dont_generate: false,
                fields_order: None,
                alias: None,
                attrs: c.attrs,
                enum_class: None,
                remove_super_class: false,
                package_info: false,
                alias_reason: None,
            });
        }
        for c in &mut classes {
            for f in &mut c.fields {
                f.zero_init = f.access & ACC_STATIC != 0
                    && f.access & ACC_FINAL != 0
                    && f.const_value.is_none();
            }
            c.package_info = c.info.short_name() == "package-info"
                && c.fields.is_empty()
                && c.methods.is_empty();
        }
        let mut root = Root {
            ctx,
            classes,
            clsp: crate::clsp::ClspGraph::default(),
            const_storage: ConstStorage::new(true),
            by_raw,
            override_sets: Vec::new(),
            pkg_aliases: HashMap::new(),
            inner_cache: std::sync::OnceLock::new(),
            enum_maps: HashMap::new(),
            shadow_fields: std::sync::OnceLock::new(),
            kotlin_name_methods: std::sync::OnceLock::new(),
        };
        let mut clsp = crate::clsp::ClspGraph::load_core()?;
        clsp.add_app(&root);
        clsp.init_cache();
        root.clsp = clsp;
        Ok(root)
    }

    /// Class processing order of the pre-decompile passes: top level classes in input order,
    /// each followed by its inner classes (depth first).
    pub fn traversal_order(&self) -> Vec<usize> {
        // the reference sorts all classes by raw name before any pass runs
        let mut sorted: Vec<usize> = (0..self.classes.len()).collect();
        sorted.sort_by_cached_key(|i| self.classes[*i].info.raw_name());
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); self.classes.len()];
        for &i in &sorted {
            let c = &self.classes[i];
            if let Some(p) = self.parent_index_of(c) {
                if p != i {
                    children[p].push(i);
                }
            }
        }
        fn visit(i: usize, children: &[Vec<usize>], out: &mut Vec<usize>) {
            out.push(i);
            for &c in &children[i] {
                visit(c, children, out);
            }
        }
        let mut out = Vec::with_capacity(self.classes.len());
        for &i in &sorted {
            if !self.classes[i].is_inner() {
                visit(i, &children, &mut out);
            }
        }
        out
    }

    /// Class processing order of passes that run over `RootNode.getClasses()`: sorted by raw name.
    pub fn sorted_class_order(&self) -> Vec<usize> {
        let mut keys: Vec<(Vec<u16>, usize)> = self
            .classes
            .iter()
            .enumerate()
            .map(|(i, c)| (c.info.raw_name().encode_utf16().collect(), i))
            .collect();
        keys.sort();
        keys.into_iter().map(|(_, i)| i).collect()
    }

    /// Top level class of a class (the class itself if it is not an inner class).
    pub fn top_parent_index(&self, idx: usize) -> usize {
        let mut cur = idx;
        let mut steps = 0;
        while let Some(p) = self.parent_index(cur) {
            if p == cur || steps > self.classes.len() {
                break;
            }
            cur = p;
            steps += 1;
        }
        cur
    }

    pub fn class_index(&self, raw_name: &str) -> Option<usize> {
        self.by_raw.get(raw_name).copied()
    }

    /// Resolve a class by type (generic types are reduced to their raw class).
    pub fn resolve_class(&self, ty: &ArgType) -> Option<usize> {
        if !ty.is_type_known() || ty.is_generic_type() {
            return None;
        }
        if ty.wildcard_bound() == Some(super::types::WildcardBound::Unbound) {
            return None;
        }
        let name = ty.object_name()?;
        self.class_index(&name)
    }

    /// Declared field lookup: exact field, then by name and type in the class and its super classes.
    pub fn resolve_field(&self, field: &FieldInfo) -> Option<&FieldNode> {
        let (c, f) = self.resolve_field_id(field)?;
        Some(&self.classes[c].fields[f])
    }

    /// Like [`Root::resolve_field`], returning `(class index, field index)`.
    pub fn resolve_field_id(&self, field: &FieldInfo) -> Option<(usize, usize)> {
        let idx = self.resolve_class(field.decl_class.arg_type())?;
        let cls = &self.classes[idx];
        if let Some(f) = cls.fields.iter().position(|f| f.info == *field) {
            return Some((idx, f));
        }
        self.deep_resolve_field(idx, field)
    }

    fn deep_resolve_field(&self, idx: usize, field: &FieldInfo) -> Option<(usize, usize)> {
        let cls = &self.classes[idx];
        if let Some(f) = cls
            .fields
            .iter()
            .position(|f| f.info.name == field.name && f.info.ty == field.ty)
        {
            return Some((idx, f));
        }
        if let Some(sup) = &cls.super_class {
            if let Some(sidx) = self.resolve_class(sup) {
                if let Some(f) = self.deep_resolve_field(sidx, field) {
                    return Some(f);
                }
            }
        }
        for iface in &cls.interfaces {
            if let Some(iidx) = self.resolve_class(iface) {
                if let Some(f) = self.deep_resolve_field(iidx, field) {
                    return Some(f);
                }
            }
        }
        None
    }

    /// `RootNode.resolveMethod`: exact method, then a search through super types by signature.
    pub fn resolve_method(&self, m: &MethodInfo) -> Option<(usize, usize)> {
        let ci = self.class_index(&m.decl_class.raw_name())?;
        if let Some(mi) = self.classes[ci].methods.iter().position(|x| x.info == *m) {
            return Some((ci, mi));
        }
        let sig = super::info::make_short_id(&m.name, &m.arg_types, None);
        self.deep_resolve_method(ci, &sig)
    }

    fn deep_resolve_method(&self, ci: usize, sig: &str) -> Option<(usize, usize)> {
        let cls = &self.classes[ci];
        if let Some(mi) = cls
            .methods
            .iter()
            .position(|m| m.info.short_id().starts_with(sig))
        {
            return Some((ci, mi));
        }
        if let Some(sup) = &cls.super_class {
            if let Some(si) = self.resolve_class(sup) {
                if let Some(f) = self.deep_resolve_method(si, sig) {
                    return Some(f);
                }
            }
        }
        for iface in &cls.interfaces {
            if let Some(ii) = self.resolve_class(iface) {
                if let Some(f) = self.deep_resolve_method(ii, sig) {
                    return Some(f);
                }
            }
        }
        None
    }

    /// `ClassNode.getInnerClasses`: inner classes sorted by raw name.
    pub fn inner_classes(&self, idx: usize) -> &[usize] {
        let all = self.inner_cache.get_or_init(|| {
            let mut sorted: Vec<usize> = (0..self.classes.len()).collect();
            sorted.sort_by_cached_key(|i| self.classes[*i].info.raw_name());
            let mut children: Vec<Vec<usize>> = vec![Vec::new(); self.classes.len()];
            for &i in &sorted {
                if let Some(p) = self.parent_index_of(&self.classes[i]) {
                    if p != i {
                        children[p].push(i);
                    }
                }
            }
            children
        });
        &all[idx]
    }

    pub fn parent_index_of(&self, cls: &ClassNode) -> Option<usize> {
        let parent = cls.info.parent_class()?;
        self.class_index(&parent.raw_name())
    }

    /// Index of the (inner class) parent of a class, if any.
    pub fn parent_index(&self, idx: usize) -> Option<usize> {
        let parent = self.classes[idx].info.parent_class()?;
        self.class_index(&parent.raw_name())
    }
}
