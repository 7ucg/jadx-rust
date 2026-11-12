//! `SignatureProcessor`: apply generic signatures to classes, fields and methods.
//!

use std::collections::BTreeSet;

use crate::model::node::{Root, ACC_ENUM};
use crate::model::types::ArgType;
use crate::name_mapper;
use crate::signature::SignatureParser;
use crate::type_compare::{TypeCompare, TypeCompareEnum};

pub fn process_signatures(root: &mut Root) {
    for idx in root.traversal_order() {
        parse_class_signature(root, idx);
        for f in 0..root.classes[idx].fields.len() {
            parse_field_signature(root, idx, f);
        }
        for m in 0..root.classes[idx].methods.len() {
            parse_method_signature(root, idx, m);
        }
    }
}

fn validate_parsed_type(root: &Root, parsed: &ArgType, current: &ArgType) -> bool {
    let result = TypeCompare::new(&root.clsp).compare_types(parsed, current);
    if result == TypeCompareEnum::Unknown
        && parsed.is_object()
        && !parsed
            .object_name()
            .is_some_and(|n| name_mapper::is_valid_full_identifier(&n))
    {
        return false;
    }
    result != TypeCompareEnum::Conflict
}

/// Type variables visible in a class: own parameters plus those of all outer classes.
fn known_type_vars_at_class(root: &Root, idx: usize) -> Vec<ArgType> {
    let mut vars = root.classes[idx].generics.clone();
    if root.classes[idx].is_inner() {
        let mut cur = idx;
        while let Some(p) = root.parent_index(cur) {
            if p == cur {
                break;
            }
            vars.extend(root.classes[p].generics.iter().cloned());
            cur = p;
        }
    }
    vars
}

fn generic_types_mut(t: &mut ArgType) -> Option<&mut Vec<ArgType>> {
    match t {
        ArgType::Generic { generics, .. } => Some(generics),
        ArgType::OuterGeneric { inner, .. } => generic_types_mut(inner),
        _ => None,
    }
}

fn expand_in_place(t: &mut ArgType, vars: &[ArgType]) {
    match t {
        ArgType::GenericType { name, extend_types } => {
            if let Some(v) = vars
                .iter()
                .find(|v| matches!(v, ArgType::GenericType { name: n, .. } if n == name))
            {
                *extend_types = v.extend_types().to_vec();
            }
        }
        ArgType::Array(e) => expand_in_place(e, vars),
        ArgType::Wildcard { ty, .. } => expand_in_place(ty, vars),
        ArgType::Generic { .. } | ArgType::OuterGeneric { .. } => {
            if let Some(g) = generic_types_mut(t) {
                for x in g {
                    expand_in_place(x, vars);
                }
            }
        }
        _ => {}
    }
}

fn expand_type_variables(ty: ArgType, vars: &[ArgType]) -> ArgType {
    if !ty.contains_type_variable()
        || vars.is_empty()
        || vars.iter().all(|v| v.extend_types().is_empty())
    {
        return ty;
    }
    let mut ty = ty;
    expand_in_place(&mut ty, vars);
    ty
}

fn valid_class_type(t: &Option<ArgType>) -> bool {
    matches!(t, Some(t) if t.is_object())
}

fn parse_class_signature(root: &mut Root, idx: usize) {
    let Some(sig) = root.classes[idx].signature.clone() else {
        return;
    };
    let mut sp = SignatureParser::new(&sig);
    let result = (|| -> Result<(), String> {
        let generics = sp.consume_generic_type_parameters().map_err(|e| e.0)?;
        let parsed_super = sp.consume_type().map_err(|e| e.0)?;
        let cls = &root.classes[idx];
        let parsed_super_t = parsed_super.clone().ok_or("missing super type")?;
        let cur_super = cls.super_class.clone();
        let super_class = if parsed_super_t.object_name() == cls.info.arg_type().object_name() {
            cur_super
        } else if valid_class_type(&parsed_super) {
            parsed_super
        } else {
            cur_super
        };
        let parsed_ifaces = sp.consume_type_list().map_err(|e| e.0)?;
        let cur_ifaces = cls.interfaces.clone();
        let interfaces = if parsed_ifaces.is_empty() {
            cur_ifaces
        } else {
            let count = cur_ifaces.len().min(parsed_ifaces.len());
            cur_ifaces
                .iter()
                .enumerate()
                .map(|(i, cur)| {
                    if i < count && parsed_ifaces[i].is_object() {
                        parsed_ifaces[i].clone()
                    } else {
                        cur.clone()
                    }
                })
                .collect()
        };
        let (generics, missing) =
            fix_type_param_declarations(root, idx, generics, &super_class, &interfaces);
        let cls = &mut root.classes[idx];
        if let Some(m) = missing {
            cls.comments.push((
                crate::codegen::CommentsLevel::Info,
                format!("Add missing generic type declarations: {m}"),
            ));
        }
        cls.generics = generics;
        cls.super_class = super_class;
        cls.interfaces = interfaces;
        Ok(())
    })();
    let _ = result;
}

fn fix_type_param_declarations(
    root: &Root,
    idx: usize,
    generics: Vec<ArgType>,
    super_class: &Option<ArgType>,
    interfaces: &[ArgType],
) -> (Vec<ArgType>, Option<String>) {
    let super_is_object = super_class
        .as_ref()
        .map(|s| *s == ArgType::object_type())
        .unwrap_or(true);
    if interfaces.is_empty() && super_is_object {
        return (generics, None);
    }
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut collect = |t: &ArgType| {
        t.visit_types::<()>(&mut |x| {
            if x.is_generic_type() {
                used.insert(x.object_name().unwrap_or_default());
            }
            None
        });
    };
    if let Some(s) = super_class {
        collect(s);
    }
    for i in interfaces {
        collect(i);
    }
    if used.is_empty() {
        return (generics, None);
    }
    let mut known = generics.clone();
    if root.classes[idx].is_inner() {
        let mut cur = idx;
        while let Some(p) = root.parent_index(cur) {
            if p == cur {
                break;
            }
            known.extend(root.classes[p].generics.iter().cloned());
            cur = p;
        }
    }
    for k in &known {
        if let Some(n) = k.object_name() {
            used.remove(&n);
        }
    }
    if used.is_empty() {
        return (generics, None);
    }
    // `Set.toString` of the reference hash set
    let mut shown: Vec<&String> = used.iter().collect();
    shown.sort_by_key(|s| {
        let h = crate::model::bitset::java_string_hash(s) as u32;
        ((h ^ (h >> 16)) as usize) & 15
    });
    let text = format!(
        "[{}]",
        shown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut fixed = generics;
    fixed.extend(used.into_iter().map(|n| ArgType::generic_type(&n)));
    (fixed, Some(text))
}

fn validate_inner_type(root: &Root, ty: &ArgType) -> bool {
    let (Some(inner), Some(outer)) = (ty.inner_type(), ty.outer_type()) else {
        return true;
    };
    let Some(outer_idx) = root.resolve_class(outer) else {
        return true;
    };
    let inner_obj = match inner.outer_type() {
        Some(o) => o.object_name().unwrap_or_default(),
        None => inner.object_name().unwrap_or_default(),
    };
    if !inner_obj.contains('.') {
        return root.classes.iter().any(|c| {
            root.parent_index_of(c) == Some(outer_idx) && c.info.short_name() == inner_obj
        });
    }
    let Some(inner_idx) = root.class_index(&inner_obj) else {
        return false;
    };
    let _ = inner_idx;
    validate_inner_type(root, inner)
}

fn parse_field_signature(root: &mut Root, idx: usize, f: usize) {
    let Some(sig) = root.classes[idx].fields[f].signature.clone() else {
        return;
    };
    let mut sp = SignatureParser::new(&sig);
    let Ok(Some(parsed)) = sp.consume_type() else {
        return;
    };
    if !validate_inner_type(root, &parsed) {
        return;
    }
    let vars = known_type_vars_at_class(root, idx);
    let ty = expand_type_variables(parsed, &vars);
    if !validate_parsed_type(root, &ty, &root.classes[idx].fields[f].ty) {
        return;
    }
    root.classes[idx].fields[f].ty = ty;
}

fn parse_method_signature(root: &mut Root, idx: usize, m: usize) {
    let Some(sig) = root.classes[idx].methods[m].signature.clone() else {
        return;
    };
    let mut sp = SignatureParser::new(&sig);
    let args_count = root.classes[idx].methods[m].info.args_count();
    let Ok(type_params) = sp.consume_generic_type_parameters() else {
        return;
    };
    let Ok(parsed_args) = sp.consume_method_args(args_count) else {
        return;
    };
    let Ok(Some(parsed_ret)) = sp.consume_type() else {
        return;
    };
    if !validate_inner_type(root, &parsed_ret)
        || !parsed_args.iter().all(|a| validate_inner_type(root, a))
    {
        return;
    }
    // type parameters are applied before expanding the argument types
    let mut vars = known_type_vars_at_class(root, idx);
    vars.extend(type_params.iter().cloned());
    let ret = expand_type_variables(parsed_ret, &vars);
    let args: Vec<ArgType> = parsed_args
        .into_iter()
        .map(|a| expand_type_variables(a, &vars))
        .collect();
    let mth = &root.classes[idx].methods[m];
    if !validate_parsed_type(root, &ret, &mth.info.return_type) {
        return;
    }
    let Some(checked) = check_arg_types(root, idx, m, args) else {
        return;
    };
    let mth = &mut root.classes[idx].methods[m];
    mth.type_params = type_params;
    mth.arg_types = checked;
    mth.return_type = ret;
}

fn check_arg_types(
    root: &Root,
    idx: usize,
    m: usize,
    parsed: Vec<ArgType>,
) -> Option<Vec<ArgType>> {
    let cls = &root.classes[idx];
    let info = &cls.methods[m].info;
    let cur = &info.arg_types;
    if parsed.len() != cur.len() {
        if cls.access & ACC_ENUM != 0 {
            return None;
        }
        if info.is_constructor() && !cur.is_empty() && !parsed.is_empty() {
            let mut new_args = parsed;
            new_args.insert(0, cur[0].clone());
            if new_args.len() == cur.len() {
                return Some(new_args);
            }
        }
        return None;
    }
    for (p, c) in parsed.iter().zip(cur) {
        if !validate_parsed_type(root, p, c) {
            return None;
        }
    }
    Some(parsed)
}
