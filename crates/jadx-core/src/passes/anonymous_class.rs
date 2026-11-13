//! `AnonymousClassVisitor`: prepare the constructor of an anonymous class for inlining at its
//! single use: arguments stored to synthetic fields are skipped at the call and the fields are
//! replaced by the constructor argument.

use std::collections::HashMap;

use crate::model::insn::{iflag, IndexValue, InsnId, InsnKind, InsnType};
use crate::model::node::{FieldReplace, MethodId, Root, ACC_SYNTHETIC};
use crate::model::ssa::ArgRef;
use crate::passes::driver::MethodState;

/// Return type of `get_args_to_fields_mapping`: the constructor-argument-to-field mapping, plus
/// the outer-instance field's own `(field index, argument index)`, if found. See that function.
type ArgsFieldsMapping = (Vec<(usize, usize)>, Option<(usize, usize)>);

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
    let (args_map, outer_this) = get_args_to_fields_mapping(root, cfg, cls, mi, &mut used_insns)?;
    if let Some((field_idx, arg_idx)) = outer_this {
        root.classes[cls].fields[field_idx].outer_this_ctor_arg = Some(arg_idx);
    }
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

/// Method argument index to the index of the synthetic field it is stored to; plus, when the
/// first constructor argument is excluded from that mapping because its type is the outer
/// class's (the implicit `this$0`-style outer-instance reference, matching the reference's own
/// `getArgsToFieldsMapping`), the `(field index, argument index)` of the field it initializes
/// (if any), so codegen can later recognize a call-site argument that is actually the caller's
/// own `this` and print it as such instead of a resolved variable name.
fn get_args_to_fields_mapping(
    root: &Root,
    cfg: &crate::model::cfg::MethodCfg,
    cls: usize,
    mi: usize,
    used_insns: &mut Vec<InsnId>,
) -> Result<ArgsFieldsMapping, String> {
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
    let outer_this_field = if start_arg == 1 {
        find_stored_field(root, cfg, cls, 0)
    } else {
        None
    };
    let mut map: Vec<(usize, usize)> = Vec::new();
    for i in start_arg..cfg.arg_regs.len() {
        let Some(var) = cfg.arg_regs[i].svar else {
            return Err("method argument without variable".to_string());
        };
        let Some(use_insn) = get_parent_insn_skip_move(cfg, var) else {
            return Ok((Vec::new(), outer_this_field.map(|fi| (fi, 0))));
        };
        match cfg.insn(use_insn).ty {
            InsnType::Iput => {
                let InsnKind::Index(IndexValue::Field(f)) = &cfg.insn(use_insn).kind else {
                    return Ok((Vec::new(), outer_this_field.map(|fi| (fi, 0))));
                };
                let Some(fi) = root.classes[cls].fields.iter().position(|x| x.info == *f) else {
                    return Ok((Vec::new(), outer_this_field.map(|fi| (fi, 0))));
                };
                if root.classes[cls].fields[fi].access & ACC_SYNTHETIC == 0 {
                    return Ok((Vec::new(), outer_this_field.map(|fi| (fi, 0))));
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
                    return Ok((Vec::new(), outer_this_field.map(|fi| (fi, 0))));
                }
                used_insns.push(use_insn);
            }
            _ => return Ok((Vec::new(), outer_this_field.map(|fi| (fi, 0)))),
        }
    }
    Ok((map, outer_this_field.map(|fi| (fi, 0))))
}

/// The field index that constructor argument `arg_idx` is stored to via a plain `Iput` (no
/// other use, skipping an intermediate `Move`), if any; used only to locate the implicit
/// outer-instance field for the excluded first constructor argument, so its lone `Iput` is left
/// untouched (unlike the mapped arguments in `get_args_to_fields_mapping`, this does not record
/// `used_insns` or otherwise affect inlining).
fn find_stored_field(
    root: &Root,
    cfg: &crate::model::cfg::MethodCfg,
    cls: usize,
    arg_idx: usize,
) -> Option<usize> {
    let var = cfg.arg_regs.get(arg_idx)?.svar?;
    let use_insn = get_parent_insn_skip_move(cfg, var)?;
    if cfg.insn(use_insn).ty != InsnType::Iput {
        return None;
    }
    let InsnKind::Index(IndexValue::Field(f)) = &cfg.insn(use_insn).kind else {
        return None;
    };
    root.classes[cls].fields.iter().position(|x| x.info == *f)
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
