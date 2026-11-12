//! `ConstInlineVisitor`: inline constant registers into the instructions using them.
//!
//! Literals and strings are replaced by constant field references where the constant storage
//! has a field for the value.

use crate::model::cfg::MethodCfg;
use crate::model::info::MethodInfo;
use crate::model::insn::{
    iflag, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType, InvokeType, LiteralArg,
};
use crate::model::ssa::{ArgRef, SsaId};
use crate::model::types::{ArgType, PrimitiveType};
use crate::passes::constructor::MethodEnv;
use crate::passes::mod_visitor::{
    add_field_usage, get_const_field, get_const_field_literal, get_const_field_str,
};
use jadx_input_api::ConstValue;

pub fn const_inline(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    for bid in cfg.blocks.clone() {
        let mut to_remove: Vec<InsnId> = Vec::new();
        for iid in cfg.block(bid).insns.clone() {
            check_insn(cfg, env, iid, &mut to_remove)?;
        }
        // InsnRemover.removeAllAndUnbind(mth, block, toRemove)
        for &i in &to_remove {
            cfg.unbind_all_args(i);
        }
        for &i in &to_remove {
            cfg.unbind_result(i)?;
            cfg.insns[i].flags |= iflag::DONT_GENERATE;
        }
        for &rem in &to_remove {
            if let Some(p) = cfg.arena[bid].insns.iter().position(|i| *i == rem) {
                cfg.arena[bid].insns.remove(p);
            }
        }
    }
    Ok(())
}

fn check_insn(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    insn: InsnId,
    to_remove: &mut Vec<InsnId>,
) -> Result<(), String> {
    {
        let n = cfg.insn(insn);
        if n.contains_flag(iflag::DONT_INLINE | iflag::DONT_GENERATE) || n.result.is_none() {
            return Ok(());
        }
    }
    let Some(var) = cfg.insn(insn).result.as_ref().and_then(|r| r.svar) else {
        return Ok(());
    };
    let const_arg: InsnArg = match cfg.insn(insn).ty {
        InsnType::Const | InsnType::Move => {
            let a = cfg.insn(insn).args[0].clone();
            let InsnArg::Literal(l) = &a else {
                return Ok(());
            };
            if l.literal == 0 && forbid_null_inlines(cfg, var) {
                return Ok(());
            }
            a
        }
        InsnType::ConstStr => {
            let InsnKind::ConstStr(value) = cfg.insn(insn).kind.clone() else {
                return Ok(());
            };
            match get_const_field_str(env, &value) {
                None => {
                    let copy = cfg.copy_without_result(insn);
                    cfg.wrap_arg(copy)?
                }
                Some(f) => {
                    let get = InsnNode::new(
                        InsnType::Sget,
                        InsnKind::Index(IndexValue::Field(f.info.clone())),
                    );
                    let id = cfg.add_insn(get);
                    let mut arg = cfg.wrap_arg(id)?;
                    if let InsnArg::Wrapped(w) = &mut arg {
                        w.ty = ArgType::string();
                    }
                    if replace_const(cfg, env, insn, &arg)? {
                        to_remove.push(insn);
                        add_field_usage(cfg, &f);
                    }
                    return Ok(());
                }
            }
        }
        InsnType::ConstClass => {
            if cfg.svars[var].is_used_in_phi() {
                return Ok(());
            }
            let copy = cfg.copy_without_result(insn);
            cfg.wrap_arg(copy)?
        }
        _ => return Ok(()),
    };
    if replace_const(cfg, env, insn, &const_arg)? {
        to_remove.push(insn);
    }
    Ok(())
}

fn forbid_null_inlines(cfg: &mut MethodCfg, var: SsaId) -> bool {
    let uses = cfg.svars[var].uses.clone();
    if uses.is_empty() {
        return false;
    }
    let mut k = 0;
    for u in &uses {
        if let Some(insn) = u.parent_insn() {
            if forbid_null_arg_inline(cfg, insn, *u) {
                k += 1;
            }
        }
    }
    k == uses.len()
}

fn forbid_null_arg_inline(cfg: &mut MethodCfg, insn: InsnId, use_arg: ArgRef) -> bool {
    if cfg.insn(insn).ty == InsnType::Move {
        let Some(v) = cfg.insn(insn).result.as_ref().and_then(|r| r.svar) else {
            return false;
        };
        return forbid_null_inlines(cfg, v);
    }
    if !can_use_null(cfg.insn(insn), use_arg) {
        cfg.reg_mut(use_arg).dont_inline_const = true;
        return true;
    }
    false
}

fn can_use_null(insn: &InsnNode, use_arg: ArgRef) -> bool {
    let ArgRef::Arg(id, k) = use_arg else {
        return true;
    };
    let _ = id;
    match insn.ty {
        InsnType::Invoke => {
            let is_static = matches!(
                &insn.kind,
                InsnKind::Invoke {
                    invoke_type: InvokeType::Static,
                    ..
                }
            );
            is_static || k != 0
        }
        InsnType::ArrayLength
        | InsnType::Aget
        | InsnType::Aput
        | InsnType::Iget
        | InsnType::Switch
        | InsnType::MonitorEnter
        | InsnType::MonitorExit
        | InsnType::InstanceOf => k != 0,
        InsnType::Iput => k != 1,
        _ => true,
    }
}

fn is_linked_to_other_ssa_vars(cfg: &MethodCfg, at: ArgRef) -> bool {
    let Some(v) = cfg.reg(at).svar else {
        return false;
    };
    match cfg.svars[v].code_var {
        Some(cv) => cfg.code_vars[cv].ssa_vars.len() > 1,
        None => false,
    }
}

fn replace_const(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    const_insn: InsnId,
    const_arg: &InsnArg,
) -> Result<bool, String> {
    let Some(var) = cfg.insn(const_insn).result.as_ref().and_then(|r| r.svar) else {
        return Ok(false);
    };
    if cfg.svars[var].use_count() == 0 {
        return Ok(true);
    }
    let uses = cfg.svars[var].uses.clone();
    let mut replace_count = 0;
    for at in &uses {
        if can_inline(cfg, env, *at) && replace_arg(cfg, env, *at, const_arg, const_insn)? {
            replace_count += 1;
        }
    }
    if replace_count == uses.len() {
        return Ok(true);
    }
    let remaining = cfg.svars[var].uses.clone();
    if remaining.iter().all(|u| can_ignore_insn(cfg, *u)) {
        cfg.insns[const_insn].flags |= iflag::DONT_GENERATE;
    }
    Ok(false)
}

fn can_ignore_insn(cfg: &MethodCfg, reg: ArgRef) -> bool {
    let Some(parent) = reg.parent_insn() else {
        return false;
    };
    if cfg.insn(parent).ty == InsnType::Phi {
        return false;
    }
    if is_linked_to_other_ssa_vars(cfg, reg) {
        return false;
    }
    cfg.insn(parent).contains_flag(iflag::DONT_GENERATE)
}

fn can_inline(cfg: &MethodCfg, env: &MethodEnv, at: ArgRef) -> bool {
    if cfg.reg(at).dont_inline_const {
        return false;
    }
    let Some(parent) = at.parent_insn() else {
        return false;
    };
    if cfg.insn(parent).contains_flag(iflag::DONT_GENERATE) {
        return false;
    }
    if is_linked_to_other_ssa_vars(cfg, at) {
        let used_in_phi = cfg
            .reg(at)
            .svar
            .is_some_and(|v| cfg.svars[v].is_used_in_phi());
        if !used_in_phi {
            return false;
        }
    }
    if cfg.insn(parent).ty == InsnType::Constructor {
        if let Some(crate::method_utils::MethodDetails::App(c, m)) =
            crate::method_utils::insn_method_details(env.root, cfg.insn(parent))
        {
            let mth = &env.root.classes[c].methods[m];
            if mth.inline_candidate || mth.anonymous_constructor {
                return false;
            }
        }
    }
    true
}

fn replace_arg(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    at: ArgRef,
    const_arg: &InsnArg,
    const_insn: InsnId,
) -> Result<bool, String> {
    let ArgRef::Arg(use_insn, k) = at else {
        return Ok(false);
    };
    if cfg.insn(use_insn).ty == InsnType::Phi {
        return Ok(false);
    }
    match const_arg {
        InsnArg::Literal(l) => {
            let literal = l.literal;
            let mut arg_type = cfg.reg_type(at);
            if arg_type == ArgType::unknown_any() {
                arg_type = cfg.reg(at).init_type.clone();
            }
            if arg_type.is_object() && literal != 0 {
                arg_type = ArgType::narrow_numbers();
            }
            let lit = InsnArg::lit(literal, arg_type);
            let lit_type = match &lit {
                InsnArg::Literal(l) => l.ty.clone(),
                _ => ArgType::unknown_any(),
            };
            cfg.replace_arg(use_insn, k, lit);
            // arg replaced, made some optimizations
            let field = if lit_type.is_type_known() {
                get_const_field_literal(
                    env,
                    &LiteralArg {
                        literal,
                        ty: lit_type.clone(),
                        explicit_primitive: false,
                    },
                )
            } else if lit_type.contains(PrimitiveType::Int) {
                get_const_field(env, &ConstValue::Int(literal as i32), false)
            } else {
                None
            };
            match field {
                Some(f) => {
                    let get = InsnNode::new(
                        InsnType::Sget,
                        InsnKind::Index(IndexValue::Field(f.info.clone())),
                    );
                    let id = cfg.add_insn(get);
                    let InsnArg::Wrapped(mut w) = cfg.wrap_arg(id)? else {
                        return Ok(false);
                    };
                    w.ty = lit_type;
                    cfg.replace_arg(use_insn, k, InsnArg::Wrapped(w));
                    add_field_usage(cfg, &f);
                }
                None => add_explicit_cast(cfg, use_insn, k)?,
            }
        }
        other => {
            let dup = cfg.duplicate_arg(other);
            cfg.replace_arg(use_insn, k, dup);
        }
    }
    cfg.inherit_metadata(use_insn, const_insn);
    Ok(true)
}

fn call_info(insn: &InsnNode) -> Option<(&MethodInfo, usize, bool)> {
    match &insn.kind {
        InsnKind::Invoke { invoke_type, mth } => {
            let is_static = *invoke_type == InvokeType::Static;
            Some((mth, usize::from(!is_static), !is_static))
        }
        InsnKind::Constructor { call_mth, .. } => Some((call_mth, 0, false)),
        _ => None,
    }
}

fn add_explicit_cast(cfg: &mut MethodCfg, insn: InsnId, k: usize) -> Result<(), String> {
    let Some((call_mth, offset, has_instance)) = call_info(cfg.insn(insn)) else {
        return Ok(());
    };
    let call_mth = call_mth.clone();
    let InsnArg::Literal(lit) = cfg.insn(insn).args[k].clone() else {
        return Ok(());
    };
    if has_instance && k == 0 {
        if lit.literal != 0 {
            return Err("Unexpected instance arg in invoke".into());
        }
        let cast_type = call_mth.decl_class.arg_type().clone();
        let mut cast = InsnNode::new(
            InsnType::Cast,
            InsnKind::Index(IndexValue::Type(cast_type.clone())),
        );
        cast.args.push(InsnArg::Literal(lit));
        cast.flags |= iflag::EXPLICIT_CAST;
        let cast_id = cfg.add_insn(cast);
        let wrapped = cfg.wrap_arg(cast_id)?;
        let InsnArg::Wrapped(mut w) = wrapped else {
            return Ok(());
        };
        w.ty = cast_type;
        cfg.replace_arg(insn, k, InsnArg::Wrapped(w));
    } else {
        let Some(arg_type) = call_mth.arg_types.get(k - offset) else {
            return Ok(());
        };
        if arg_type.is_primitive() {
            if let InsnArg::Literal(l) = &mut cfg.insns[insn].args[k] {
                l.ty = arg_type.clone();
                if *arg_type == ArgType::byte() {
                    l.explicit_primitive = true;
                }
            }
        }
    }
    let _ = LiteralArg::with_fixed_type;
    Ok(())
}
