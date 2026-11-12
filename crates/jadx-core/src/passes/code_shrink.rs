//! `CodeShrinkVisitor`: inline single use variables into the instruction using them.

use crate::model::bitset::BitSet;
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{iflag, BlockId, InsnArg, InsnId, InsnKind, InsnType, InvokeType};
use crate::model::ssa::{ArgRef, SsaId};
use crate::passes::block_utils;
use crate::passes::insn_utils::{
    can_reorder, check_last_insn_type, get_all_paths_blocks, is_const_insn, register_args,
    search_arg_with_name, svar_name, wrap_instruction,
};

/// `ArgsInfo`: register arguments of an instruction and the inlining state inside a block.
struct ArgsInfo {
    insn: InsnId,
    /// Register arguments with their register numbers (fixed when the info is built).
    args: Vec<(ArgRef, u32)>,
    inline_border: usize,
    inlined_insn: Option<usize>,
    wrapped_insns: Option<Vec<usize>>,
}

/// `WrapInfo`
struct WrapInfo {
    insn: InsnId,
    arg: ArgRef,
}

/// `CodeShrinkVisitor.shrinkMethod`
pub fn code_shrink(cfg: &mut MethodCfg) -> Result<(), String> {
    for bid in cfg.blocks.clone() {
        shrink_block(cfg, bid)?;
        simplify_move_insns(cfg, bid)?;
    }
    Ok(())
}

fn shrink_block(cfg: &mut MethodCfg, block: BlockId) -> Result<(), String> {
    if cfg.block(block).insns.is_empty() {
        return Ok(());
    }
    let insn_list = cfg.block(block).insns.clone();
    let mut infos: Vec<ArgsInfo> = Vec::with_capacity(insn_list.len());
    for (pos, &insn) in insn_list.iter().enumerate() {
        infos.push(ArgsInfo {
            insn,
            args: get_args(cfg, insn),
            inline_border: pos,
            inlined_insn: None,
            wrapped_insns: None,
        });
    }
    let mut wrap_list: Vec<WrapInfo> = Vec::new();
    for idx in 0..infos.len() {
        let args = infos[idx].args.clone();
        for &(arg, _) in args.iter().rev() {
            check_inline(cfg, block, &insn_list, &mut infos, &mut wrap_list, idx, arg)?;
        }
    }
    for w in &wrap_list {
        inline(cfg, w.arg, w.insn, block)?;
    }
    Ok(())
}

/// `ArgsInfo.getArgs`: the condition of a ternary, then the register arguments of the instruction,
/// then the arguments of its wrapped instructions.
fn get_args(cfg: &MethodCfg, insn: InsnId) -> Vec<(ArgRef, u32)> {
    let mut out = Vec::new();
    add_args(cfg, insn, &mut out);
    out
}

fn add_args(cfg: &MethodCfg, insn: InsnId, out: &mut Vec<(ArgRef, u32)>) {
    let n = cfg.insn(insn);
    if let InsnKind::Ternary { cond } = n.kind {
        out.extend(
            register_args(cfg, cond)
                .into_iter()
                .map(|a| (a, cfg.reg(a).reg_num)),
        );
    }
    if let InsnKind::TernaryCond(cond) = &n.kind {
        out.extend(
            cond.register_args(cfg)
                .into_iter()
                .map(|a| (a, cfg.reg(a).reg_num)),
        );
    }
    for (k, a) in n.args.iter().enumerate() {
        if let InsnArg::Register(r) = a {
            out.push((ArgRef::Arg(insn, k), r.reg_num));
        }
    }
    for a in &n.args {
        if let InsnArg::Wrapped(w) = a {
            add_args(cfg, w.insn, out);
        }
    }
}

/// `ArgsInfo.getArgsSet`: `None` stands for the shared empty set.
fn args_set(infos: &[ArgsInfo], me: usize) -> Option<BitSet> {
    let info = &infos[me];
    if info.args.is_empty() && info.wrapped_insns.as_ref().is_none_or(|w| w.is_empty()) {
        return None;
    }
    let mut set = BitSet::new();
    fill_args_set(infos, me, &mut set);
    Some(set)
}

fn fill_args_set(infos: &[ArgsInfo], me: usize, set: &mut BitSet) {
    for (_, reg) in &infos[me].args {
        set.set(*reg as usize);
    }
    if let Some(wl) = &infos[me].wrapped_insns {
        for &w in wl {
            fill_args_set(infos, w, set);
        }
    }
}

/// `ArgsInfo.checkInline`
fn args_check_inline(
    cfg: &MethodCfg,
    infos: &mut [ArgsInfo],
    me: usize,
    assign_pos: usize,
    arg: ArgRef,
) -> Result<Option<WrapInfo>, String> {
    let border = infos[me].inline_border;
    if assign_pos >= border || !can_move(cfg, infos, me, assign_pos, border)? {
        return Ok(None);
    }
    infos[me].inline_border = assign_pos;
    Ok(Some(args_inline(infos, me, assign_pos, arg)))
}

fn can_move(
    cfg: &MethodCfg,
    infos: &mut [ArgsInfo],
    me: usize,
    from: usize,
    to: usize,
) -> Result<bool, String> {
    let start = from + 1;
    if start == to {
        // previous instruction or on edge of inline border
        return Ok(true);
    }
    if start > to {
        return Err(format!("Invalid inline insn positions: {start} - {to}"));
    }
    let moved_set = args_set(infos, from);
    if moved_set.is_none() && is_const_insn(cfg.insn(infos[from].insn).ty) {
        return Ok(true);
    }
    let reorder = args_can_reorder(cfg, infos, from);
    for i in start..to {
        if get_inlined_insn(infos, i) == Some(me) {
            continue;
        }
        let cur = infos[i].insn;
        if reorder {
            if used_arg_assign(cfg, cur, moved_set.as_ref()) {
                return Ok(false);
            }
        } else if !can_reorder(cfg, cur) || used_arg_assign(cfg, cur, moved_set.as_ref()) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn args_can_reorder(cfg: &MethodCfg, infos: &[ArgsInfo], me: usize) -> bool {
    if !can_reorder(cfg, infos[me].insn) {
        return false;
    }
    if let Some(wl) = &infos[me].wrapped_insns {
        for &w in wl {
            if !args_can_reorder(cfg, infos, w) {
                return false;
            }
        }
    }
    true
}

/// `ArgsInfo.usedArgAssign`
fn used_arg_assign(cfg: &MethodCfg, insn: InsnId, args: Option<&BitSet>) -> bool {
    let Some(args) = args else { return false };
    if args.is_empty() {
        return false;
    }
    match &cfg.insn(insn).result {
        Some(r) => args.get(r.reg_num as usize),
        None => false,
    }
}

fn args_inline(infos: &mut [ArgsInfo], me: usize, assign_pos: usize, arg: ArgRef) -> WrapInfo {
    infos[assign_pos].inlined_insn = Some(me);
    infos[me]
        .wrapped_insns
        .get_or_insert_with(Vec::new)
        .push(assign_pos);
    WrapInfo {
        insn: infos[assign_pos].insn,
        arg,
    }
}

/// `ArgsInfo.getInlinedInsn` (with path compression).
fn get_inlined_insn(infos: &mut [ArgsInfo], me: usize) -> Option<usize> {
    if let Some(inl) = infos[me].inlined_insn {
        if let Some(parent) = get_inlined_insn(infos, inl) {
            infos[me].inlined_insn = Some(parent);
        }
    }
    infos[me].inlined_insn
}

fn check_inline(
    cfg: &mut MethodCfg,
    block: BlockId,
    insn_list: &[InsnId],
    infos: &mut [ArgsInfo],
    wrap_list: &mut Vec<WrapInfo>,
    idx: usize,
    arg: ArgRef,
) -> Result<(), String> {
    let Some(parent) = arg.parent_insn() else {
        return Ok(());
    };
    let Some(reg) = cfg.try_reg(arg) else {
        return Ok(());
    };
    if reg.dont_inline || cfg.insn(parent).contains_flag(iflag::DONT_GENERATE) {
        return Ok(());
    }
    let Some(var) = reg.svar else {
        return Ok(());
    };
    let assign = cfg.svars[var].assign;
    if cfg.try_reg(assign).is_some_and(|r| r.dont_inline) {
        return Ok(());
    }
    let Some(assign_insn) = assign.parent_insn() else {
        return Ok(());
    };
    if cfg
        .insn(assign_insn)
        .contains_flag(iflag::DONT_INLINE | iflag::WRAPPED)
    {
        return Ok(());
    }
    let assign_inline = cfg
        .insn(assign_insn)
        .contains_flag(iflag::FORCE_ASSIGN_INLINE);
    if !assign_inline && cfg.svars[var].is_used_in_phi() {
        return Ok(());
    }
    // allow inline only one use arg
    let mut use_count = 0;
    for use_arg in cfg.svars[var].uses.clone() {
        if let Some(p) = use_arg.parent_insn() {
            if cfg.insn(p).contains_flag(iflag::DONT_GENERATE) {
                continue;
            }
        }
        if !assign_inline && cfg.try_reg(use_arg).is_some_and(|r| r.dont_inline_const) {
            return Ok(());
        }
        use_count += 1;
    }
    if !assign_inline && use_count != 1 {
        return Ok(());
    }
    if !assign_inline {
        if let Some(name) = svar_name(cfg, var) {
            if search_arg_with_name(cfg, assign_insn, &name) {
                // allow inline if name is reused in result
            } else if var_with_same_name_exists(cfg, var) {
                // allow inline if var name is duplicated
            } else {
                // reject inline of named variable
                return Ok(());
            }
        }
    }
    if !check_lambda_inline(cfg, var, assign_insn) {
        return Ok(());
    }

    match insn_list.iter().position(|i| *i == assign_insn) {
        Some(assign_pos) => {
            if let Some(w) = args_check_inline(cfg, infos, idx, assign_pos, arg)? {
                wrap_list.push(w);
            }
        }
        None => {
            // another block
            let assign_block = cfg.block_by_insn(assign_insn);
            if let Some(ab) = assign_block {
                if assign_insn != parent
                    && can_move_between_blocks(cfg, assign_insn, ab, block, infos[idx].insn)?
                {
                    if assign_inline {
                        assign_inline_insn(cfg, arg, assign_insn, ab)?;
                    } else {
                        inline(cfg, arg, assign_insn, ab)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Forbid inline lambda into invoke as an instance arg, i.e. this will not compile:
/// `() -> { ... }.apply();`
fn check_lambda_inline(cfg: &MethodCfg, var: SsaId, assign_insn: InsnId) -> bool {
    let a = cfg.insn(assign_insn);
    let is_custom = a.ty == InsnType::Invoke
        && matches!(
            &a.kind,
            InsnKind::Invoke {
                invoke_type: InvokeType::Custom,
                ..
            }
        );
    if is_custom {
        for use_arg in &cfg.svars[var].uses {
            let Some(p) = use_arg.parent_insn() else {
                continue;
            };
            let pi = cfg.insn(p);
            if pi.ty == InsnType::Invoke
                && crate::method_utils::has_instance_arg(pi)
                && *use_arg == ArgRef::Arg(p, 0)
            {
                return false;
            }
        }
    }
    true
}

fn var_with_same_name_exists(cfg: &MethodCfg, inline_var: SsaId) -> bool {
    let name = svar_name(cfg, inline_var);
    for &sv in &cfg.svar_list {
        if sv == inline_var || cfg.svars[sv].code_var == cfg.svars[inline_var].code_var {
            continue;
        }
        if svar_name(cfg, sv) == name {
            return cfg.svars[sv].use_count() > cfg.svars[inline_var].use_count();
        }
    }
    false
}

fn assign_inline_insn(
    cfg: &mut MethodCfg,
    arg: ArgRef,
    assign_insn: InsnId,
    assign_block: BlockId,
) -> Result<bool, String> {
    let Some(var) = cfg.reg(arg).svar else {
        return Ok(false);
    };
    let use_arg = *cfg.svars[var]
        .uses
        .first()
        .ok_or_else(|| "Index 0 out of bounds for length 0".to_string())?;
    let Some(use_insn) = use_arg.parent_insn() else {
        return Ok(false);
    };
    if cfg.insn(use_insn).contains_flag(iflag::DONT_GENERATE) {
        return Ok(false);
    }
    if !cfg.remove_without_unbind(assign_block, assign_insn) {
        return Ok(false);
    }
    wrap_instruction(cfg, use_arg, assign_insn, true)?;
    Ok(true)
}

fn inline(cfg: &mut MethodCfg, arg: ArgRef, insn: InsnId, block: BlockId) -> Result<bool, String> {
    if cfg.insn(insn).contains_flag(iflag::FORCE_ASSIGN_INLINE) {
        return assign_inline_insn(cfg, arg, insn, block);
    }
    // just move instruction into arg, don't unbind/copy/duplicate
    let replaced = wrap_instruction(cfg, arg, insn, false)?;
    if replaced {
        if let Some(p) = arg.parent_insn() {
            cfg.inherit_metadata(p, insn);
        }
        cfg.unbind_result(insn)?;
        cfg.remove_without_unbind(block, insn);
    }
    Ok(replaced)
}

fn can_move_between_blocks(
    cfg: &MethodCfg,
    assign_insn: InsnId,
    assign_block: BlockId,
    use_block: BlockId,
    use_insn: InsnId,
) -> Result<bool, String> {
    if !block_utils::is_path_exists(cfg, assign_block, use_block) {
        return Ok(false);
    }
    let mut args = BitSet::new();
    for (_, reg) in get_args(cfg, assign_insn) {
        args.set(reg as usize);
    }
    let mut start_check = false;
    for &insn in &cfg.block(assign_block).insns {
        if start_check && (!can_reorder(cfg, insn) || used_arg_assign(cfg, insn, Some(&args))) {
            return Ok(false);
        }
        if insn == assign_insn {
            start_check = true;
        }
    }
    let mut paths = get_all_paths_blocks(cfg, assign_block, use_block);
    paths.remove(&assign_block);
    paths.remove(&use_block);
    for b in paths {
        if cfg.block(b).contains(bflag::DONT_GENERATE) {
            if check_last_insn_type(cfg, b, InsnType::MonitorExit) {
                if crate::passes::regions::utils::is_blocks_in_same_region(
                    cfg,
                    assign_block,
                    use_block,
                ) {
                    // allow move inside same synchronized region
                } else {
                    // don't move from synchronized block
                    return Ok(false);
                }
            }
            // skip checks for not generated blocks
            continue;
        }
        for &insn in &cfg.block(b).insns {
            if !can_reorder(cfg, insn) || used_arg_assign(cfg, insn, Some(&args)) {
                return Ok(false);
            }
        }
    }
    for &insn in &cfg.block(use_block).insns {
        if insn == use_insn {
            return Ok(true);
        }
        if !can_reorder(cfg, insn) || used_arg_assign(cfg, insn, Some(&args)) {
            return Ok(false);
        }
    }
    Err(format!(
        "Can't process instruction move : block {assign_block}"
    ))
}

/// Replace a `move` of a wrapped instruction with the wrapped instruction.
fn simplify_move_insns(cfg: &mut MethodCfg, block: BlockId) -> Result<(), String> {
    let size = cfg.block(block).insns.len();
    for i in 0..size {
        let insn = cfg.block(block).insns[i];
        if cfg.insn(insn).ty != InsnType::Move {
            continue;
        }
        let Some(InsnArg::Wrapped(w)) = cfg.insn(insn).args.first().cloned() else {
            continue;
        };
        let wrap_insn = w.insn;
        cfg.unbind_result(wrap_insn)?;
        let res = cfg.insn(insn).result.clone();
        cfg.set_result(wrap_insn, res);
        cfg.inherit_metadata(wrap_insn, insn);
        cfg.insns[wrap_insn].offset = cfg.insn(insn).offset;
        cfg.insns[wrap_insn].flags &= !iflag::WRAPPED;
        cfg.arena[block].insns[i] = wrap_insn;
    }
    Ok(())
}
