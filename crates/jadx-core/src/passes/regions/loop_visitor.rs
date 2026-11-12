//! `LoopRegionVisitor`: convert `while` loops to `for` loops (indexed or for-each).

use crate::model::cfg::MethodCfg;
use crate::model::condition::IfCondition;
use crate::model::insn::{
    iflag, ArithOp, IfOp, InsnArg, InsnId, InsnKind, InsnNode, InsnType, InvokeType, RegisterArg,
};
use crate::model::region::{Container, LoopType, RegionId, RegionKind};
use crate::model::ssa::{ArgRef, SsaId};
use crate::model::types::{ArgType, WildcardBound};
use crate::passes::block_utils::get_last_insn;
use crate::passes::constructor::MethodEnv;
use crate::passes::insn_utils::register_args;
use crate::passes::regions::block_ext::{
    arg_equals, block_by_insn_full, search_wrapped_insn_parent, skip_synthetic_predecessor,
};
use crate::passes::regions::loop_maker::loop_pre_header;
use crate::passes::regions::maker::Res;
use crate::passes::regions::shrink_method;
use crate::passes::regions::switch_break::process_if_requested;
use crate::passes::regions::traversal::{traverse_method, RegionVisitor};
use crate::passes::regions::utils::is_region_contains_block;

struct LoopVisitor<'a> {
    env: &'a MethodEnv<'a>,
}

impl RegionVisitor for LoopVisitor<'_> {
    fn enter_region(&mut self, cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
        if matches!(cfg.regions.arena[region].kind, RegionKind::Loop { .. })
            && process_loop_region(cfg, self.env, region)?
        {
            // optimize `if` block after instructions remove
            cfg.regions.request_if_region_optimize = true;
        }
        Ok(true)
    }
}

/// `LoopRegionVisitor.visit`
pub fn loop_region_visitor(cfg: &mut MethodCfg, env: &MethodEnv) -> Res<()> {
    if cfg.blocks.is_empty() {
        return Ok(());
    }
    traverse_method(cfg, &mut LoopVisitor { env })?;
    process_if_requested(cfg, env)
}

fn process_loop_region(cfg: &mut MethodCfg, env: &MethodEnv, region: RegionId) -> Res<bool> {
    let (condition_at_end, cond) = match &cfg.regions.arena[region].kind {
        RegionKind::Loop {
            condition_at_end,
            cond,
            ..
        } => (*condition_at_end, cond.clone()),
        _ => return Ok(false),
    };
    if condition_at_end {
        return Ok(false);
    }
    let Some(cond) = cond else {
        return Ok(false);
    };
    if check_for_indexed_loop(cfg, env, region, &cond)? {
        return Ok(true);
    }
    check_iterable_for_each(cfg, env, region, &cond)
}

fn same_var(a: &RegisterArg, b: &RegisterArg) -> bool {
    a.reg_num == b.reg_num && a.svar == b.svar
}

fn loop_id(cfg: &MethodCfg, region: RegionId) -> usize {
    match &cfg.regions.arena[region].kind {
        RegionKind::Loop { info, .. } => *info,
        _ => 0,
    }
}

/// Check for indexed loop.
fn check_for_indexed_loop(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    region: RegionId,
    cond: &IfCondition,
) -> Res<bool> {
    let loop_end_block = cfg.loop_infos[loop_id(cfg, region)].end;
    let Some(incr_insn) = get_last_insn(cfg, skip_synthetic_predecessor(cfg, loop_end_block))
    else {
        return Ok(false);
    };
    let Some(incr_arg) = cfg.insn(incr_insn).result.clone() else {
        return Ok(false);
    };
    let Some(incr_var) = incr_arg.svar else {
        return Ok(false);
    };
    if !cfg.svars[incr_var].is_used_in_phi() {
        return Ok(false);
    }
    let phi_list = cfg.svars[incr_var].used_in_phi.clone().unwrap_or_default();
    if phi_list.len() != 1 {
        return Ok(false);
    }
    let phi = phi_list[0];
    let phi_args: Vec<RegisterArg> = cfg
        .insn(phi)
        .args
        .iter()
        .filter_map(|a| a.as_register().cloned())
        .collect();
    if cfg.insn(phi).args.len() != 2
        || !phi_args.iter().any(|a| same_var(a, &incr_arg))
        || cfg.svars[incr_var].uses.len() != 1
    {
        return Ok(false);
    }
    let Some(arg) = cfg.insn(phi).result.clone() else {
        return Ok(false);
    };
    let cond_args = cond.register_args(cfg);
    if !cond_args.iter().any(|a| same_var(cfg.reg(*a), &arg)) {
        return Ok(false);
    }
    let Some(arg_var) = arg.svar else {
        return Ok(false);
    };
    if cfg.svars[arg_var].is_used_in_phi() {
        return Ok(false);
    }
    let Some(init_arg) = phi_args.first() else {
        return Ok(false);
    };
    let Some(init_var) = init_arg.svar else {
        return Ok(false);
    };
    let Some(init_insn) = cfg.svar_assign_insn(init_var) else {
        return Ok(false);
    };
    if cfg.insn(init_insn).contains_flag(iflag::DONT_GENERATE)
        || cfg.svars[init_var].uses.len() != 1
    {
        return Ok(false);
    }
    if !used_only_in_loop(cfg, region, arg_var) {
        return Ok(false);
    }
    // can't make loop if argument from increment instruction is assign in loop
    for a in register_args(cfg, incr_insn) {
        let r = cfg.reg(a).clone();
        if assign_only_in_loop(cfg, region, &r, 0)? {
            return Ok(false);
        }
    }
    // all checks passed
    cfg.insns[init_insn].flags |= iflag::DONT_GENERATE;
    cfg.insns[incr_insn].flags |= iflag::DONT_GENERATE;
    let arr_for_each = check_array_for_each(cfg, env, region, init_insn, incr_insn, cond)?;
    let ty = arr_for_each.unwrap_or(LoopType::For {
        init: init_insn,
        incr: incr_insn,
    });
    if let RegionKind::Loop { ty: t, .. } = &mut cfg.regions.arena[region].kind {
        *t = Some(ty);
    }
    Ok(true)
}

fn is_literal(a: &InsnArg, value: i64) -> bool {
    matches!(a, InsnArg::Literal(l) if l.literal == value)
}

fn check_array_for_each(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    region: RegionId,
    init_insn: InsnId,
    incr_insn: InsnId,
    cond: &IfCondition,
) -> Res<Option<LoopType>> {
    let incr = cfg.insn(incr_insn).clone();
    if !matches!(incr.kind, InsnKind::Arith(ArithOp::Add)) || incr.ty != InsnType::Arith {
        return Ok(None);
    }
    if incr.args.len() < 2 || !is_literal(&incr.args[1], 1) {
        return Ok(None);
    }
    let init = cfg.insn(init_insn);
    if init.ty != InsnType::Const || init.args.is_empty() || !is_literal(&init.args[0], 0) {
        return Ok(None);
    }
    let Some(InsnArg::Register(cond_reg)) = incr.args.first() else {
        return Ok(None);
    };
    let Some(var) = cond_reg.svar else {
        return Ok(None);
    };
    let uses = cfg.svars[var].uses.clone();
    if uses.len() != 3 {
        return Ok(None);
    }
    let find_use = |cfg: &MethodCfg, ty: InsnType| {
        uses.iter()
            .copied()
            .find(|u| u.parent_insn().is_some_and(|p| cfg.insn(p).ty == ty))
    };
    let Some(cond_arg) = find_use(cfg, InsnType::If) else {
        return Ok(None);
    };
    let Some(arr_index) = find_use(cfg, InsnType::Aget) else {
        return Ok(None);
    };
    let Some(arr_get_insn) = arr_index.parent_insn() else {
        return Ok(None);
    };
    if cfg
        .insn(arr_get_insn)
        .args
        .iter()
        .any(|a| matches!(a, InsnArg::Wrapped(_)))
    {
        return Ok(None);
    }
    if !cond.is_compare() {
        return Ok(None);
    }
    let Some(compare) = cond.compare else {
        return Ok(None);
    };
    if crate::model::condition::if_op(cfg, compare) != IfOp::Lt
        || cond_arg != ArgRef::Arg(compare, 0)
    {
        return Ok(None);
    }
    let b_cond_arg = cfg.insn(compare).args[1].clone();
    let len = match &b_cond_arg {
        InsnArg::Wrapped(w) => Some(w.insn),
        InsnArg::Register(r) => r.svar.and_then(|v| cfg.svar_assign_insn(v)),
        _ => None,
    };
    let Some(len) = len else {
        return Ok(None);
    };
    if cfg.insn(len).ty != InsnType::ArrayLength {
        return Ok(None);
    }
    let array_arg = cfg.insn(len).args[0].clone();
    if cfg.insn(arr_get_insn).args.is_empty()
        || !arg_equals(cfg, &array_arg, &cfg.insn(arr_get_insn).args[0].clone())
    {
        return Ok(None);
    }
    let iter_var = match cfg.insn(arr_get_insn).result.clone() {
        Some(iter_var) => {
            let Some(iter_svar) = iter_var.svar else {
                return Ok(None);
            };
            if !used_only_in_loop(cfg, region, iter_svar) {
                return Ok(None);
            }
            iter_var
        }
        None => {
            if !cfg.insn(arr_get_insn).contains_flag(iflag::WRAPPED) {
                return Ok(None);
            }
            // create new variable and replace wrapped insn
            let Some((parent_insn, arg_idx)) = search_wrapped_insn_parent(cfg, arr_get_insn) else {
                cfg.warnings.push(format!(
                    "checkArrayForEach: Wrapped insn not found: {arr_get_insn}"
                ));
                return Ok(None);
            };
            let wrap_type = match &cfg.insn(parent_insn).args[arg_idx] {
                InsnArg::Wrapped(w) => w.ty.clone(),
                _ => return Ok(None),
            };
            let (at, var) = cfg.duplicate_with_new_svar(&RegisterArg::new(0, wrap_type.clone()));
            crate::passes::init_code_vars::init_code_var(cfg, var)?;
            cfg.svar_set_type(var, wrap_type)?;
            let iter_var = cfg.reg(at).clone();
            cfg.replace_arg(parent_insn, arg_idx, InsnArg::Register(iter_var.clone()));
            crate::passes::insn_utils::rebind_args(cfg, parent_insn)?;
            iter_var
        }
    };
    // array for each loop confirmed
    if let Some(r) = cfg.insns[incr_insn].result.as_mut() {
        r.dont_generate = true;
    }
    if let Some(r) = cfg.try_reg_mut(cond_arg) {
        r.dont_generate = true;
    }
    if let Some(InsnArg::Register(r)) = cfg.insns[compare].args.get_mut(1) {
        r.dont_generate = true;
    }
    cfg.insns[arr_get_insn].flags |= iflag::DONT_GENERATE;
    cfg.insns[compare].flags |= iflag::DONT_GENERATE;
    let for_each = make_for_each(cfg, &iter_var, &array_arg)?;
    inject_fake_insns(cfg, region, &for_each)?;
    if dont_generate_if_not_used(cfg, len) {
        cfg.remove_insn(len)?;
    }
    shrink_method(cfg)?;
    let _ = env;
    Ok(Some(for_each))
}

/// `InsnUtils.dontGenerateIfNotUsed`
fn dont_generate_if_not_used(cfg: &mut MethodCfg, insn: InsnId) -> bool {
    if let Some(res) = &cfg.insn(insn).result {
        if let Some(v) = res.svar {
            for u in &cfg.svars[v].uses {
                if let Some(p) = u.parent_insn() {
                    if !cfg.insn(p).contains_flag(iflag::DONT_GENERATE) {
                        return false;
                    }
                }
            }
        }
    }
    cfg.insns[insn].flags |= iflag::DONT_GENERATE;
    true
}

/// `new ForEachLoop(varArg, iterableArg)`
fn make_for_each(cfg: &mut MethodCfg, var_arg: &RegisterArg, iterable: &InsnArg) -> Res<LoopType> {
    // store for-each args in fake instructions to save code semantics and allow args manipulations
    let mut var_insn = InsnNode::plain(InsnType::RegionArg);
    var_insn.flags |= iflag::DONT_INLINE;
    let var_id = cfg.add_insn(var_insn);
    cfg.set_result(var_id, Some(var_arg.clone()));
    let mut iter_insn = InsnNode::plain(InsnType::RegionArg);
    iter_insn.flags |= iflag::DONT_INLINE;
    let iter_id = cfg.add_insn(iter_insn);
    let dup = cfg.duplicate_arg(iterable);
    let var = match &dup {
        InsnArg::Register(r) => r.svar,
        _ => None,
    };
    cfg.insns[iter_id].args.push(dup);
    if let Some(v) = var {
        cfg.svar_use(v, ArgRef::Arg(iter_id, 0));
    }
    // will be declared at codegen
    if let Some(v) = var_arg.svar {
        if let Some(cv) = cfg.svars[v].code_var {
            cfg.code_vars[cv].is_declared = true;
        }
    }
    Ok(LoopType::ForEach {
        var_arg_insn: var_id,
        iterable_insn: iter_id,
    })
}

/// `ForEachLoop.injectFakeInsns`
fn inject_fake_insns(cfg: &mut MethodCfg, region: RegionId, lt: &LoopType) -> Res<()> {
    let LoopType::ForEach {
        var_arg_insn,
        iterable_insn,
    } = lt
    else {
        return Ok(());
    };
    let l = loop_id(cfg, region);
    let pre_header = loop_pre_header(cfg, l)?;
    cfg.block_mut(pre_header).insns.push(*iterable_insn);
    let header = match &cfg.regions.arena[region].kind {
        RegionKind::Loop {
            header: Some(h), ..
        } => *h,
        _ => return Err("NullPointerException: loop header".to_string()),
    };
    cfg.block_mut(header).insns.insert(0, *var_arg_insn);
    Ok(())
}

fn check_invoke(
    cfg: &MethodCfg,
    insn: Option<InsnId>,
    decl_cls: Option<&str>,
    mth_id: &str,
) -> bool {
    let Some(insn) = insn else {
        return false;
    };
    let n = cfg.insn(insn);
    if n.ty == InsnType::Invoke {
        if let InsnKind::Invoke { invoke_type, mth } = &n.kind {
            if matches!(invoke_type, InvokeType::Interface | InvokeType::Virtual)
                && mth.short_id() == mth_id
            {
                return match decl_cls {
                    None => true,
                    Some(c) => mth.decl_class.full_name() == c,
                };
            }
        }
    }
    false
}

fn check_iterable_for_each(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    region: RegionId,
    cond: &IfCondition,
) -> Res<bool> {
    let cond_args = cond.register_args(cfg);
    if cond_args.len() != 1 {
        return Ok(false);
    }
    let iterator_arg = cfg.reg(cond_args[0]).clone();
    let Some(var) = iterator_arg.svar else {
        return Ok(false);
    };
    if cfg.svars[var].is_used_in_phi() {
        return Ok(false);
    }
    let it_use_list = cfg.svars[var].uses.clone();
    let assign_insn = cfg.svar_assign_insn(var);
    if it_use_list.len() != 2 {
        return Ok(false);
    }
    if !check_invoke(cfg, assign_insn, None, "iterator()Ljava/util/Iterator;") {
        return Ok(false);
    }
    let assign_insn = assign_insn.expect("checked");
    let Some(iterable_arg) = cfg.insn(assign_insn).args.first().cloned() else {
        return Ok(false);
    };
    let has_next_call = it_use_list[0].parent_insn();
    let next_call = it_use_list[1].parent_insn();
    if !check_invoke(cfg, has_next_call, Some("java.util.Iterator"), "hasNext()Z")
        || !check_invoke(
            cfg,
            next_call,
            Some("java.util.Iterator"),
            "next()Ljava/lang/Object;",
        )
    {
        return Ok(false);
    }
    let next_call = next_call.expect("checked");
    let mut to_skip: Vec<InsnId> = Vec::new();
    let iter_var: RegisterArg;
    if cfg.insn(next_call).contains_flag(iflag::WRAPPED) {
        let Some((parent_insn, arg_idx)) = search_wrapped_insn_parent(cfg, next_call) else {
            return Ok(false);
        };
        let Some(block) = block_by_insn_full(cfg, parent_insn) else {
            return Ok(false);
        };
        if !is_region_contains_block(cfg, Container::Region(region), block) {
            return Ok(false);
        }
        if cfg.insn(parent_insn).ty == InsnType::CheckCast {
            let Some(res) = cfg.insn(parent_insn).result.clone() else {
                return Ok(false);
            };
            if !fix_iterable_type(cfg, env, &iterable_arg, &res) {
                return Ok(false);
            }
            iter_var = res;
            match search_wrapped_insn_parent(cfg, parent_insn) {
                Some((cast_parent, cast_idx)) => {
                    cfg.replace_arg(cast_parent, cast_idx, InsnArg::Register(iter_var.clone()));
                }
                None => {
                    // cast not inlined
                    to_skip.push(parent_insn);
                }
            }
        } else {
            let Some(res) = cfg.insn(next_call).result.clone() else {
                return Ok(false);
            };
            iter_var = res;
            // restore variable from inlined insn
            cfg.insns[next_call].flags |= iflag::DONT_GENERATE;
            if !fix_iterable_type(cfg, env, &iterable_arg, &iter_var) {
                return Ok(false);
            }
            cfg.replace_arg(parent_insn, arg_idx, InsnArg::Register(iter_var.clone()));
        }
    } else {
        let Some(res) = cfg.insn(next_call).result.clone() else {
            return Ok(false);
        };
        iter_var = res;
        let Some(iv) = iter_var.svar else {
            return Ok(false);
        };
        if !used_only_in_loop(cfg, region, iv) {
            return Ok(false);
        }
        if !assign_only_in_loop(cfg, region, &iter_var, 0)? {
            return Ok(false);
        }
        to_skip.push(next_call);
    }
    cfg.insns[assign_insn].flags |= iflag::DONT_GENERATE;
    if let Some(r) = cfg.insns[assign_insn].result.as_mut() {
        r.dont_generate = true;
    }
    for insn in to_skip {
        cfg.insns[insn].result = None;
        cfg.insns[insn].flags |= iflag::DONT_GENERATE;
    }
    for u in it_use_list {
        if let Some(r) = cfg.try_reg_mut(u) {
            r.dont_generate = true;
        }
    }
    let for_each = make_for_each(cfg, &iter_var, &iterable_arg)?;
    inject_fake_insns(cfg, region, &for_each)?;
    if let RegionKind::Loop { ty, .. } = &mut cfg.regions.arena[region].kind {
        *ty = Some(for_each);
    }
    Ok(true)
}

/// `ArgType.isInstanceOf`
fn is_instance_of(env: &MethodEnv, ty: &ArgType, of: &ArgType) -> bool {
    if ty == of {
        return true;
    }
    if !ty.is_object() || !of.is_object() {
        return false;
    }
    match (ty.object_name(), of.object_name()) {
        (Some(a), Some(b)) => env.root.clsp.is_implements(&a, &b),
        _ => false,
    }
}

fn arg_type_of(cfg: &MethodCfg, a: &InsnArg) -> ArgType {
    match a {
        InsnArg::Register(r) => match r.svar {
            Some(v) => cfg.svars[v].ty.clone(),
            None => r.init_type.clone(),
        },
        InsnArg::Literal(l) => l.ty.clone(),
        InsnArg::Wrapped(w) => w.ty.clone(),
        InsnArg::Named(n) => n.ty.clone(),
    }
}

fn fix_iterable_type(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    iterable_arg: &InsnArg,
    iter_var: &RegisterArg,
) -> bool {
    let iterable_type = arg_type_of(cfg, iterable_arg);
    let var_type = match iter_var.svar {
        Some(v) => cfg.svars[v].ty.clone(),
        None => iter_var.init_type.clone(),
    };
    if iterable_type.is_generic() {
        let Some(generic_types) = iterable_type.generic_types() else {
            return false;
        };
        if generic_types.len() != 1 {
            return false;
        }
        let g_type = generic_types[0].clone();
        if g_type == var_type {
            return true;
        }
        if g_type.is_generic_type() {
            if let Some(v) = iter_var.svar {
                let _ = cfg.svar_set_type(v, g_type);
            }
            return true;
        }
        if is_instance_of(env, &g_type, &var_type) {
            return true;
        }
        if let Some(w) = g_type.wildcard_type() {
            if g_type.wildcard_bound() == Some(WildcardBound::Extends)
                && is_instance_of(env, w, &var_type)
            {
                return true;
            }
        }
        return false;
    }
    let InsnArg::Register(ir) = iterable_arg else {
        return true;
    };
    if !iterable_type.is_object() {
        return true;
    }
    let generic_type = ArgType::generic(&iterable_type, vec![var_type.clone()]);
    if let Some(v) = ir.svar {
        if let Some(immutable) = cfg.svar_immutable_type(v) {
            if immutable != generic_type {
                // can't change type
                // allow to iterate over not generified collection only for Object vars
                return var_type == ArgType::object_type();
            }
        }
        let _ = cfg.svar_set_type(v, generic_type);
    }
    true
}

fn assign_only_in_loop(
    cfg: &MethodCfg,
    region: RegionId,
    arg: &RegisterArg,
    depth: usize,
) -> Res<bool> {
    if depth > 500 {
        return Err("LoopRegionVisitor.assignOnlyInLoop endless recursion".to_string());
    }
    let Some(var) = arg.svar else {
        return Ok(true);
    };
    let Some(assign_insn) = cfg.svar_assign_insn(var) else {
        return Ok(true);
    };
    if !arg_in_loop(cfg, region, cfg.svars[var].assign) {
        return Ok(false);
    }
    if cfg.insn(assign_insn).ty == InsnType::Phi {
        for a in &cfg.insn(assign_insn).args {
            if let InsnArg::Register(r) = a {
                if !assign_only_in_loop(cfg, region, r, depth + 1)? {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn used_only_in_loop(cfg: &MethodCfg, region: RegionId, var: SsaId) -> bool {
    cfg.svars[var]
        .uses
        .iter()
        .all(|u| arg_in_loop(cfg, region, *u))
}

fn arg_in_loop(cfg: &MethodCfg, region: RegionId, arg: ArgRef) -> bool {
    let Some(parent) = arg.parent_insn() else {
        return false;
    };
    let Some(block) = block_by_insn_full(cfg, parent) else {
        return false;
    };
    is_region_contains_block(cfg, Container::Region(region), block)
}
