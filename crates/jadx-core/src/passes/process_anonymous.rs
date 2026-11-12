//! `ProcessAnonymous`: mark anonymous and lambda classes for inlining into the class that
//! creates them.

use std::collections::{HashMap, HashSet};

use crate::model::node::{
    AnonymousClassAttr, InlineType, MethodId, Root, ACC_FINAL, ACC_PUBLIC, ACC_STATIC,
    ACC_SYNTHETIC,
};
use crate::model::types::ArgType;

const KOTLIN_LAMBDA: &str = "kotlin.jvm.internal.Lambda";

pub fn process_anonymous(root: &mut Root, allow_inline_kotlin_lambda: bool) {
    let order = root.sorted_class_order();
    for &c in &order {
        mark_anonymous_class(root, c, allow_inline_kotlin_lambda);
    }
    merge_anonymous_deps(root, &order);
}

fn mark_anonymous_class(root: &mut Root, c: usize, allow_inline_kotlin_lambda: bool) {
    if !can_be_anonymous(root, c) {
        return;
    }
    let ctrs: Vec<usize> = (0..root.classes[c].methods.len())
        .filter(|m| root.classes[c].methods[*m].is_constructor())
        .collect();
    let [ctr] = ctrs[..] else {
        return;
    };
    let Some((inline_type, inst_fld)) = check_usage(root, c, ctr) else {
        return;
    };
    if let Some(f) = inst_fld {
        root.classes[c].fields[f].inline_instance_field = true;
    }
    let Some(base_type) = base_type(root, c, allow_inline_kotlin_lambda) else {
        return;
    };
    let outer = match inline_type {
        InlineType::InstanceField => root.classes[c].use_in_mth[0].0,
        InlineType::Constructor => root.classes[c].methods[ctr].use_in[0].0,
    };
    root.classes[outer].inlined_classes.push(c);
    let cls = &mut root.classes[c];
    cls.inlined_into = Some(outer);
    cls.anonymous = Some(AnonymousClassAttr {
        outer_class: outer,
        base_type,
        inline_type,
    });
    cls.dont_generate = true;
    cls.methods[ctr].anonymous_constructor = true;

    // force anonymous class to be processed before outer class,
    // actual usage of outer class will be removed at anonymous class process
    let top_outer = root.top_parent_index(outer);
    remove_first(&mut root.classes[c].dependencies, top_outer);
    remove_first(&mut root.classes[outer].use_in, c);

    // move dependency to codegen stage
    if root.parent_index(c).is_none() {
        remove_first(&mut root.classes[top_outer].dependencies, c);
        let deps = &mut root.classes[top_outer].codegen_deps;
        if !deps.contains(&c) {
            deps.push(c);
        }
    }
}

fn remove_first(list: &mut Vec<usize>, item: usize) {
    if let Some(p) = list.iter().position(|x| *x == item) {
        list.remove(p);
    }
}

fn can_be_anonymous(root: &Root, c: usize) -> bool {
    let cls = &root.classes[c];
    if cls.access & ACC_SYNTHETIC != 0 {
        return true;
    }
    let short = cls.info.short_name();
    if short.contains('$') || short.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        return true;
    }
    if let ([_], [use_mth]) = (&cls.use_in[..], &cls.use_in_mth[..]) {
        // allow use in enum class init
        let m = &root.classes[use_mth.0].methods[use_mth.1];
        return m.info.is_class_init() && root.classes[use_mth.0].is_enum();
    }
    false
}

/// Checks:
/// - class have only one constructor which used only once (allow common code for field init)
/// - methods or fields not used outside (allow only nested inner classes with synthetic usage)
/// - if constructor used only in class init check if possible inline by instance field
///
/// Returns the decided inline type and the instance field of an `InstanceField` inline.
fn check_usage(root: &Root, c: usize, ctr: usize) -> Option<(InlineType, Option<usize>)> {
    let cls = &root.classes[c];
    let ctr_mth = &cls.methods[ctr];
    if ctr_mth.use_in.len() != 1 && !check_for_common_field_init(root, &ctr_mth.use_in) {
        return None;
    }
    let ctr_use_mth = ctr_mth.use_in[0];
    let ctr_use_cls = ctr_use_mth.0;
    if ctr_use_cls == c {
        return check_for_instance_field_usage(root, c, ctr)
            .map(|f| (InlineType::InstanceField, Some(f)));
    }
    if root.top_parent_index(ctr_use_cls) == c {
        // exclude usage inside inner classes
        return None;
    }
    if !check_methods_usage(root, c, ctr, ctr_use_mth) {
        return None;
    }
    for f in &cls.fields {
        if f.use_in
            .iter()
            .any(|m| bad_method_usage(root, c, *m, f.access))
        {
            return None;
        }
    }
    Some((InlineType::Constructor, None))
}

fn check_methods_usage(root: &Root, c: usize, ctr: usize, ctr_use_mth: MethodId) -> bool {
    for (mi, m) in root.classes[c].methods.iter().enumerate() {
        if mi == ctr {
            continue;
        }
        for use_mth in &m.use_in {
            if *use_mth == ctr_use_mth {
                continue;
            }
            if bad_method_usage(root, c, *use_mth, m.access) {
                return false;
            }
        }
    }
    true
}

fn check_for_instance_field_usage(root: &Root, c: usize, ctr: usize) -> Option<usize> {
    let cls = &root.classes[c];
    let ctr_use_mth = cls.methods[ctr].use_in[0];
    let use_mth = &root.classes[ctr_use_mth.0].methods[ctr_use_mth.1];
    if !use_mth.info.is_class_init() {
        return None;
    }
    if cls.use_in_mth.is_empty() {
        // no outside usage, inline not needed
        return None;
    }
    let want = ACC_PUBLIC | ACC_STATIC | ACC_FINAL;
    let mut candidates = cls
        .fields
        .iter()
        .enumerate()
        .filter(|(_, f)| f.access & want == want && f.info.ty == *cls.info.arg_type());
    let (inst_idx, inst_fld) = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    let use_in = &inst_fld.use_in;
    if use_in.len() != 2
        || !use_in.contains(&ctr_use_mth) // initialized in class init
        || !cls.use_in_mth.iter().all(|m| use_in.contains(m))
    // class used only with this field
    {
        return None;
    }
    if !check_methods_usage(root, c, ctr, ctr_use_mth) {
        return None;
    }
    for (fi, f) in cls.fields.iter().enumerate() {
        if fi == inst_idx {
            continue;
        }
        if f.use_in
            .iter()
            .any(|m| bad_method_usage(root, c, *m, f.access))
        {
            return None;
        }
    }
    Some(inst_idx)
}

fn bad_method_usage(root: &Root, c: usize, use_mth: MethodId, access: u32) -> bool {
    let use_cls = use_mth.0;
    if use_cls == c {
        return false;
    }
    if access & ACC_SYNTHETIC != 0 {
        // allow synthetic usage in inner class
        return root.parent_index(use_cls).unwrap_or(use_cls) != c;
    }
    true
}

/// Checks:
/// + all in constructors
/// + all usage in one class
fn check_for_common_field_init(root: &Root, ctr_use: &[MethodId]) -> bool {
    let Some(first) = ctr_use.first() else {
        return false;
    };
    ctr_use
        .iter()
        .all(|m| root.classes[m.0].methods[m.1].is_constructor() && m.0 == first.0)
}

fn base_type(root: &Root, c: usize, allow_inline_kotlin_lambda: bool) -> Option<ArgType> {
    let cls = &root.classes[c];
    if cls.interfaces.len() > 1 {
        return None;
    }
    let object = ArgType::object_type();
    let sup = match &cls.super_class {
        Some(s) if *s != object => s,
        _ => {
            return Some(cls.interfaces.first().cloned().unwrap_or(object));
        }
    };
    let Some(iface) = cls.interfaces.first() else {
        return Some(sup.clone());
    };
    // check if super class already implement that interface (weird case)
    let sup_name = sup.object_name().unwrap_or_default();
    let iface_name = iface.object_name().unwrap_or_default();
    if root.clsp.is_implements(&sup_name, &iface_name) {
        return Some(sup.clone());
    }
    if allow_inline_kotlin_lambda && sup_name == KOTLIN_LAMBDA {
        return Some(iface.clone());
    }
    None
}

fn undo_anonymous_mark(root: &mut Root, c: usize) {
    let Some(attr) = root.classes[c].anonymous.take() else {
        return;
    };
    let top_outer = root.top_parent_index(attr.outer_class);
    root.classes[c].dependencies.push(top_outer);
    root.classes[attr.outer_class].use_in.push(c);
    let cls = &mut root.classes[c];
    cls.dont_generate = false;
    for m in cls.methods.iter_mut().filter(|m| m.is_constructor()) {
        m.anonymous_constructor = false;
    }
}

fn merge_anonymous_deps(root: &mut Root, order: &[usize]) {
    // Collect edges to build bidirectional tree:
    // inline edge: anonymous -> outer (one-to-one)
    // use edges: outer -> *anonymous (one-to-many)
    let mut inline_map: HashMap<usize, usize> = HashMap::new();
    let mut use_map: HashMap<usize, Vec<usize>> = HashMap::new();
    for &c in order {
        if let Some(attr) = &root.classes[c].anonymous {
            use_map.entry(attr.outer_class).or_default().push(c);
            // put leaf explicitly
            use_map.entry(c).or_default();
            inline_map.insert(c, attr.outer_class);
        }
    }
    if inline_map.is_empty() {
        return;
    }
    // starting from leaf process deps in nodes up to root
    for &c in order {
        if use_map.get(&c).is_some_and(|l| l.is_empty()) {
            update_deps(root, c, &inline_map);
        }
    }
    for cls in &mut root.classes {
        let deps = &mut cls.codegen_deps;
        if deps.len() > 1 {
            // distinct sorted deps
            let set: HashSet<usize> = deps.iter().copied().collect();
            *deps = set.into_iter().collect();
        }
    }
    // codegen deps are sorted by class name
    let keys: Vec<Vec<u16>> = root
        .classes
        .iter()
        .map(|c| c.info.raw_name().encode_utf16().collect())
        .collect();
    for cls in &mut root.classes {
        cls.codegen_deps.sort_by(|a, b| keys[*a].cmp(&keys[*b]));
    }
}

fn update_deps(root: &mut Root, leaf: usize, inline_map: &HashMap<usize, usize>) {
    let mut added: Vec<usize> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut current = leaf;
    let top_node = loop {
        if !seen.insert(current) {
            // loop in anonymous inline
            for c in added {
                undo_anonymous_mark(root, c);
            }
            return;
        }
        added.push(current);
        match inline_map.get(&current) {
            Some(next) => current = *next,
            None => break root.top_parent_index(current),
        }
    };
    if added.len() <= 2 {
        // first level deps already processed
        return;
    }
    for a in added {
        let top = root.top_parent_index(a);
        root.classes[top_node].codegen_deps.push(top);
    }
}
