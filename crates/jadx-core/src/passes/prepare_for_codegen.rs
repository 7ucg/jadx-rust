//! `PrepareForCodeGen`: last modifications of the instructions before the code generation (removal
//! of instructions that are not generated, short forms of arithmetic, explicit types of constants,
//! constructor call placement). Most of them break the register dependencies, so this pass must
//! run just before the code generation.

use crate::method_utils::has_instance_arg;
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{
    iflag, ArithOp, BlockId, CallType, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType,
};
use crate::model::ssa::ArgRef;
use crate::model::types::ArgType;
use crate::passes::block_utils::follow_empty_path;
use crate::passes::constructor::MethodEnv;
use crate::passes::fix_instructions::visit_insns;
use crate::passes::insn_utils::{
    arg_name, get_all_paths_blocks, reg_name, register_args, same_reg_and_svar,
    wrap_instruction_in_place,
};

pub fn prepare_for_codegen(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    for block in cfg.blocks.clone() {
        if cfg.block(block).contains(bflag::DONT_GENERATE) {
            continue;
        }
        remove_instructions(cfg, block)?;
        check_inline(cfg, block)?;
        remove_parenthesis_block(cfg, block);
        modify_arith(cfg, block)?;
        check_const_usage(cfg, block);
        add_null_casts(cfg, block)?;
    }
    move_constructor_in_constructor(cfg, env)
}

fn remove_instructions(cfg: &mut MethodCfg, block: BlockId) -> Result<(), String> {
    let insns = cfg.block(block).insns.clone();
    let mut keep: Vec<InsnId> = Vec::with_capacity(insns.len());
    for insn in insns {
        let n = cfg.insn(insn);
        let remove = match n.ty {
            InsnType::Nop
            | InsnType::MonitorEnter
            | InsnType::MonitorExit
            | InsnType::MoveException => true,
            InsnType::Constructor => {
                matches!(
                    n.kind,
                    InsnKind::Constructor {
                        call_type: CallType::SelfCall,
                        ..
                    }
                )
            }
            InsnType::Move => {
                // remove redundant moves: unused result and same args names (a = a;)
                let result = n
                    .result
                    .as_ref()
                    .ok_or_else(|| "move without result".to_string())?;
                let var = result
                    .svar
                    .ok_or_else(|| "move result without variable".to_string())?;
                if cfg.svars[var].use_count() == 0 {
                    match (reg_name(cfg, result), n.args.first()) {
                        (Some(name), Some(arg)) => arg_name(cfg, arg).as_deref() == Some(&name),
                        _ => false,
                    }
                } else {
                    false
                }
            }
            _ => false,
        };
        if !remove {
            keep.push(insn);
        }
    }
    cfg.arena[block].insns = keep;
    Ok(())
}

fn check_inline(cfg: &mut MethodCfg, block: BlockId) -> Result<(), String> {
    for i in 0..cfg.block(block).insns.len() {
        let insn = cfg.block(block).insns[i];
        if cfg.insn(insn).ty != InsnType::Move {
            continue;
        }
        // replace 'move' with inner wrapped instruction
        if let Some(InsnArg::Wrapped(w)) = cfg.insn(insn).args.first() {
            let wrap_insn = w.insn;
            let res = cfg.insn(insn).result.clone();
            cfg.set_result(wrap_insn, res);
            let copy = cfg.insn(insn).clone();
            cfg.insns[wrap_insn].copy_attributes_from(&copy);
            cfg.arena[block].insns[i] = wrap_insn;
        }
    }
    Ok(())
}

/// Add explicit type for non int constants
fn check_const_usage(cfg: &mut MethodCfg, block: BlockId) {
    for block_insn in cfg.block(block).insns.clone() {
        let mut all = Vec::new();
        visit_insns(cfg, block_insn, &mut all);
        for insn in all {
            if forbid_explicit_type(cfg.insn(insn).ty) {
                continue;
            }
            for a in cfg.insns[insn].args.iter_mut() {
                if let InsnArg::Literal(l) = a {
                    if l.ty != ArgType::int() {
                        l.explicit_primitive = true;
                    }
                }
            }
        }
    }
}

fn forbid_explicit_type(ty: InsnType) -> bool {
    matches!(
        ty,
        InsnType::Const
            | InsnType::Cast
            | InsnType::If
            | InsnType::FilledNewArray
            | InsnType::Aput
            | InsnType::Arith
    )
}

fn remove_parenthesis_block(cfg: &mut MethodCfg, block: BlockId) {
    for insn in cfg.block(block).insns.clone() {
        remove_parenthesis(cfg, insn);
    }
}

/// Remove parenthesis for wrapped insn in arith '+' or '-' (`(a + b) + c` => `a + b + c`)
fn remove_parenthesis(cfg: &mut MethodCfg, insn: InsnId) {
    if cfg.insn(insn).ty == InsnType::Arith {
        let InsnKind::Arith(op) = cfg.insn(insn).kind else {
            return;
        };
        if matches!(op, ArithOp::Add | ArithOp::Mul | ArithOp::And | ArithOp::Or) {
            for i in 0..2 {
                let Some(InsnArg::Wrapped(w)) = cfg.insn(insn).args.get(i) else {
                    continue;
                };
                let wrap_insn = w.insn;
                if cfg.insn(wrap_insn).ty == InsnType::Arith
                    && matches!(cfg.insn(wrap_insn).kind, InsnKind::Arith(o) if o == op)
                {
                    cfg.insns[wrap_insn].flags |= iflag::DONT_WRAP;
                }
                remove_parenthesis(cfg, wrap_insn);
            }
        }
    } else {
        if cfg.insn(insn).ty == InsnType::Ternary {
            if let InsnKind::TernaryCond(cond) = &mut cfg.insns[insn].kind {
                let mode = cond.mode;
                for c in cond.args.iter_mut() {
                    if c.mode == mode {
                        c.dont_wrap = true;
                    }
                }
            }
        }
        let wrapped: Vec<InsnId> = cfg
            .insn(insn)
            .args
            .iter()
            .filter_map(|a| match a {
                InsnArg::Wrapped(w) => Some(w.insn),
                _ => None,
            })
            .collect();
        for w in wrapped {
            remove_parenthesis(cfg, w);
        }
    }
}

/// Replace arithmetic operation with short form (`a = a + 2` => `a += 2`)
fn modify_arith(cfg: &mut MethodCfg, block: BlockId) -> Result<(), String> {
    for insn in cfg.block(block).insns.clone() {
        let n = cfg.insn(insn);
        if n.ty != InsnType::Arith
            || n.contains_flag(iflag::ARITH_ONEARG)
            || n.contains_flag(iflag::DECLARE_VAR)
        {
            continue;
        }
        let res = n
            .result
            .as_ref()
            .ok_or_else(|| "arith without result".to_string())?;
        let replace = match n.args.first() {
            Some(InsnArg::Register(reg)) => {
                same_reg_and_svar(res, reg) || same_code_var(cfg, res.svar, reg.svar)
            }
            _ => false,
        };
        if replace {
            cfg.insns[insn].result = None;
            cfg.insns[insn].flags |= iflag::ARITH_ONEARG;
        }
    }
    Ok(())
}

/// `RegisterArg.sameCodeVar`
fn same_code_var(
    cfg: &MethodCfg,
    a: Option<crate::model::ssa::SsaId>,
    b: Option<crate::model::ssa::SsaId>,
) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => cfg.svars[a].code_var == cfg.svars[b].code_var,
        _ => false,
    }
}

fn add_null_casts(cfg: &mut MethodCfg, block: BlockId) -> Result<(), String> {
    for insn in cfg.block(block).insns.clone() {
        match cfg.insn(insn).ty {
            InsnType::Invoke => {
                if has_instance_arg(cfg.insn(insn)) {
                    verify_null_cast(cfg, insn, 0)?;
                }
            }
            InsnType::ArrayLength => verify_null_cast(cfg, insn, 0)?,
            _ => {}
        }
    }
    Ok(())
}

fn verify_null_cast(cfg: &mut MethodCfg, insn: InsnId, index: usize) -> Result<(), String> {
    let Some(arg) = cfg.insn(insn).args.get(index) else {
        return Ok(());
    };
    if !is_zero_const(cfg, arg) {
        return Ok(());
    }
    let cast_type = cfg.arg_type(ArgRef::Arg(insn, index));
    let mut cast = InsnNode::new(
        InsnType::Cast,
        InsnKind::Index(IndexValue::Type(cast_type.clone())),
    );
    cast.args.push(InsnArg::lit(0, cast_type));
    let id = cfg.add_insn(cast);
    wrap_instruction_in_place(cfg, insn, index, id)
}

/// `InsnArg.isZeroConst`
fn is_zero_const(cfg: &MethodCfg, arg: &InsnArg) -> bool {
    match arg {
        InsnArg::Literal(l) => l.literal == 0,
        InsnArg::Wrapped(w) => {
            let n = cfg.insn(w.insn);
            n.ty == InsnType::Const
                && matches!(n.args.first(), Some(InsnArg::Literal(l)) if l.literal == 0)
        }
        _ => false,
    }
}

/// Check that 'super' or 'this' call in constructor is a first instruction.
/// Otherwise, move to the top and add a warning.
fn move_constructor_in_constructor(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    if !env.root.classes[env.cls].methods[env.mth].is_constructor() {
        return Ok(());
    }
    let Some(ctr) = search_constructor_call(cfg) else {
        return Ok(());
    };
    if cfg.insn(ctr).contains_flag(iflag::DONT_GENERATE) {
        return Ok(());
    }
    let first_insn = is_first_insn(cfg, ctr)?;
    let Some(method_region) = cfg.regions.method_region else {
        return Ok(());
    };
    let has_declare_vars = !cfg.regions.arena[method_region].declare_vars.is_empty();
    if first_insn && !has_declare_vars {
        // move not needed
        return Ok(());
    }
    let call_type = match &cfg.insn(ctr).kind {
        InsnKind::Constructor { call_type, .. } => call_type.name().to_lowercase(),
        _ => String::new(),
    };
    let Some(block) = cfg.block_by_insn(ctr) else {
        cfg.warnings
            .push(format!("Failed to move {call_type} instruction to top"));
        return Ok(());
    };
    if !first_insn {
        let mut reg_args: Vec<ArgRef> = register_args(cfg, ctr);
        reg_args.retain(|a| {
            let Some(r) = cfg.try_reg(*a) else {
                return true;
            };
            let is_this = cfg
                .this_arg
                .as_ref()
                .is_some_and(|t| same_reg_and_svar(t, r));
            let is_arg = cfg.arg_regs.iter().any(|m| same_reg_and_svar(m, r));
            !(is_this || is_arg)
        });
        if !reg_args.is_empty() {
            cfg.warnings
                .push("Illegal instructions before constructor call".to_string());
            return Ok(());
        }
        cfg.warnings.push(format!(
            "'{call_type}' call moved to the top of the method (can break code semantics)"
        ));
    }
    // move confirmed
    cfg.arena[block].insns.retain(|i| *i != ctr);
    let container = cfg.regions.new_insn_container(vec![ctr]);
    cfg.regions
        .plain_blocks_mut(method_region)
        .insert(0, container);
    Ok(())
}

fn search_constructor_call(cfg: &MethodCfg) -> Option<InsnId> {
    for &block in &cfg.blocks {
        for &insn in &cfg.block(block).insns {
            let n = cfg.insn(insn);
            if n.ty == InsnType::Constructor {
                if let InsnKind::Constructor { call_type, .. } = &n.kind {
                    if matches!(call_type, CallType::Super | CallType::This) {
                        return Some(insn);
                    }
                }
                return None;
            }
        }
    }
    None
}

/// `BlockUtils.isFirstInsn`
fn is_first_insn(cfg: &MethodCfg, insn: InsnId) -> Result<bool, String> {
    let start_block = follow_empty_path(cfg, cfg.enter, false, false);
    if let Some(first) = cfg.block(start_block).insns.first() {
        return Ok(*first == insn);
    }
    // handle branching with empty blocks
    let block = cfg
        .block_by_insn(insn)
        .ok_or_else(|| "Insn not found in method".to_string())?;
    if cfg.block(block).insns.first() != Some(&insn) {
        return Ok(false);
    }
    for path_block in get_all_paths_blocks(cfg, cfg.enter, block) {
        if !cfg.block(path_block).insns.is_empty() && path_block != block {
            return Ok(false);
        }
    }
    Ok(true)
}
