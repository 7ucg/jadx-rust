//! `AnonymousClassVisitor`: prepare the constructor of an anonymous class for inlining at its
//! single use: arguments stored to synthetic fields are skipped at the call and the fields are
//! replaced by the constructor argument.

use std::collections::HashMap;

use crate::model::insn::{iflag, IndexValue, InsnId, InsnKind, InsnType};
use crate::model::node::{FieldReplace, MethodId, Root, ACC_SYNTHETIC};
use crate::model::ssa::ArgRef;
use crate::passes::driver::MethodState;

pub fn anonymous_class_visitor(
    root: &mut Root,
    methods: &mut HashMap<MethodId, MethodState>,
    cls: usize,
) -> Result<(), String> {
    if root.classes[cls].anonymous.is_none() {
        return Ok(());
    }
    let Some(mi) = root.classes[cls]
        .methods
        .iter()
        .position(|m| m.anonymous_constructor)
    else {
        return Ok(());
    };
    process_anonymous_constructor(root, methods, cls, mi)
}

fn process_anonymous_constructor(
    root: &mut Root,
    methods: &mut HashMap<MethodId, MethodState>,
    cls: usize,
    mi: usize,
) -> Result<(), String> {
    let Some(state) = methods.get_mut(&(cls, mi)) else {
        return Ok(());
    };
    let Some(cfg) = state.cfg.as_mut() else {
        return Ok(());
    };
    let mut used_insns: Vec<InsnId> = Vec::new();
    let args_map = get_args_to_fields_mapping(root, cfg, cls, mi, &mut used_insns)?;
    if args_map.is_empty() {
        root.classes[cls].methods[mi].no_skip_args = true;
    } else {
        let args_count = root.classes[cls].methods[mi].info.arg_types.len();
        for (arg_idx, field_idx) in args_map {
            let field = &mut root.classes[cls].fields[field_idx];
            field.replace = Some(FieldReplace::Var {
                ctor: mi,
                arg: arg_idx,
            });
            field.dont_generate = true;
            let skip = root.classes[cls].methods[mi]
                .skip_args
                .get_or_insert_with(|| vec![false; args_count]);
            if arg_idx < skip.len() {
                skip[arg_idx] = true;
            }
        }
    }
    for i in used_insns {
        cfg.insns[i].flags |= iflag::DONT_GENERATE;
    }
    Ok(())
}

/// Method argument index to the index of the synthetic field it is stored to.
fn get_args_to_fields_mapping(
    root: &Root,
    cfg: &crate::model::cfg::MethodCfg,
    cls: usize,
    mi: usize,
    used_insns: &mut Vec<InsnId>,
) -> Result<Vec<(usize, usize)>, String> {
    let call_mth = &root.classes[cls].methods[mi];
    let use_in = call_mth
        .use_in
        .first()
        .ok_or_else(|| "anonymous constructor without use".to_string())?;
    let outer_cls = use_in.0;
    let mut start_arg = 0;
    if !call_mth.info.arg_types.is_empty()
        && call_mth.info.arg_types[0] == *root.classes[outer_cls].info.arg_type()
    {
        start_arg = 1;
    }
    let mut map: Vec<(usize, usize)> = Vec::new();
    for i in start_arg..cfg.arg_regs.len() {
        let Some(var) = cfg.arg_regs[i].svar else {
            return Err("method argument without variable".to_string());
        };
        let Some(use_insn) = get_parent_insn_skip_move(cfg, var) else {
            return Ok(Vec::new());
        };
        match cfg.insn(use_insn).ty {
            InsnType::Iput => {
                let InsnKind::Index(IndexValue::Field(f)) = &cfg.insn(use_insn).kind else {
                    return Ok(Vec::new());
                };
                let Some(fi) = root.classes[cls].fields.iter().position(|x| x.info == *f) else {
                    return Ok(Vec::new());
                };
                if root.classes[cls].fields[fi].access & ACC_SYNTHETIC == 0 {
                    return Ok(Vec::new());
                }
                map.push((i, fi));
                used_insns.push(use_insn);
            }
            InsnType::Constructor => {
                let is_super = matches!(
                    cfg.insn(use_insn).kind,
                    InsnKind::Constructor {
                        call_type: crate::model::insn::CallType::Super,
                        ..
                    }
                );
                if !is_super {
                    return Ok(Vec::new());
                }
                used_insns.push(use_insn);
            }
            _ => return Ok(Vec::new()),
        }
    }
    Ok(map)
}

fn get_parent_insn_skip_move(
    cfg: &crate::model::cfg::MethodCfg,
    var: crate::model::ssa::SsaId,
) -> Option<InsnId> {
    let sv = &cfg.svars[var];
    if sv.use_count() != 1 {
        return None;
    }
    let use_arg: ArgRef = sv.uses[0];
    let parent = use_arg.parent_insn()?;
    if cfg.insn(parent).ty == InsnType::Move {
        let next = cfg.insn(parent).result.as_ref()?.svar?;
        return get_parent_insn_skip_move(cfg, next);
    }
    Some(parent)
}
