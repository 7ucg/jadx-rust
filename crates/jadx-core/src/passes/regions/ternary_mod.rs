//! `TernaryMod`: convert `if` regions to ternary operations.

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::condition::{is_false, is_true, CondMode, IfCondition};
use crate::model::insn::{InsnArg, InsnId, InsnKind, InsnNode, InsnType, RegisterArg};
use crate::model::region::{Container, RegionId, RegionKind};
use crate::model::ssa::{ArgRef, SsaId};
use crate::passes::code_shrink::code_shrink;
use crate::passes::constructor::MethodEnv;
use crate::passes::insn_utils::{is_const_insn, rebind_args, wrap_insn_into_arg};
use crate::passes::regions::maker::Res;
use crate::passes::regions::traversal::{traverse_iterative, traverse_method, RegionVisitor};
use crate::passes::regions::utils::cflag;

const ACC_ENUM: u32 = 0x4000;

struct TernaryVisitor<'a> {
    env: &'a MethodEnv<'a>,
}

impl RegionVisitor for TernaryVisitor<'_> {
    fn enter_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
        if process_region(cfg, self.env, region)? {
            cfg.regions.request_code_shrink = true;
        }
        Ok(true)
    }
}

/// `TernaryMod.process`
pub fn ternary_mod(cfg: &mut MethodCfg, env: &MethodEnv) -> Res<()> {
    let mut changed = false;
    // convert all found ternary nodes in one iteration
    traverse_method(cfg, &mut TernaryVisitor { env })?;
    if cfg.regions.request_code_shrink {
        cfg.regions.request_code_shrink = false;
        code_shrink(cfg)?;
        changed = true;
    }
    if changed && env.root.classes[env.cls].methods[env.mth].is_constructor() {
        // aggressive mode to help code inline before super call in constructor
        // iterative runs with shrink after each change
        traverse_iterative(cfg, &mut |cfg, region| {
            if process_region(cfg, env, region)? {
                code_shrink(cfg)?;
                return Ok(true);
            }
            Ok(false)
        })?;
    }
    Ok(())
}

fn process_region(cfg: &mut MethodCfg, env: &MethodEnv, region: RegionId) -> Res<bool> {
    if matches!(cfg.regions.arena[region].kind, RegionKind::If { .. }) {
        return make_ternary_insn(cfg, env, region);
    }
    Ok(false)
}

fn if_parts(
    cfg: &MethodCfg,
    region: RegionId,
) -> (
    Option<IfCondition>,
    Vec<usize>,
    Option<Container>,
    Option<Container>,
) {
    match &cfg.regions.arena[region].kind {
        RegionKind::If {
            cond,
            cond_blocks,
            then_r,
            else_r,
        } => (cond.clone(), cond_blocks.clone(), *then_r, *else_r),
        _ => (None, Vec::new(), None, None),
    }
}

/// Block of a region that contains exactly one block with exactly one instruction.
fn get_ternary_insn_block(cfg: &MethodCfg, c: Container) -> Option<usize> {
    if let Container::Region(r) = c {
        if let RegionKind::Plain { blocks } = &cfg.regions.arena[r].kind {
            if blocks.len() == 1 {
                if let Container::Block(b) = blocks[0] {
                    if cfg.block(b).insns.len() == 1 {
                        return Some(b);
                    }
                }
            }
        }
    }
    None
}

fn replace_in_parent(cfg: &mut MethodCfg, region: RegionId, header: usize) -> Res<bool> {
    let Some(parent) = cfg.regions.arena[region].parent else {
        return Err("NullPointerException: if region parent".to_string());
    };
    Ok(cfg
        .regions
        .replace_sub_block(parent, Container::Region(region), Container::Block(header)))
}

fn list_remove(cfg: &mut MethodCfg, block: usize, insn: InsnId) {
    if let Some(p) = cfg.block(block).insns.iter().position(|i| *i == insn) {
        cfg.block_mut(block).insns.remove(p);
    }
}

fn clear_condition_blocks(cfg: &mut MethodCfg, cond_blocks: &[usize], header: usize) {
    for &b in cond_blocks {
        if b != header {
            cfg.block_mut(b).insns.clear();
            cfg.block_mut(b).add(bflag::REMOVE);
        }
    }
}

fn only_one_use_in_phi(cfg: &MethodCfg, var: SsaId) -> Option<InsnId> {
    match &cfg.svars[var].used_in_phi {
        Some(l) if l.len() == 1 => Some(l[0]),
        _ => None,
    }
}

/// `new TernaryInsn(condition, result, th, els)`
fn new_ternary(
    cfg: &mut MethodCfg,
    cond: IfCondition,
    result: Option<RegisterArg>,
    th: InsnArg,
    els: InsnArg,
) -> InsnId {
    let (cond, a, b) = if is_false(&th) && is_true(&els) {
        // inverted
        (IfCondition::invert(cfg, &cond), els, th)
    } else {
        (cond, th, els)
    };
    let mut n = InsnNode::new(InsnType::Ternary, InsnKind::TernaryCond(Box::new(cond)));
    n.result = result;
    n.args.push(a);
    n.args.push(b);
    cfg.add_insn(n)
}

/// `TernaryInsn.simplifyCondition`
fn simplify_ternary_condition(cfg: &mut MethodCfg, ternary: InsnId) {
    let cond = match &cfg.insn(ternary).kind {
        InsnKind::TernaryCond(c) => (**c).clone(),
        _ => return,
    };
    let (mut cond, _) = IfCondition::simplify(cfg, &cond);
    if cond.mode == CondMode::Not {
        // invert()
        cond = IfCondition::invert(cfg, &cond);
        cfg.insns[ternary].args.swap(0, 1);
    }
    cfg.insns[ternary].kind = InsnKind::TernaryCond(Box::new(cond));
}

fn make_ternary_insn(cfg: &mut MethodCfg, env: &MethodEnv, region: RegionId) -> Res<bool> {
    if cflag(cfg, Container::Region(region), bflag::ELSE_IF_CHAIN) {
        return Ok(false);
    }
    let (cond, cond_blocks, then_r, else_r) = if_parts(cfg, region);
    let Some(then_r) = then_r else {
        return Ok(false);
    };
    let Some(else_r) = else_r else {
        return process_one_branch_ternary(cfg, env, region);
    };
    let (Some(tb), Some(eb)) = (
        get_ternary_insn_block(cfg, then_r),
        get_ternary_insn_block(cfg, else_r),
    ) else {
        return Ok(false);
    };
    if cfg.block(tb).contains(bflag::DUPLICATED) || cfg.block(eb).contains(bflag::DUPLICATED) {
        return Ok(false);
    }
    if cond_blocks.is_empty() {
        return Ok(false);
    }
    let header = cond_blocks[0];
    let then_insn = cfg.block(tb).insns[0];
    let else_insn = cfg.block(eb).insns[0];
    let cond = cond.ok_or("NullPointerException: if condition")?;
    let then_res = cfg.insn(then_insn).result.clone();
    let else_res = cfg.insn(else_insn).result.clone();
    if let (Some(then_res), Some(else_res)) = (then_res, else_res) {
        let (Some(tv), Some(ev)) = (then_res.svar, else_res.svar) else {
            return Ok(false);
        };
        let then_phi = only_one_use_in_phi(cfg, tv);
        let else_phi = only_one_use_in_phi(cfg, ev);
        let Some(then_phi) = then_phi else {
            return Ok(false);
        };
        if Some(then_phi) != else_phi {
            return Ok(false);
        }
        if !replace_in_parent(cfg, region, header)? {
            return Ok(false);
        }
        list_remove(cfg, tb, then_insn);
        list_remove(cfg, eb, else_insn);
        let res_arg = if cfg.insn(then_phi).args.len() == 2 {
            cfg.insn(then_phi)
                .result
                .clone()
                .ok_or("phi without result")?
        } else {
            // `thenPhi.removeArg(elseResArg)` never finds the result argument among the phi
            // arguments, so nothing is removed
            then_res.clone()
        };
        let then_arg_insn = cfg.copy_without_result(then_insn);
        let then_arg = wrap_insn_into_arg(cfg, then_arg_insn)?;
        let else_arg_insn = cfg.copy_without_result(else_insn);
        let else_arg = wrap_insn_into_arg(cfg, else_arg_insn)?;
        let tern = new_ternary(cfg, cond, Some(res_arg), then_arg, else_arg);
        cfg.unbind_insn(then_insn)?;
        cfg.unbind_insn(else_insn)?;
        rebind_args(cfg, tern)?;
        if cfg.insn(then_phi).args.is_empty() {
            cfg.unbind_result(then_phi)?;
            delist_phi(cfg, then_phi);
        }
        // remove 'if' instruction
        cfg.block_mut(header).insns.clear();
        cfg.block_mut(header).insns.push(tern);
        clear_condition_blocks(cfg, &cond_blocks, header);
        return Ok(true);
    }
    let then_ty = cfg.insn(then_insn).ty;
    let else_ty = cfg.insn(else_insn).ty;
    if !cfg.void_return && then_ty == InsnType::Return && else_ty == InsnType::Return {
        let then_arg = cfg.insn(then_insn).args[0].clone();
        let else_arg = cfg.insn(else_insn).args[0].clone();
        if matches!(then_arg, InsnArg::Literal(_)) != matches!(else_arg, InsnArg::Literal(_)) {
            // one arg is literal
            return Ok(false);
        }
        if !replace_in_parent(cfg, region, header)? {
            return Ok(false);
        }
        let then_type = cfg.arg_type(ArgRef::Arg(then_insn, 0));
        list_remove(cfg, tb, then_insn);
        list_remove(cfg, eb, else_insn);
        cfg.block_mut(tb).flags &= !bflag::RETURN;
        cfg.block_mut(eb).flags &= !bflag::RETURN;
        let tern = new_ternary(cfg, cond, None, then_arg, else_arg);
        // the arguments move from the return instructions into the ternary
        for (k, from) in [then_insn, else_insn].into_iter().enumerate() {
            // (the constructor only swaps literal arguments, which have no uses)
            move_use(cfg, from, tern, k);
        }
        let mut ret = InsnNode::plain(InsnType::Return);
        let wrapped = cfg.wrap_arg(tern)?;
        let wrapped = match wrapped {
            InsnArg::Wrapped(mut w) => {
                w.ty = then_type;
                InsnArg::Wrapped(w)
            }
            other => other,
        };
        ret.args.push(wrapped);
        let ret_id = cfg.add_insn(ret);
        cfg.block_mut(header).insns.clear();
        rebind_args(cfg, ret_id)?;
        cfg.block_mut(header).insns.push(ret_id);
        cfg.block_mut(header).add(bflag::RETURN);
        clear_condition_blocks(cfg, &cond_blocks, header);
        return Ok(true);
    }
    Ok(false)
}

/// Move the use of argument 0 of `from` to argument `slot` of `to`.
fn move_use(cfg: &mut MethodCfg, from: InsnId, to: InsnId, slot: usize) {
    if let InsnArg::Register(r) = &cfg.insn(to).args[slot].clone() {
        if let Some(v) = r.svar {
            for u in cfg.svars[v].uses.iter_mut() {
                if *u == ArgRef::Arg(from, 0) {
                    *u = ArgRef::Arg(to, slot);
                }
            }
        }
    }
}

fn delist_phi(cfg: &mut MethodCfg, phi: InsnId) {
    for b in cfg.blocks.clone() {
        if let Some(l) = &mut cfg.block_mut(b).phi_list {
            l.retain(|i| *i != phi);
        }
    }
}

/// Convert one variable change with only 'then' branch:
/// `if (c) {r = a;}` to `r = c ? a : r`; convert if `r` used only once
fn process_one_branch_ternary(cfg: &mut MethodCfg, env: &MethodEnv, region: RegionId) -> Res<bool> {
    let (_, _, then_r, _) = if_parts(cfg, region);
    let Some(then_r) = then_r else {
        return Ok(false);
    };
    if let Some(block) = get_ternary_insn_block(cfg, then_r) {
        let insn = cfg.block(block).insns[0];
        if cfg.insn(insn).result.is_some() {
            replace_with_ternary(cfg, env, region, block, insn)?;
        }
    }
    Ok(false)
}

fn replace_with_ternary(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    region: RegionId,
    block: usize,
    insn: InsnId,
) -> Res<()> {
    let Some(res_arg) = cfg.insn(insn).result.clone() else {
        return Ok(());
    };
    let Some(rv) = res_arg.svar else {
        return Ok(());
    };
    if cfg.svars[rv].uses.len() != 1 {
        return Ok(());
    }
    let Some(phi) = only_one_use_in_phi(cfg, rv) else {
        return Ok(());
    };
    if cfg.insn(phi).args.len() != 2 {
        return Ok(());
    }
    let mut other_arg: Option<RegisterArg> = None;
    for a in &cfg.insn(phi).args {
        if let InsnArg::Register(r) = a {
            if !(r.reg_num == res_arg.reg_num && r.svar == res_arg.svar) {
                other_arg = Some(r.clone());
                break;
            }
        }
    }
    let Some(other_arg) = other_arg else {
        return Ok(());
    };
    let else_assign = other_arg.svar.and_then(|v| cfg.svar_assign_insn(v));
    let else_is_const = else_assign.is_some_and(|e| is_const_insn(cfg.insn(e).ty));
    let m = &env.root.classes[env.cls].methods[env.mth];
    let skip_checks = m.is_constructor()
        || (env.root.classes[env.cls].access & ACC_ENUM != 0 && m.info.is_class_init());
    if !skip_checks && !else_is_const {
        let same = match (res_arg.svar, other_arg.svar) {
            (Some(a), Some(b)) => cfg.svars[a].code_var == cfg.svars[b].code_var,
            _ => false,
        };
        if same {
            // don't use same variable in else branch to prevent: l = (l == 0) ? 1 : l
            return Ok(());
        }
    }
    // all checks passed
    let (cond, cond_blocks, _, _) = if_parts(cfg, region);
    let cond = cond.ok_or("NullPointerException: if condition")?;
    let header = cond_blocks[0];
    if !replace_in_parent(cfg, region, header)? {
        return Ok(());
    }
    let else_arg = if else_is_const {
        let ea = else_assign.expect("const assign");
        // inline constant
        let copy = cfg.copy_without_result(ea);
        let arg = wrap_insn_into_arg(cfg, copy)?;
        let else_var = cfg
            .insn(ea)
            .result
            .as_ref()
            .and_then(|r| r.svar)
            .ok_or("NullPointerException: else var")?;
        if cfg.svars[else_var].uses.len() == 1 && only_one_use_in_phi(cfg, else_var) == Some(phi) {
            cfg.remove_insn(ea)?;
        }
        arg
    } else {
        InsnArg::Register(other_arg.clone())
    };
    let then_arg = wrap_insn_into_arg(cfg, insn)?;
    let result_arg = cfg
        .insn(phi)
        .result
        .clone()
        .ok_or("NullPointerException: phi result")?;
    let tern = new_ternary(cfg, cond, Some(result_arg), then_arg, else_arg);
    simplify_ternary_condition(cfg, tern);
    cfg.unbind_all_args(phi);
    delist_phi(cfg, phi);
    cfg.unbind_result(insn)?;
    list_remove(cfg, block, insn);
    cfg.block_mut(header).insns.clear();
    rebind_args(cfg, tern)?;
    cfg.block_mut(header).insns.push(tern);
    clear_condition_blocks(cfg, &cond_blocks, header);
    // shrink method again
    code_shrink(cfg)?;
    Ok(())
}
