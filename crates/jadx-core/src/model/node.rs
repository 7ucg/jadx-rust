//! Class, field and method nodes of the loaded input plus the root that indexes them.

use std::collections::HashMap;
use std::sync::Arc;

use jadx_input_api::{AnnotationRef, ClassData, ConstValue, InnerClassData};

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
    /// Synthetic method (or constructor) that may be inlined at its usage.
    pub inline_candidate: bool,
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
    pub anonymous: Option<AnonymousClassAttr>,
    /// Anonymous classes inlined into this class.
    pub inlined_classes: Vec<usize>,
    /// Class this class is inlined into (`InlinedAttr`).
    pub inlined_into: Option<usize>,
    /// Class is not generated as separate class.
    pub dont_generate: bool,
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
#[derive(Debug)]
pub struct Root {
    pub ctx: Arc<RootCtx>,
    pub classes: Vec<ClassNode>,
    pub clsp: crate::clsp::ClspGraph,
    pub const_storage: ConstStorage,
    by_raw: HashMap<String, usize>,
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
                    inline_candidate: false,
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
                anonymous: None,
                inlined_classes: Vec::new(),
                inlined_into: None,
                dont_generate: false,
            });
        }
        let mut root = Root {
            ctx,
            classes,
            clsp: crate::clsp::ClspGraph::default(),
            const_storage: ConstStorage::new(true),
            by_raw,
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

    /// Order of `DepthTraversal` over the sorted class list: top level classes by raw name, each
    /// followed by its inner classes (depth first).
    pub fn depth_traversal_order(&self) -> Vec<usize> {
        let sorted = self.sorted_class_order();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); self.classes.len()];
        for &i in &sorted {
            if let Some(p) = self.parent_index(i).filter(|p| *p != i) {
                children[p].push(i);
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
            if self.parent_index(i).is_none() {
                visit(i, &children, &mut out);
            }
        }
        out
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
