//! `OverrideMethodVisitor`: find overridden methods and revert type erasure of return and
//! argument types using the generic types of the base method.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::model::node::{OverrideFirst, Root};
use crate::model::types::ArgType;
use crate::type_compare::{TypeCompare, TypeCompareEnum};
use crate::type_utils::TypeUtils;

const ACC_PUBLIC: u32 = 0x1;
const ACC_PRIVATE: u32 = 0x2;
const ACC_PROTECTED: u32 = 0x4;
const ACC_STATIC: u32 = 0x8;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MthRef {
    App(usize, usize),
    /// Framework method: class name and short id.
    Clsp(String, String),
}

#[derive(Debug, Clone)]
struct OverrideAttr {
    override_list: Vec<MthRef>,
    base_methods: Vec<MthRef>,
    /// Index of the set of related methods (`getRelatedMthNodes`); the set is shared between
    /// the methods of one override hierarchy.
    related: usize,
}

/// State of the pass: override attributes and the sets of related methods.
#[derive(Default)]
struct Cx {
    attrs: HashMap<MthRef, OverrideAttr>,
    sets: Vec<BTreeSet<(usize, usize)>>,
    /// `AFlag.DONT_RENAME`
    dont_rename: HashSet<(usize, usize)>,
}

struct SuperTypes {
    types: Vec<ArgType>,
    end_types: HashSet<String>,
}

struct Details {
    ret: ArgType,
    args: Vec<ArgType>,
    decl_class: ArgType,
}

fn details(root: &Root, m: &MthRef) -> Option<Details> {
    match m {
        MthRef::App(c, i) => {
            let cls = &root.classes[*c];
            let mth = &cls.methods[*i];
            Some(Details {
                ret: mth.return_type.clone(),
                args: mth.arg_types.clone(),
                decl_class: cls.info.arg_type().clone(),
            })
        }
        MthRef::Clsp(cls, id) => {
            let m = root.clsp.cls_details(cls)?.method_by_short_id(id)?;
            Some(Details {
                ret: m.return_type.clone(),
                args: m.arg_types.clone(),
                decl_class: ArgType::object(cls),
            })
        }
    }
}

pub fn process_overrides(root: &mut Root) {
    let mut cx = Cx::default();
    for cidx in root.traversal_order() {
        let Some(sd) = collect_super_types(root, cidx) else {
            continue;
        };
        for midx in 0..root.classes[cidx].methods.len() {
            let upd = process_mth(root, &mut cx, cidx, midx, &sd);
            if let Some((ret, args)) = upd {
                let m = &mut root.classes[cidx].methods[midx];
                if let Some(r) = ret {
                    m.return_type = r;
                }
                if let Some(a) = args {
                    m.arg_types = a;
                }
                check_method_signature_collisions(root, &cx, cidx, midx);
            }
        }
    }
    if std::env::var("JADX_RS_OVR_DUMP").is_ok() {
        let name = |m: &MthRef| match m {
            MthRef::App(c, i) => format!(
                "{}.{}",
                root.classes[*c].info.raw_name(),
                root.classes[*c].methods[*i].info.short_id()
            ),
            MthRef::Clsp(c, id) => format!("{c}.{id}"),
        };
        let mut lines: Vec<String> = Vec::new();
        for (m, attr) in &cx.attrs {
            if let MthRef::App(..) = m {
                let mut b: Vec<String> = attr.base_methods.iter().map(&name).collect();
                b.sort();
                lines.push(format!("O {} base=[{}]", name(m), b.join(", ")));
            }
        }
        lines.sort();
        for l in lines {
            eprintln!("{l}");
        }
    }
    for (m, attr) in &cx.attrs {
        if let MthRef::App(c, i) = m {
            root.classes[*c].methods[*i].override_set = Some(attr.related);
            let decls: Vec<ArgType> = attr
                .override_list
                .iter()
                .filter_map(|o| details(root, o).map(|d| d.decl_class))
                .collect();
            let is_base = attr.base_methods.contains(m);
            let first = attr
                .override_list
                .iter()
                .find(|o| details(root, o).is_some())
                .and_then(|o| match o {
                    MthRef::App(c, i) => Some(OverrideFirst::App((*c, *i))),
                    MthRef::Clsp(cls, id) => root
                        .clsp
                        .cls_details(cls)
                        .and_then(|d| d.method_by_short_id(id))
                        .map(|m| OverrideFirst::Clsp(m.access_flags)),
                });
            let node = &mut root.classes[*c].methods[*i];
            node.override_first = first;
            node.override_decls = decls;
            node.override_is_base = is_base;
            if let [base] = attr.base_methods.as_slice() {
                if let Some(d) = details(root, base) {
                    root.classes[*c].methods[*i].override_base_decl = Some(d.decl_class);
                }
            }
        }
    }
    for (c, i) in &cx.dont_rename {
        root.classes[*c].methods[*i].dont_rename = true;
    }
    root.override_sets = cx
        .sets
        .iter()
        .map(|s| s.iter().copied().collect())
        .collect();
}

fn collect_super_types(root: &Root, cidx: usize) -> Option<SuperTypes> {
    let mut types: Vec<ArgType> = Vec::new();
    let mut end = HashSet::new();
    collect_cls(root, cidx, &mut types, &mut end);
    if types.is_empty() {
        return None;
    }
    if end.is_empty() {
        return None;
    }
    Some(SuperTypes {
        types,
        end_types: end,
    })
}

fn collect_cls(root: &Root, cidx: usize, types: &mut Vec<ArgType>, end: &mut HashSet<String>) {
    let cls = &root.classes[cidx];
    let mut k = 0;
    if let Some(sup) = &cls.super_class {
        k += add_super_type(root, types, end, sup);
    }
    for i in &cls.interfaces {
        k += add_super_type(root, types, end, i);
    }
    if k == 0 {
        end.insert(cls.info.arg_type().object_name().unwrap_or_default());
    }
}

fn add_super_type(
    root: &Root,
    types: &mut Vec<ArgType>,
    end: &mut HashSet<String>,
    st: &ArgType,
) -> i32 {
    if *st == ArgType::object_type() {
        return 0;
    }
    if types.contains(st) {
        return 0;
    }
    types.push(st.clone());
    if let Some(idx) = root.resolve_class(st) {
        collect_cls(root, idx, types, end);
        return 1;
    }
    let name = st.object_name().unwrap_or_default();
    if let Some(d) = root.clsp.cls_details(&name) {
        let mut k = 0;
        for p in d.parents().to_vec() {
            k += add_super_type(root, types, end, &p);
        }
        if k == 0 {
            end.insert(name);
        }
        return 1;
    }
    end.insert(name);
    1
}

type TypeUpdate = Option<(Option<ArgType>, Option<Vec<ArgType>>)>;

fn process_mth(root: &Root, cx: &mut Cx, cidx: usize, midx: usize, sd: &SuperTypes) -> TypeUpdate {
    let mth = &root.classes[cidx].methods[midx];
    if mth.info.is_constructor() || mth.access & (ACC_STATIC | ACC_PRIVATE) != 0 {
        return None;
    }
    let me = MthRef::App(cidx, midx);
    let attr = process_override_methods(root, cx, cidx, midx, sd)?;
    cx.attrs.insert(me, attr.clone());
    if attr.base_methods.len() != 1 {
        return None;
    }
    let base = details(root, &attr.base_methods[0])?;
    let ret = fix_return_type(root, cidx, midx, &base, sd);
    let args = fix_arg_types(root, cidx, midx, &base, sd);
    if ret.is_none() && args.is_none() {
        return None;
    }
    Some((ret, args))
}

fn process_override_methods(
    root: &Root,
    cx: &mut Cx,
    cidx: usize,
    midx: usize,
    sd: &SuperTypes,
) -> Option<OverrideAttr> {
    let me = MthRef::App(cidx, midx);
    if let Some(a) = cx.attrs.get(&me) {
        return Some(a.clone());
    }
    let cls = &root.classes[cidx];
    let mth = &cls.methods[midx];
    let signature = {
        let id = mth.info.short_id();
        // `name(args)` without the return type
        id[..id.find(')').map(|p| p + 1).unwrap_or(id.len())].to_string()
    };
    let mut override_list: Vec<MthRef> = Vec::new();
    let mut base_methods: Vec<MthRef> = Vec::new();
    for st in &sd.types {
        if let Some(sidx) = root.resolve_class(st) {
            if let Some(smidx) = search_overridden_method(root, sidx, cidx, midx, &signature) {
                if is_method_visible_in_cls(root, sidx, smidx, cidx) {
                    let sref = MthRef::App(sidx, smidx);
                    override_list.push(sref.clone());
                    if let Some(a) = cx.attrs.get(&sref).cloned() {
                        add_base_method(sd, &override_list, &mut base_methods, st);
                        return build_override_attr(
                            root,
                            cx,
                            cidx,
                            midx,
                            override_list,
                            base_methods,
                            Some(a),
                        );
                    }
                }
            }
        } else if let Some(d) = st.object_name().and_then(|n| root.clsp.cls_details(&n)) {
            let mut keys: Vec<(&String, &crate::clsp::ClspMethod)> = d.methods_map_iter().collect();
            keys.sort_by(|a, b| a.0.cmp(b.0));
            if let Some((k, _)) = keys.into_iter().find(|(k, _)| k.starts_with(&signature)) {
                override_list.push(MthRef::Clsp(d.name.clone(), k.clone()));
            }
        }
        add_base_method(sd, &override_list, &mut base_methods, st);
    }
    build_override_attr(root, cx, cidx, midx, override_list, base_methods, None)
}

fn add_base_method(
    sd: &SuperTypes,
    override_list: &[MthRef],
    base: &mut Vec<MthRef>,
    st: &ArgType,
) {
    if sd.end_types.contains(&st.object_name().unwrap_or_default()) {
        if let Some(last) = override_list.last() {
            if !base.contains(last) {
                base.push(last.clone());
            }
        }
    }
}

fn search_overridden_method(
    root: &Root,
    sidx: usize,
    cidx: usize,
    midx: usize,
    signature: &str,
) -> Option<usize> {
    let sup = &root.classes[sidx];
    let mth = &root.classes[cidx].methods[midx];
    let short_id = mth.info.short_id();
    for (i, m) in sup.methods.iter().enumerate() {
        if m.info.short_id() == short_id && m.access & ACC_STATIC == 0 {
            return Some(i);
        }
    }
    let tc = TypeCompare::new(&root.clsp);
    for (i, m) in sup.methods.iter().enumerate() {
        if m.info.short_id().starts_with(signature) && m.access & ACC_STATIC == 0 {
            let res = tc.compare_types(&m.info.return_type, &mth.info.return_type);
            if res.is_wider() {
                return Some(i);
            }
        }
    }
    None
}

fn is_method_visible_in_cls(root: &Root, sidx: usize, smidx: usize, cidx: usize) -> bool {
    let acc = root.classes[sidx].methods[smidx].access;
    if acc & ACC_PRIVATE != 0 {
        return false;
    }
    if acc & (ACC_PUBLIC | ACC_PROTECTED) != 0 {
        return true;
    }
    root.classes[sidx].info.package() == root.classes[cidx].info.package()
}

fn build_override_attr(
    root: &Root,
    cx: &mut Cx,
    cidx: usize,
    midx: usize,
    override_list: Vec<MthRef>,
    base_methods: Vec<MthRef>,
    attr: Option<OverrideAttr>,
) -> Option<OverrideAttr> {
    if override_list.is_empty() && attr.is_none() {
        return None;
    }
    match attr {
        None => {
            let clean = distinct(override_list);
            Some(apply_override_attr(
                root,
                cx,
                cidx,
                midx,
                clean,
                base_methods,
                false,
            ))
        }
        Some(a) => {
            let mut merged = override_list;
            merged.extend(a.override_list.iter().cloned());
            let clean = distinct(merged);
            let mut merged_base = base_methods;
            for b in &a.base_methods {
                if !merged_base.contains(b) {
                    merged_base.push(b.clone());
                }
            }
            Some(apply_override_attr(
                root,
                cx,
                cidx,
                midx,
                clean,
                merged_base,
                true,
            ))
        }
    }
}

fn distinct(list: Vec<MthRef>) -> Vec<MthRef> {
    let mut out: Vec<MthRef> = Vec::new();
    for m in list {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

fn apply_override_attr(
    _root: &Root,
    cx: &mut Cx,
    cidx: usize,
    midx: usize,
    override_list: Vec<MthRef>,
    base_methods: Vec<MthRef>,
    update: bool,
) -> OverrideAttr {
    // don't rename method if override list contains not resolved method
    let dont_rename = override_list.iter().any(|m| !matches!(m, MthRef::App(..)));
    let mut nodes: Vec<MthRef> = vec![MthRef::App(cidx, midx)];
    nodes.extend(
        override_list
            .iter()
            .filter(|m| matches!(m, MthRef::App(..)))
            .cloned(),
    );
    let node_ids: Vec<(usize, usize)> = nodes
        .iter()
        .filter_map(|n| match n {
            MthRef::App(c, i) => Some((*c, *i)),
            MthRef::Clsp(..) => None,
        })
        .collect();
    let related = if update {
        // merge related methods from all override attributes: use one of already allocated sets
        let existing = nodes
            .iter()
            .find_map(|n| cx.attrs.get(n).map(|a| a.related));
        let set = match existing {
            Some(s) => {
                cx.sets[s].extend(node_ids.iter().copied());
                s
            }
            None => {
                cx.sets.push(node_ids.iter().copied().collect());
                cx.sets.len() - 1
            }
        };
        for n in &nodes {
            if let Some(a) = cx.attrs.get(n) {
                let other = a.related;
                if other != set {
                    let items = cx.sets[other].clone();
                    cx.sets[set].extend(items);
                }
            }
        }
        set
    } else {
        cx.sets.push(node_ids.iter().copied().collect());
        cx.sets.len() - 1
    };
    let mut depth = 0usize;
    for node in &nodes {
        if dont_rename {
            if let MthRef::App(c, i) = node {
                cx.dont_rename.insert((*c, *i));
            }
        }
        if depth == 0 {
            depth = 1;
            continue;
        }
        if update {
            if let Some(a) = cx.attrs.get_mut(node) {
                a.related = related;
                continue;
            }
        }
        let tail = override_list.get(depth..).unwrap_or(&[]).to_vec();
        cx.attrs.insert(
            node.clone(),
            OverrideAttr {
                override_list: tail,
                base_methods: base_methods.clone(),
                related,
            },
        );
        depth += 1;
    }
    OverrideAttr {
        override_list,
        base_methods,
        related,
    }
}

fn fix_return_type(
    root: &Root,
    cidx: usize,
    midx: usize,
    base: &Details,
    sd: &SuperTypes,
) -> Option<ArgType> {
    let ret = &root.classes[cidx].methods[midx].return_type;
    if *ret == ArgType::void() {
        return None;
    }
    if *ret == base.ret || !base.ret.contains_type_variable() {
        return None;
    }
    let tc = TypeCompare::new(&root.clsp);
    let tu = TypeUtils::new(root);
    for st in &sd.types {
        if tc.compare_types(st, &base.decl_class) == TypeCompareEnum::NarrowByGeneric {
            if let Some(target) = tu.replace_class_generics(st, &base.ret) {
                if !target.contains_type_variable() && target != *ret {
                    return Some(target);
                }
            }
        }
    }
    None
}

fn fix_arg_types(
    root: &Root,
    cidx: usize,
    midx: usize,
    base: &Details,
    sd: &SuperTypes,
) -> Option<Vec<ArgType>> {
    let mth_args = &root.classes[cidx].methods[midx].arg_types;
    if *mth_args == base.args || mth_args.len() != base.args.len() {
        return None;
    }
    let tc = TypeCompare::new(&root.clsp);
    let tu = TypeUtils::new(root);
    let mut changed = false;
    let mut out = Vec::with_capacity(mth_args.len());
    for (arg, base_arg) in mth_args.iter().zip(&base.args) {
        let mut new_type = None;
        if arg != base_arg && base_arg.contains_type_variable() {
            for st in &sd.types {
                if tc.compare_types(st, &base.decl_class) == TypeCompareEnum::NarrowByGeneric {
                    if let Some(t) = tu.replace_class_generics(st, base_arg) {
                        if !t.contains_type_variable() && t != *arg {
                            new_type = Some(t);
                            break;
                        }
                    }
                }
            }
        }
        match new_type {
            Some(t) => {
                changed = true;
                out.push(t);
            }
            None => out.push(arg.clone()),
        }
    }
    changed.then_some(out)
}

/// `checkMethodSignatureCollisions`: the method whose signature collides with the changed
/// signature of `mth` is renamed (when it is not part of an override hierarchy).
fn check_method_signature_collisions(root: &mut Root, cx: &Cx, cidx: usize, midx: usize) {
    let (name, new_sig) = {
        let m = &root.classes[cidx].methods[midx];
        let name = m.alias.clone().unwrap_or_else(|| m.info.name.clone());
        let sig = crate::model::info::make_short_id(&name, &m.arg_types, None);
        (name, sig)
    };
    let other = root.classes[cidx]
        .methods
        .iter()
        .enumerate()
        .position(|(i, o)| {
            let o_name = o.alias.as_deref().unwrap_or(&o.info.name);
            o_name == name
                && i != midx
                && crate::model::info::make_short_id(o_name, &o.info.arg_types, None) == new_sig
        });
    if let Some(o) = other {
        let skip =
            cx.dont_rename.contains(&(cidx, o)) || cx.attrs.contains_key(&MthRef::App(cidx, o));
        if skip {
            root.classes[cidx].methods[o].comments.push((
                crate::codegen::CommentsLevel::Warn,
                "Can't rename method to resolve collision".to_string(),
            ));
        } else {
            let new_name = make_new_name(root, cidx, o);
            let node = &mut root.classes[cidx].methods[o];
            node.alias = Some(new_name);
            node.alias_reason = Some("avoid collision after fix types in other method".to_string());
        }
        root.classes[cidx].methods[o].bridged_by = Some(midx);
    }
}

fn make_new_name(root: &Root, cidx: usize, midx: usize) -> String {
    let m = &root.classes[cidx].methods[midx];
    let base = m.alias.clone().unwrap_or_else(|| m.info.name.clone());
    let mut k = 2;
    loop {
        let name = format!("{base}{k}");
        if !root.classes[cidx]
            .methods
            .iter()
            .any(|x| x.info.name == name)
        {
            return name;
        }
        k += 1;
    }
}
