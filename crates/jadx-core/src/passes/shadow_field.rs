//! `ShadowFieldVisitor`: fix access to fields shadowed by a field of a sub class.

use std::collections::{HashMap, HashSet};

use crate::model::cfg::MethodCfg;
use crate::model::info::FieldInfo;
use crate::model::insn::{iflag, IndexValue, InsnId, InsnKind, InsnNode, InsnType};
use crate::model::node::{Root, ShadowFix, ACC_STATIC};
use crate::model::ssa::ArgRef;
use crate::passes::insn_utils::{wrap_insn_into_arg, wrap_instruction};

/// `ShadowFieldVisitor.init`: shadowed fields per class (computed once per root).
fn fix_map(root: &Root) -> &HashMap<usize, HashMap<FieldInfo, ShadowFix>> {
    root.shadow_fields.get_or_init(|| {
        let mut map = HashMap::new();
        for ci in 0..root.classes.len() {
            let fix = search_shadowed_fields(root, ci);
            if !fix.is_empty() {
                map.insert(ci, fix);
            }
        }
        map
    })
}

/// Instance fields of the class and its super classes as `(class, field)` indexes.
fn collect_all_instance_fields(root: &Root, cls: usize) -> Vec<(usize, usize)> {
    let mut list = Vec::new();
    let mut visited: HashSet<usize> = HashSet::new();
    let mut current = Some(cls);
    while let Some(c) = current {
        if !visited.insert(c) {
            // 'super' loop
            return list;
        }
        for (fi, f) in root.classes[c].fields.iter().enumerate() {
            if f.access & ACC_STATIC == 0 {
                list.push((c, fi));
            }
        }
        current = root.classes[c]
            .super_class
            .as_ref()
            .and_then(|s| root.resolve_class(s));
    }
    list
}

fn search_shadowed_fields(root: &Root, this_cls: usize) -> HashMap<FieldInfo, ShadowFix> {
    let all_fields = collect_all_instance_fields(root, this_cls);
    if all_fields.is_empty() {
        return HashMap::new();
    }
    // group by name (keep the order of the first occurrence)
    let mut order: Vec<&str> = Vec::new();
    let mut by_name: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    for &(c, f) in &all_fields {
        let name = root.classes[c].fields[f].info.name.as_str();
        by_name
            .entry(name)
            .or_insert_with(|| {
                order.push(name);
                Vec::new()
            })
            .push((c, f));
    }
    let mut fix = HashMap::new();
    for name in order {
        let fields = &by_name[name];
        if fields.len() == 1 {
            continue;
        }
        let from_this_cls = fields[0].0 == this_cls;
        if from_this_cls && fields.len() == 2 {
            // only one super class contains same field => can use super
            let other = fields[1];
            if other.0 != this_cls {
                fix.insert(
                    root.classes[other.0].fields[other.1].info.clone(),
                    ShadowFix::Super,
                );
            }
        } else {
            // several super classes contains same field => can't use super, need cast to exact class
            for &(c, f) in fields {
                if c != this_cls {
                    fix.insert(root.classes[c].fields[f].info.clone(), ShadowFix::Cast);
                }
            }
        }
    }
    fix
}

pub fn shadow_field(cfg: &mut MethodCfg, root: &Root) -> Result<(), String> {
    let map = fix_map(root);
    for block in cfg.blocks.clone() {
        for insn in cfg.block(block).insns.clone() {
            process_insn(cfg, root, map, insn)?;
        }
    }
    Ok(())
}

fn process_insn(
    cfg: &mut MethodCfg,
    root: &Root,
    map: &HashMap<usize, HashMap<FieldInfo, ShadowFix>>,
    insn: InsnId,
) -> Result<(), String> {
    let n = cfg.insn(insn);
    if !matches!(n.ty, InsnType::Iput | InsnType::Iget) {
        return Ok(());
    }
    let InsnKind::Index(IndexValue::Field(field_info)) = n.kind.clone() else {
        return Ok(());
    };
    let Some(last) = n.args.len().checked_sub(1) else {
        return Ok(());
    };
    let ty = cfg.arg_type(ArgRef::Arg(insn, last));
    if !ty.is_type_known() || !ty.is_object() {
        return Ok(());
    }
    let Some(name) = ty.object_name() else {
        return Ok(());
    };
    let Some(ci) = root.class_index(&name) else {
        return Ok(());
    };
    let Some(info) = map.get(&ci) else {
        return Ok(());
    };
    let Some(&fix) = info.get(&field_info) else {
        return Ok(());
    };
    fix_field_access(cfg, insn, last, &field_info, fix)
}

fn fix_field_access(
    cfg: &mut MethodCfg,
    insn: InsnId,
    index: usize,
    field_info: &FieldInfo,
    fix: ShadowFix,
) -> Result<(), String> {
    let at = ArgRef::Arg(insn, index);
    if fix == ShadowFix::Super {
        if let Some(r) = cfg.try_reg_mut(at) {
            if r.is_this {
                // convert 'this' to 'super'
                r.is_super = true;
                return Ok(());
            }
        }
    }
    // apply cast
    let mut cast = InsnNode::new(
        InsnType::Cast,
        InsnKind::Index(IndexValue::Type(field_info.decl_class.arg_type().clone())),
    );
    cast.flags |= iflag::SYNTHETIC | iflag::EXPLICIT_CAST;
    let cast_id = cfg.add_insn(cast);
    let arg = cfg.insn(insn).args[index].clone();
    let dup = cfg.duplicate_arg(&arg);
    crate::passes::insn_utils::add_arg(cfg, cast_id, dup);
    if cfg.try_reg(at).is_some() {
        wrap_instruction(cfg, at, cast_id, false)?;
    } else {
        let ty = cfg.arg_type(at);
        let wrapped = wrap_insn_into_arg(cfg, cast_id)?;
        let wrapped = match wrapped {
            crate::model::insn::InsnArg::Wrapped(mut w) => {
                w.ty = ty;
                crate::model::insn::InsnArg::Wrapped(w)
            }
            other => other,
        };
        cfg.replace_arg(insn, index, wrapped);
    }
    Ok(())
}
