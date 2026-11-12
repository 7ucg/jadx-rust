//! `FixSwitchOverEnum`: restore `switch` over an enum: the switch over `ordinal()` (direct) or
//! over the remapped ordinal of a compiler generated map class is replaced with a switch over the
//! enum value with the enum constants as case keys.

use std::collections::HashMap;

use crate::model::cfg::MethodCfg;
use crate::model::info::FieldInfo;
use crate::model::insn::{iflag, IndexValue, InsnArg, InsnId, InsnKind, InsnType};
use crate::model::node::{MethodId, Root, ACC_FINAL, ACC_STATIC, ACC_SYNTHETIC};
use crate::model::ssa::ArgRef;
use crate::passes::constructor::MethodEnv;
use crate::passes::driver::MethodState;

/// `initClsEnumMap`: `EnumMapAttr` of a class: the map fields of the compiler generated class
/// with the enum constant of every ordinal.
pub fn init_cls_enum_map(root: &mut Root, methods: &HashMap<MethodId, MethodState>, cls: usize) {
    let Some(ci) = root.classes[cls]
        .methods
        .iter()
        .position(|m| m.info.is_class_init())
    else {
        return;
    };
    let Some(cfg) = methods.get(&(cls, ci)).and_then(|s| s.cfg.as_ref()) else {
        return;
    };
    if cfg.blocks.is_empty() {
        return;
    }
    let mut found: Vec<((usize, usize), i32, FieldInfo)> = Vec::new();
    for &b in &cfg.blocks {
        for &insn in &cfg.block(b).insns {
            if cfg.insn(insn).ty != InsnType::Aput {
                continue;
            }
            if let Some(e) = add_to_enum_map(root, cfg, insn) {
                found.push(e);
            }
        }
    }
    // the map of a field is attached to its own class: only the maps of fields of the class
    // with the initializer are found later
    for (field, literal, enum_field) in found {
        if field.0 == cls {
            root.enum_maps
                .entry(field)
                .or_default()
                .insert(literal, enum_field);
        }
    }
}

fn add_to_enum_map(
    root: &Root,
    cfg: &MethodCfg,
    aput: InsnId,
) -> Option<((usize, usize), i32, FieldInfo)> {
    let InsnArg::Literal(lit) = cfg.insn(aput).args.get(2)? else {
        return None;
    };
    let (enum_arg, field) = check_enum_map_access(root, cfg, aput)?;
    let InsnArg::Wrapped(w) = enum_arg else {
        return None;
    };
    let InsnKind::Index(IndexValue::Field(f)) = &cfg.insn(w.insn).kind else {
        return None;
    };
    root.resolve_field(f)?;
    Some((field, lit.literal as i32, f.clone()))
}

/// `checkEnumMapAccess`: the enum instance argument and the map field.
fn check_enum_map_access(
    root: &Root,
    cfg: &MethodCfg,
    check: InsnId,
) -> Option<(InsnArg, (usize, usize))> {
    let n = cfg.insn(check);
    let (InsnArg::Wrapped(sget), InsnArg::Wrapped(inv)) = (n.args.first()?, n.args.get(1)?) else {
        return None;
    };
    let inv_insn = cfg.insn(inv.insn);
    let sget_insn = cfg.insn(sget.insn);
    if inv_insn.ty != InsnType::Invoke || sget_insn.ty != InsnType::Sget {
        return None;
    }
    let InsnKind::Invoke { mth, .. } = &inv_insn.kind else {
        return None;
    };
    if mth.short_id() != "ordinal()I" {
        return None;
    }
    let enum_cls = root.class_index(&mth.decl_class.raw_name())?;
    if !root.classes[enum_cls].is_enum() {
        return None;
    }
    let InsnKind::Index(IndexValue::Field(f)) = &sget_insn.kind else {
        return None;
    };
    let (fc, fi) = root.resolve_field_id(f)?;
    if root.classes[fc].fields[fi].access & ACC_SYNTHETIC == 0 {
        return None;
    }
    Some((inv_insn.args.first()?.clone(), (fc, fi)))
}

pub fn fix_switch_over_enum(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    pending: &mut Vec<(usize, usize)>,
) -> Result<(), String> {
    let mut changed = false;
    for block in cfg.blocks.clone() {
        for insn in cfg.block(block).insns.clone() {
            if cfg.insn(insn).ty == InsnType::Switch && !cfg.insn(insn).contains_flag(iflag::REMOVE)
            {
                changed |= process_enum_switch(cfg, env, insn, pending)?;
            }
        }
    }
    if changed {
        crate::passes::regions::shrink_method(cfg)?;
    }
    Ok(())
}

fn process_enum_switch(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    sw: InsnId,
    pending: &mut Vec<(usize, usize)>,
) -> Result<bool, String> {
    let Some(InsnArg::Wrapped(w)) = cfg.insn(sw).args.first().cloned() else {
        return Ok(false);
    };
    match cfg.insn(w.insn).ty {
        InsnType::Aget => process_remapped_enum_switch(cfg, env, sw, w.insn, pending),
        InsnType::Invoke => process_direct_enum_switch(cfg, env, sw, w.insn),
        _ => Ok(false),
    }
}

fn switch_keys(cfg: &MethodCfg, sw: InsnId) -> Vec<i32> {
    match &cfg.insn(sw).kind {
        InsnKind::Switch { data: Some(d), .. } => d.keys.clone(),
        _ => Vec::new(),
    }
}

/// `executeReplace`: replace the argument and the case keys.
fn execute_replace(
    cfg: &mut MethodCfg,
    sw: InsnId,
    inv_var: InsnArg,
    case_replace: &[Option<FieldInfo>],
) -> bool {
    if !cfg.regions.insn_region_ref.contains_key(&sw) {
        return false;
    }
    cfg.replace_arg(sw, 0, inv_var);
    if let InsnKind::Switch { data: Some(d), .. } = &mut cfg.insns[sw].kind {
        d.key_fields.resize(d.keys.len(), None);
        for (i, f) in case_replace.iter().enumerate() {
            d.key_fields[i] = f.clone();
        }
    }
    true
}

fn process_direct_enum_switch(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    sw: InsnId,
    inv: InsnId,
) -> Result<bool, String> {
    let InsnKind::Invoke { mth, .. } = &cfg.insn(inv).kind else {
        return Ok(false);
    };
    if mth.short_id() != "ordinal()I" {
        return Ok(false);
    }
    let Some(inv_var) = cfg.insn(inv).args.first().cloned() else {
        return Ok(false);
    };
    let ty = cfg.arg_type(ArgRef::Arg(inv, 0));
    let Some(enum_cls) = env.root.resolve_class(&ty) else {
        return Ok(false);
    };
    let Some(attr) = &env.root.classes[enum_cls].enum_class else {
        return Ok(false);
    };
    let keys = switch_keys(cfg, sw);
    // `mapToCases`
    if attr.fields.len() < keys.len() {
        return Ok(false);
    }
    let mut cases: Vec<Option<FieldInfo>> = vec![None; keys.len()];
    for &k in &keys {
        let ordinal = k as usize;
        match (attr.fields.get(ordinal), cases.get_mut(ordinal)) {
            (Some(f), Some(slot)) if k >= 0 => {
                *slot = Some(env.root.classes[enum_cls].fields[f.field].info.clone());
            }
            _ => return Ok(false),
        }
    }
    Ok(execute_replace(cfg, sw, inv_var, &cases))
}

fn process_remapped_enum_switch(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    sw: InsnId,
    aget: InsnId,
    pending: &mut Vec<(usize, usize)>,
) -> Result<bool, String> {
    let Some((inv_arg, map_field)) = check_enum_map_access(env.root, cfg, aget) else {
        return Ok(false);
    };
    let Some(value_map) = env.root.enum_maps.get(&map_field) else {
        return Ok(false);
    };
    let keys = switch_keys(cfg, sw);
    let mut cases = Vec::with_capacity(keys.len());
    for k in &keys {
        match value_map.get(k) {
            Some(f) => cases.push(Some(f.clone())),
            None => return Ok(false),
        }
    }
    if execute_replace(cfg, sw, inv_arg, &cases) {
        pending.push(map_field);
        return Ok(true);
    }
    Ok(false)
}

/// Hide the map field and, if all static final synthetic fields are hidden, the whole class.
pub fn hide_enum_map_field(root: &mut Root, field: (usize, usize)) {
    root.classes[field.0].fields[field.1].dont_generate = true;
    let cls = &root.classes[field.0];
    for f in &cls.fields {
        if f.access & ACC_SYNTHETIC != 0
            && f.access & ACC_STATIC != 0
            && f.access & ACC_FINAL != 0
            && !f.dont_generate
        {
            return;
        }
    }
    root.classes[field.0].dont_generate = true;
}
