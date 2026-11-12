//! Instruction level helpers used by the modification and shrink passes: queries on instructions
//! (`InsnNode.canReorder`, `getRegisterArgs`, ...), argument wrapping (`InsnArg.wrapInstruction`),
//! instruction replacement (`BlockUtils.replaceInsn`) and a block bound `InsnRemover`.

use std::collections::HashSet;

use crate::model::cfg::MethodCfg;
use crate::model::insn::{
    iflag, BlockId, InsnArg, InsnId, InsnKind, InsnNode, InsnType, RegisterArg,
};
use crate::model::ssa::{ArgRef, SsaId};
use crate::model::types::ArgType;

/// `InsnNode.isConstInsn`
pub fn is_const_insn(ty: InsnType) -> bool {
    matches!(
        ty,
        InsnType::Const | InsnType::ConstStr | InsnType::ConstClass
    )
}

/// `InsnNode.canReorder`
pub fn can_reorder(cfg: &MethodCfg, insn: InsnId) -> bool {
    let n = cfg.insn(insn);
    if n.contains_flag(iflag::DONT_GENERATE) {
        return n.ty != InsnType::MonitorExit;
    }
    for a in &n.args {
        if let InsnArg::Wrapped(w) = a {
            if !can_reorder(cfg, w.insn) {
                return false;
            }
        }
    }
    matches!(
        n.ty,
        InsnType::Const
            | InsnType::ConstStr
            | InsnType::ConstClass
            | InsnType::Cast
            | InsnType::Move
            | InsnType::Arith
            | InsnType::Neg
            | InsnType::CmpL
            | InsnType::CmpG
            | InsnType::CheckCast
            | InsnType::InstanceOf
            | InsnType::FillArray
            | InsnType::FilledNewArray
            | InsnType::NewArray
            | InsnType::StrConcat
    )
}

/// `InsnNode.getRegisterArgs`: register arguments including those of wrapped instructions (and
/// of the condition of a ternary).
pub fn register_args(cfg: &MethodCfg, insn: InsnId) -> Vec<ArgRef> {
    let mut out = Vec::new();
    collect_register_args(cfg, insn, &mut out);
    out
}

fn collect_register_args(cfg: &MethodCfg, insn: InsnId, out: &mut Vec<ArgRef>) {
    let n = cfg.insn(insn);
    for (k, a) in n.args.iter().enumerate() {
        match a {
            InsnArg::Register(_) => out.push(ArgRef::Arg(insn, k)),
            InsnArg::Wrapped(w) => collect_register_args(cfg, w.insn, out),
            InsnArg::Literal(_) | InsnArg::Named(_) => {}
        }
    }
    if let InsnKind::Ternary { cond } = n.kind {
        collect_register_args(cfg, cond, out);
    }
    if let InsnKind::TernaryCond(cond) = &n.kind {
        out.extend(cond.register_args(cfg));
    }
}

/// `RegisterArg.getName`
pub fn reg_name(cfg: &MethodCfg, reg: &RegisterArg) -> Option<String> {
    if reg.is_super {
        return Some("super".to_string());
    }
    if reg.is_this {
        return Some("this".to_string());
    }
    svar_name(cfg, reg.svar?)
}

/// `SSAVar.getName`
pub fn svar_name(cfg: &MethodCfg, var: SsaId) -> Option<String> {
    let cv = cfg.svars[var].code_var?;
    cfg.code_vars[cv].name.clone()
}

/// `SSAVar.setName`
pub fn svar_set_name(cfg: &mut MethodCfg, var: SsaId, name: &str) -> Result<(), String> {
    let Some(cv) = cfg.svars[var].code_var else {
        return Err(format!(
            "CodeVar not initialized for name set in SSAVar: r{}v{}",
            cfg.svars[var].reg_num, cfg.svars[var].version
        ));
    };
    cfg.code_vars[cv].name = Some(name.to_string());
    Ok(())
}

/// `RegisterArg.setNameIfUnknown` for the register stored in `reg`.
fn set_name_if_unknown(cfg: &mut MethodCfg, reg: &RegisterArg, name: &str) -> Result<(), String> {
    if reg_name(cfg, reg).is_none() {
        if let Some(v) = reg.svar {
            svar_set_name(cfg, v, name)?;
        }
    }
    Ok(())
}

/// Name of an argument if it is `Named` (`Named.getName`).
pub fn arg_name(cfg: &MethodCfg, arg: &InsnArg) -> Option<String> {
    match arg {
        InsnArg::Register(r) => reg_name(cfg, r),
        InsnArg::Named(n) => Some(n.name.clone()),
        _ => None,
    }
}

/// `InsnNode.visitArgs` looking for an argument named `name` (wrapped instructions are entered).
pub fn search_arg_with_name(cfg: &MethodCfg, insn: InsnId, name: &str) -> bool {
    for a in &cfg.insn(insn).args {
        match a {
            InsnArg::Wrapped(w) => {
                if search_arg_with_name(cfg, w.insn, name) {
                    return true;
                }
            }
            other => {
                if arg_name(cfg, other).as_deref() == Some(name) {
                    return true;
                }
            }
        }
    }
    false
}

/// Type of an argument value that is not (yet) attached to a location.
fn value_type(cfg: &MethodCfg, arg: &InsnArg) -> ArgType {
    match arg {
        InsnArg::Literal(l) => l.ty.clone(),
        InsnArg::Wrapped(w) => w.ty.clone(),
        InsnArg::Named(n) => n.ty.clone(),
        InsnArg::Register(r) => match r.svar {
            Some(v) => cfg.svars[v].ty.clone(),
            None => ArgType::unknown_any(),
        },
    }
}

/// `InsnArg.wrapInsnIntoArg`
pub fn wrap_insn_into_arg(cfg: &mut MethodCfg, insn: InsnId) -> Result<InsnArg, String> {
    let ty = cfg.insn(insn).ty;
    if matches!(ty, InsnType::Const | InsnType::Move) {
        if cfg.insn(insn).contains_flag(iflag::FORCE_ASSIGN_INLINE) {
            cfg.insns[insn].flags |= iflag::WRAPPED;
            let arg_ty = if cfg.insn(insn).result.is_some() {
                cfg.reg_type(ArgRef::Result(insn))
            } else {
                ArgType::unknown_any()
            };
            return Ok(InsnArg::Wrapped(crate::model::insn::WrapArg {
                insn,
                ty: arg_ty,
            }));
        }
        let arg = cfg.insn(insn).args[0].clone();
        // the argument itself moves out of the instruction: its use at the old location is dropped
        if let InsnArg::Register(r) = &arg {
            if let Some(v) = r.svar {
                cfg.svar_remove_use(v, ArgRef::Arg(insn, 0));
            }
        }
        cfg.insns[insn].flags |= iflag::DONT_GENERATE;
        return Ok(arg);
    }
    cfg.wrap_arg(insn)
}

/// `InsnArg.wrapInstruction`: replace the register argument at `at` with `insn`.
/// Returns `false` if the wrap is impossible.
pub fn wrap_instruction(
    cfg: &mut MethodCfg,
    at: ArgRef,
    insn: InsnId,
    unbind: bool,
) -> Result<bool, String> {
    let ArgRef::Arg(parent, index) = at else {
        return Ok(false);
    };
    if parent == insn {
        return Ok(false);
    }
    let old_reg = match cfg.insns[parent].args.get(index) {
        Some(InsnArg::Register(r)) => r.clone(),
        _ => return Ok(false),
    };
    if cfg.insn(insn).ty == InsnType::Move {
        // preserve variable name for move insn (needed in `for-each` loop for iteration variable)
        if let Some(name) = reg_name(cfg, &old_reg) {
            match cfg.insn(insn).args[0].clone() {
                InsnArg::Register(r) => set_name_if_unknown(cfg, &r, &name)?,
                InsnArg::Wrapped(w) => {
                    if let Some(r) = cfg.insn(w.insn).result.clone() {
                        set_name_if_unknown(cfg, &r, &name)?;
                    }
                }
                _ => {}
            }
        }
    }
    let had_result = cfg.insn(insn).result.is_some();
    let mut arg = wrap_insn_into_arg(cfg, insn)?;
    if value_type(cfg, &arg) == ArgType::unknown_any() {
        // restore arg type if wrapped insn missing result
        let old_ty = cfg.arg_type(at);
        match &mut arg {
            InsnArg::Literal(l) => l.ty = old_ty,
            InsnArg::Wrapped(w) => w.ty = old_ty,
            InsnArg::Named(n) => n.ty = old_ty,
            InsnArg::Register(r) => {
                if let Some(v) = r.svar {
                    cfg.svar_set_type(v, old_ty)?;
                }
            }
        }
    }
    cfg.replace_arg(parent, index, arg);
    let _ = unbind; // the old argument is already unbound by `replace_arg`
    if had_result && !cfg.insn(insn).contains_flag(iflag::FORCE_ASSIGN_INLINE) {
        // result not needed in wrapped insn
        cfg.unbind_result(insn)?;
    }
    Ok(true)
}

/// `BlockUtils.replaceInsn(mth, block, i, insn)` for an instruction already added to the arena.
pub fn replace_insn_id(
    cfg: &mut MethodCfg,
    block: BlockId,
    index: usize,
    new_id: InsnId,
) -> Result<(), String> {
    let prev = cfg.arena[block].insns[index];
    let prev_node = cfg.insns[prev].clone();
    cfg.insns[new_id].copy_attributes_from(&prev_node);
    cfg.inherit_metadata(new_id, prev);
    cfg.insns[new_id].offset = prev_node.offset;
    cfg.arena[block].insns[index] = new_id;

    let same = match (&cfg.insns[new_id].result, &prev_node.result) {
        (Some(a), Some(b)) => a.reg_num == b.reg_num && a.svar == b.svar,
        _ => false,
    };
    if same {
        // Don't unbind result for same register.
        cfg.unbind_all_args(prev);
    } else {
        cfg.unbind_insn(prev)?;
    }
    rebind_args(cfg, new_id)
}

/// `BlockUtils.replaceInsn(mth, block, i, insn)`
pub fn replace_insn(
    cfg: &mut MethodCfg,
    block: BlockId,
    index: usize,
    new_insn: InsnNode,
) -> Result<InsnId, String> {
    let id = cfg.add_insn(new_insn);
    replace_insn_id(cfg, block, index, id)?;
    Ok(id)
}

/// `BlockUtils.replaceInsn(mth, block, oldInsn, newInsn)`
pub fn replace_insn_by_ref(
    cfg: &mut MethodCfg,
    block: BlockId,
    old: InsnId,
    new_id: InsnId,
) -> Result<bool, String> {
    match cfg.arena[block].insns.iter().position(|i| *i == old) {
        Some(index) => {
            replace_insn_id(cfg, block, index, new_id)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// `InsnNode.rebindArgs` including the condition of a ternary (`TernaryInsn.rebindArgs`).
pub fn rebind_args(cfg: &mut MethodCfg, insn: InsnId) -> Result<(), String> {
    cfg.rebind_args(insn)?;
    if let InsnKind::Ternary { cond } = cfg.insn(insn).kind {
        for reg in register_args(cfg, cond) {
            if let Some(p) = reg.parent_insn() {
                cfg.rebind_args(p)?;
            }
        }
    }
    if let InsnKind::TernaryCond(cond) = &cfg.insn(insn).kind {
        for reg in cond.register_args(cfg) {
            if let Some(p) = reg.parent_insn() {
                cfg.rebind_args(p)?;
            }
        }
    }
    Ok(())
}

/// `BlockUtils.getAllPathsBlocks`: `start`, `end` and every block on a path between them.
pub fn get_all_paths_blocks(cfg: &MethodCfg, start: BlockId, end: BlockId) -> HashSet<BlockId> {
    let mut set = HashSet::new();
    set.insert(start);
    if start != end {
        let mut stack = vec![end];
        set.insert(end);
        while let Some(b) = stack.pop() {
            for &pred in &cfg.block(b).preds {
                if pred != start && set.insert(pred) {
                    stack.push(pred);
                }
            }
        }
    }
    set
}

/// `BlockUtils.checkLastInsnType`
pub fn check_last_insn_type(cfg: &MethodCfg, block: BlockId, ty: InsnType) -> bool {
    cfg.last_insn(block).is_some_and(|i| i.ty == ty)
}

/// `InsnRemover` bound to a block: removal is collected and performed with [`Remover::perform`].
#[derive(Default)]
pub struct Remover {
    to_remove: Vec<InsnId>,
}

impl Remover {
    pub fn add_and_unbind(&mut self, cfg: &mut MethodCfg, insn: InsnId) -> Result<(), String> {
        self.to_remove.push(insn);
        cfg.unbind_insn(insn)
    }

    pub fn perform(&mut self, cfg: &mut MethodCfg, block: BlockId) -> Result<(), String> {
        if self.to_remove.is_empty() {
            return Ok(());
        }
        for &i in &self.to_remove {
            cfg.unbind_all_args(i);
        }
        for &i in &self.to_remove {
            cfg.unbind_result(i)?;
            cfg.insns[i].flags |= iflag::DONT_GENERATE;
        }
        for &rem in &self.to_remove {
            if let Some(p) = cfg.arena[block].insns.iter().position(|i| *i == rem) {
                cfg.arena[block].insns.remove(p);
            }
        }
        self.to_remove.clear();
        Ok(())
    }
}
