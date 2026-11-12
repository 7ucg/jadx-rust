//! `UsageInfoVisitor`: scan all classes and methods and collect usage info and class
//! dependencies (`useIn`, `used`, `dependencies`, ...).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use jadx_input_api::{AnnotationRef, CodeRef, CodeRefProvider, FieldRefData, MethodRefData};

use crate::model::info::{FieldInfo, MethodInfo};
use crate::model::node::{MethodId, Root};
use crate::model::types::ArgType;

type FieldId = (usize, usize);

/// Map of nodes to the set of nodes using them (a node never uses itself).
struct UseSet<K, V>(HashMap<K, HashSet<V>>);

impl<K: Hash + Eq, V: Hash + Eq> UseSet<K, V> {
    fn new() -> Self {
        UseSet(HashMap::new())
    }

    fn add(&mut self, key: K, use_by: V) {
        self.0.entry(key).or_default().insert(use_by);
    }
}

impl<K: Hash + Eq + Copy> UseSet<K, K> {
    fn add_other(&mut self, key: K, use_by: K) {
        if key != use_by {
            self.add(key, use_by);
        }
    }
}

/// Sort keys: classes by raw name, methods by declaring class and short id, both compared by
/// UTF-16 code units like `String.compareTo`.
struct SortKeys {
    cls: Vec<Vec<u16>>,
    mth: Vec<Vec<Vec<u16>>>,
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl SortKeys {
    fn new(root: &Root) -> Self {
        SortKeys {
            cls: root
                .classes
                .iter()
                .map(|c| utf16(&c.info.raw_name()))
                .collect(),
            mth: root
                .classes
                .iter()
                .map(|c| {
                    c.methods
                        .iter()
                        .map(|m| utf16(&m.info.short_id()))
                        .collect()
                })
                .collect(),
        }
    }

    fn cmp_cls(&self, a: usize, b: usize) -> Ordering {
        self.cls[a].cmp(&self.cls[b])
    }

    fn cmp_mth(&self, a: MethodId, b: MethodId) -> Ordering {
        self.cls[a.0]
            .cmp(&self.cls[b.0])
            .then_with(|| self.mth[a.0][a.1].cmp(&self.mth[b.0][b.1]))
    }

    fn sorted_cls(&self, set: Option<&HashSet<usize>>) -> Vec<usize> {
        let mut v: Vec<usize> = set.map(|s| s.iter().copied().collect()).unwrap_or_default();
        v.sort_by(|a, b| self.cmp_cls(*a, *b));
        v
    }

    fn sorted_mth(&self, set: Option<&HashSet<MethodId>>) -> Vec<MethodId> {
        let mut v: Vec<MethodId> = set.map(|s| s.iter().copied().collect()).unwrap_or_default();
        v.sort_by(|a, b| self.cmp_mth(*a, *b));
        v
    }
}

fn cmp_method_info(a: &MethodInfo, b: &MethodInfo) -> Ordering {
    utf16(&a.decl_class.raw_name())
        .cmp(&utf16(&b.decl_class.raw_name()))
        .then_with(|| utf16(&a.short_id()).cmp(&utf16(&b.short_id())))
}

fn field_info(root: &Root, f: &FieldRefData) -> Option<FieldInfo> {
    let decl = root
        .ctx
        .class_info(&ArgType::parse(&f.parent_class).ok()?)
        .ok()?;
    Some(FieldInfo {
        decl_class: decl,
        name: f.name.clone(),
        ty: ArgType::parse(&f.ty).ok()?,
    })
}

fn method_info(root: &Root, m: &MethodRefData) -> Option<MethodInfo> {
    let decl = root
        .ctx
        .class_info(&ArgType::parse(&m.parent_class).ok()?)
        .ok()?;
    let mut args = Vec::with_capacity(m.arg_types.len());
    for a in &m.arg_types {
        args.push(ArgType::parse(a).ok()?);
    }
    Some(MethodInfo {
        decl_class: decl,
        name: m.name.clone(),
        arg_types: args,
        return_type: ArgType::parse(&m.return_type).ok()?,
    })
}

/// Classes of the input found in the type and in its generic arguments, bounds and wildcard.
fn type_classes(root: &Root, ty: &ArgType, out: &mut Vec<usize>) {
    if *ty == ArgType::object_type() {
        return;
    }
    if ty.is_array() {
        type_classes(root, ty.array_root_element(), out);
        return;
    }
    if ty.is_object() {
        if let Some(c) = root.resolve_class(ty) {
            out.push(c);
        }
        for g in ty.generic_types().unwrap_or_default() {
            type_classes(root, g, out);
        }
        for e in ty.extend_types() {
            type_classes(root, e, out);
        }
        if let Some(w) = ty.wildcard_type() {
            type_classes(root, w, out);
        }
    }
}

struct UsageInfo<'a> {
    root: &'a Root,
    cls_deps: UseSet<usize, usize>,
    cls_usage: UseSet<usize, usize>,
    cls_use_in_mth: UseSet<usize, MethodId>,
    field_usage: UseSet<FieldId, MethodId>,
    mth_usage: UseSet<MethodId, MethodId>,
    mth_uses: UseSet<MethodId, MethodId>,
    unresolved_mth_usage: HashMap<MethodId, HashSet<MethodInfo>>,
    self_calls: HashSet<MethodId>,
}

impl<'a> UsageInfo<'a> {
    fn new(root: &'a Root) -> Self {
        UsageInfo {
            root,
            cls_deps: UseSet::new(),
            cls_usage: UseSet::new(),
            cls_use_in_mth: UseSet::new(),
            field_usage: UseSet::new(),
            mth_usage: UseSet::new(),
            mth_uses: UseSet::new(),
            unresolved_mth_usage: HashMap::new(),
            self_calls: HashSet::new(),
        }
    }

    fn type_classes(&self, ty: &ArgType) -> Vec<usize> {
        let mut v = Vec::new();
        type_classes(self.root, ty, &mut v);
        v
    }

    /// Class `cls` uses the classes of type `ty`.
    fn cls_use_type(&mut self, cls: usize, ty: &ArgType) {
        for dep in self.type_classes(ty) {
            self.cls_use(cls, dep);
        }
    }

    fn mth_use_type(&mut self, mth: MethodId, ty: &ArgType) {
        for dep in self.type_classes(ty) {
            self.mth_use_cls(mth, dep);
        }
    }

    fn mth_use_cls(&mut self, mth: MethodId, use_cls: usize) {
        let parent = mth.0;
        self.cls_use(parent, use_cls);
        if parent != use_cls {
            // exclude class usage in self methods
            self.cls_use_in_mth.add(use_cls, mth);
        }
    }

    fn cls_use(&mut self, cls: usize, dep_cls: usize) {
        let top = self.root.top_parent_index(cls);
        let dep_top = self.root.top_parent_index(dep_cls);
        self.cls_deps.add_other(top, dep_top);
        self.cls_usage.add_other(dep_cls, cls);
        self.cls_usage.add_other(dep_cls, top);
    }

    fn method_use(&mut self, mth: MethodId, use_mth: MethodId) {
        self.mth_use_cls(mth, use_mth.0);
        self.mth_usage.add_other(use_mth, mth);
        self.mth_uses.add_other(mth, use_mth);
        if mth == use_mth {
            self.self_calls.insert(mth);
        }
        // implicit usage
        let m = &self.root.classes[use_mth.0].methods[use_mth.1];
        self.mth_use_type(mth, &m.return_type);
        for a in &m.info.arg_types {
            self.mth_use_type(mth, a);
        }
    }

    fn unresolved_method_use(&mut self, mth: MethodId, use_mth: MethodInfo) {
        if use_mth.decl_class.raw_name() == "java.lang.Object" && use_mth.short_id() == "<init>()V"
        {
            // ignore default object constructor (called in every constructor)
            return;
        }
        self.unresolved_mth_usage
            .entry(mth)
            .or_default()
            .insert(use_mth);
    }

    fn field_use(&mut self, mth: MethodId, fld: FieldId) {
        self.mth_use_cls(mth, fld.0);
        self.field_usage.add(fld, mth);
        // implicit usage
        let ty = &self.root.classes[fld.0].fields[fld.1].ty;
        self.mth_use_type(mth, ty);
    }

    fn process_class(&mut self, refs: &dyn CodeRefProvider, c: usize) {
        let root = self.root;
        let cls = &root.classes[c];
        if let Some(s) = &cls.super_class {
            self.cls_use_type(c, s);
        }
        for i in &cls.interfaces {
            self.cls_use_type(c, i);
        }
        for g in &cls.generics {
            self.cls_use_type(c, g);
        }
        for f in &cls.fields {
            self.cls_use_type(c, &f.ty);
            for r in &f.annotation_refs {
                // field refs in field annotations are not tracked
                if let AnnotationRef::Class(desc) = r {
                    self.cls_use_desc(c, desc);
                }
            }
        }
        for r in &cls.annotation_refs {
            match r {
                AnnotationRef::Class(desc) => self.cls_use_desc(c, desc),
                AnnotationRef::Field(f) => {
                    if let Some((fc, _)) =
                        field_info(root, f).and_then(|i| root.resolve_field_id(&i))
                    {
                        self.cls_use(c, fc);
                    }
                }
            }
        }
        for m in 0..cls.methods.len() {
            self.process_method(refs, (c, m));
        }
    }

    fn cls_use_desc(&mut self, cls: usize, desc: &str) {
        if let Ok(ty) = ArgType::parse(desc) {
            self.cls_use_type(cls, &ty);
        }
    }

    fn process_method(&mut self, refs: &dyn CodeRefProvider, id: MethodId) {
        let root = self.root;
        let mth = &root.classes[id.0].methods[id.1];
        for r in &mth.annotation_refs {
            match r {
                AnnotationRef::Class(desc) => {
                    if let Ok(ty) = ArgType::parse(desc) {
                        self.mth_use_type(id, &ty);
                    }
                }
                AnnotationRef::Field(f) => {
                    if let Some(fld) = field_info(root, f).and_then(|i| root.resolve_field_id(&i)) {
                        self.field_use(id, fld);
                    }
                }
            }
        }
        self.mth_use_type(id, &mth.return_type);
        for a in &mth.arg_types {
            self.mth_use_type(id, a);
        }
        let Some(code_off) = mth.code_offset else {
            return;
        };
        // an unreadable method is skipped
        let Ok(code_refs) = refs.code_refs(id.0, code_off) else {
            return;
        };
        for r in code_refs {
            self.process_code_ref(id, r);
        }
    }

    fn process_code_ref(&mut self, id: MethodId, r: CodeRef) {
        let root = self.root;
        match r {
            CodeRef::Type(desc) => {
                if let Ok(ty) = ArgType::parse(&desc) {
                    self.mth_use_type(id, &ty);
                }
            }
            CodeRef::Field(f) => {
                if let Some(fld) = field_info(root, &f).and_then(|i| root.resolve_field_id(&i)) {
                    self.field_use(id, fld);
                }
            }
            CodeRef::Method(m) => {
                let Some(info) = method_info(root, &m) else {
                    return;
                };
                match root.resolve_method(&info) {
                    Some(target) => self.method_use(id, target),
                    None => self.unresolved_method_use(id, info),
                }
            }
        }
    }
}

/// Collect usage info of all classes and store it in the nodes.
pub fn collect_usage(root: &mut Root, refs: &dyn CodeRefProvider) {
    let keys = SortKeys::new(root);
    let mut usage = UsageInfo::new(root);
    for c in 0..root.classes.len() {
        usage.process_class(refs, c);
    }
    let UsageInfo {
        cls_deps,
        cls_usage,
        cls_use_in_mth,
        field_usage,
        mth_usage,
        mth_uses,
        mut unresolved_mth_usage,
        self_calls,
        ..
    } = usage;
    for (c, cls) in root.classes.iter_mut().enumerate() {
        cls.dependencies = keys.sorted_cls(cls_deps.0.get(&c));
        cls.use_in = keys.sorted_cls(cls_usage.0.get(&c));
        cls.use_in_mth = keys.sorted_mth(cls_use_in_mth.0.get(&c));
        for (f, fld) in cls.fields.iter_mut().enumerate() {
            fld.use_in = keys.sorted_mth(field_usage.0.get(&(c, f)));
        }
        for (m, mth) in cls.methods.iter_mut().enumerate() {
            mth.use_in = keys.sorted_mth(mth_usage.0.get(&(c, m)));
            mth.used = keys.sorted_mth(mth_uses.0.get(&(c, m)));
            let mut unresolved: Vec<MethodInfo> = unresolved_mth_usage
                .remove(&(c, m))
                .map(|s| s.into_iter().collect())
                .unwrap_or_default();
            unresolved.sort_by(cmp_method_info);
            mth.unresolved_used = unresolved;
            mth.calls_self = self_calls.contains(&(c, m));
        }
    }
}
