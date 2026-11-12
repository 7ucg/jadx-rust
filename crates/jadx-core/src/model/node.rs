//! Class, field and method nodes of the loaded input plus the root that indexes them.

use std::collections::HashMap;
use std::sync::Arc;

use jadx_input_api::ClassData;

use super::info::{ClassInfo, FieldInfo, MethodInfo};
use super::types::ArgType;
use crate::root::RootCtx;

pub const ACC_STATIC: u32 = 0x8;
pub const ACC_ENUM: u32 = 0x4000;

#[derive(Debug, Clone)]
pub struct FieldNode {
    pub info: FieldInfo,
    pub access: u32,
    /// Current type (declared type, replaced by the generic type from the signature).
    pub ty: ArgType,
    pub signature: Option<String>,
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
}

impl ClassNode {
    pub fn is_inner(&self) -> bool {
        self.info.is_inner()
    }
}

/// All classes of the input.
#[derive(Debug)]
pub struct Root {
    pub ctx: Arc<RootCtx>,
    pub classes: Vec<ClassNode>,
    pub clsp: crate::clsp::ClspGraph,
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
            });
        }
        let mut root = Root {
            ctx,
            classes,
            clsp: crate::clsp::ClspGraph::default(),
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
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); self.classes.len()];
        for (i, c) in self.classes.iter().enumerate() {
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
        for (i, c) in self.classes.iter().enumerate() {
            if !c.is_inner() {
                visit(i, &children, &mut out);
            }
        }
        out
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
        let idx = self.resolve_class(field.decl_class.arg_type())?;
        let cls = &self.classes[idx];
        if let Some(f) = cls.fields.iter().find(|f| f.info == *field) {
            return Some(f);
        }
        self.deep_resolve_field(idx, field)
    }

    fn deep_resolve_field(&self, idx: usize, field: &FieldInfo) -> Option<&FieldNode> {
        let cls = &self.classes[idx];
        if let Some(f) = cls
            .fields
            .iter()
            .find(|f| f.info.name == field.name && f.info.ty == field.ty)
        {
            return Some(f);
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
