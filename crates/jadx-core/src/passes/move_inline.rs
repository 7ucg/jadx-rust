//! `MoveInlineVisitor`: inline redundant move instructions.

use crate::model::cfg::MethodCfg;
use crate::model::insn::{iflag, InsnArg, InsnId, InsnType};
use crate::model::ssa::ArgRef;

pub fn move_inline(cfg: &mut MethodCfg) -> Result<(), String> {
    for bid in cfg.blocks.clone() {
        let mut to_remove: Vec<InsnId> = Vec::new();
        for iid in cfg.block(bid).insns.clone() {
            if cfg.insn(iid).ty != InsnType::Move {
                continue;
            }
            if process_move(cfg, iid) {
                to_remove.push(iid);
                cfg.unbind_insn(iid)?;
            }
        }
        // InsnRemover.perform
        for &i in &to_remove {
            cfg.unbind_all_args(i);
        }
        for &i in &to_remove {
            cfg.unbind_result(i)?;
            cfg.insns[i].flags |= iflag::DONT_GENERATE;
        }
        cfg.arena[bid].insns.retain(|i| !to_remove.contains(i));
    }
    Ok(())
}

fn process_move(cfg: &mut MethodCfg, mv: InsnId) -> bool {
    let Some(res) = cfg.insn(mv).result.clone() else {
        return false;
    };
    let move_arg = cfg.insn(mv).args[0].clone();
    if let InsnArg::Register(r) = &move_arg {
        // sameRegAndSVar
        if r.reg_num == res.reg_num && r.svar == res.svar {
            return true;
        }
        if let Some(v) = r.svar {
            if cfg.svar_assign_in_phi(v) {
                return false;
            }
        }
    }
    let Some(var) = res.svar else { return false };
    if cfg.svars[var].uses.is_empty() {
        return true;
    }
    if cfg.svars[var].is_used_in_phi() {
        return false;
    }
    if cfg.svars[var]
        .uses
        .iter()
        .any(|u| u.parent_insn().is_none())
    {
        return false;
    }
    for use_arg in cfg.svars[var].uses.clone() {
        let ArgRef::Arg(use_insn, k) = use_arg else {
            continue;
        };
        let replace = match &move_arg {
            InsnArg::Register(r) => {
                let use_reg = cfg.reg(use_arg);
                let mut dup = r.clone();
                dup.init_type = use_reg.init_type.clone();
                dup.immutable_type |= use_reg.immutable_type;
                dup.is_this |= use_reg.is_this;
                dup.method_arg |= use_reg.method_arg;
                InsnArg::Register(dup)
            }
            other => other.clone(),
        };
        cfg.inherit_metadata(use_insn, mv);
        cfg.replace_arg(use_insn, k, replace);
    }
    true
}
