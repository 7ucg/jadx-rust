//! SSA variables and the register-argument bookkeeping around them.
//!
//! Register arguments live inside instructions (`InsnNode::args` / `InsnNode::result`) or are the
//! method's own `this` / argument registers. They are addressed by [`ArgRef`] (their location),
//! which is how an [`SsaVar`] tracks its assignment and its uses.

use super::cfg::MethodCfg;
use super::insn::{iflag, BlockId, InsnArg, InsnId, InsnKind, InsnNode, InsnType, RegisterArg};
use super::types::ArgType;

/// Index into `MethodCfg::svars`.
pub type SsaId = usize;

/// Location of a register argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArgRef {
    /// Result register of an instruction.
    Result(InsnId),
    /// Argument `n` of an instruction.
    Arg(InsnId, usize),
    /// The method's `this` register.
    This,
    /// Method argument register `n` (`getArgRegs`).
    Method(usize),
}

impl ArgRef {
    /// `getParentInsn`
    pub fn parent_insn(self) -> Option<InsnId> {
        match self {
            ArgRef::Result(i) | ArgRef::Arg(i, _) => Some(i),
            ArgRef::This | ArgRef::Method(_) => None,
        }
    }
}

/// `SSAVar`
#[derive(Debug, Clone)]
pub struct SsaVar {
    pub reg_num: u32,
    pub version: u32,
    pub assign: ArgRef,
    pub uses: Vec<ArgRef>,
    /// `None` mirrors the reference's `null` list.
    pub used_in_phi: Option<Vec<InsnId>>,
}

impl SsaVar {
    pub fn use_count(&self) -> usize {
        self.uses.len()
    }
    pub fn is_used_in_phi(&self) -> bool {
        self.used_in_phi.as_ref().is_some_and(|l| !l.is_empty())
    }
}

impl MethodCfg {
    /// Register argument at `r` (panics if the location is empty).
    pub fn reg(&self, r: ArgRef) -> &RegisterArg {
        match r {
            ArgRef::Result(i) => self.insns[i].result.as_ref().expect("result register"),
            ArgRef::Arg(i, k) => match &self.insns[i].args[k] {
                InsnArg::Register(reg) => reg,
                other => panic!("not a register: {other:?}"),
            },
            ArgRef::This => self.this_arg.as_ref().expect("this register"),
            ArgRef::Method(k) => &self.arg_regs[k],
        }
    }

    pub fn reg_mut(&mut self, r: ArgRef) -> &mut RegisterArg {
        match r {
            ArgRef::Result(i) => self.insns[i].result.as_mut().expect("result register"),
            ArgRef::Arg(i, k) => match &mut self.insns[i].args[k] {
                InsnArg::Register(reg) => reg,
                other => panic!("not a register: {other:?}"),
            },
            ArgRef::This => self.this_arg.as_mut().expect("this register"),
            ArgRef::Method(k) => &mut self.arg_regs[k],
        }
    }

    pub fn var(&self, id: SsaId) -> &SsaVar {
        &self.svars[id]
    }

    /// `makeNewSVar(regNum, version, assignArg)`
    pub fn make_new_svar(&mut self, reg_num: u32, version: u32, assign: ArgRef) -> SsaId {
        let id = self.svars.len();
        self.svars.push(SsaVar {
            reg_num,
            version,
            assign,
            uses: Vec::new(),
            used_in_phi: None,
        });
        self.reg_mut(assign).svar = Some(id);
        self.svar_list.push(id);
        id
    }

    /// `removeSVar`
    pub fn remove_svar(&mut self, var: SsaId) {
        if let Some(p) = self.svar_list.iter().position(|v| *v == var) {
            self.svar_list.remove(p);
        }
    }

    /// `SSAVar.use`
    pub fn svar_use(&mut self, var: SsaId, arg: ArgRef) {
        if let Some(old) = self.reg(arg).svar {
            self.svar_remove_use(old, arg);
        }
        self.reg_mut(arg).svar = Some(var);
        self.svars[var].uses.push(arg);
    }

    pub fn svar_remove_use(&mut self, var: SsaId, arg: ArgRef) {
        self.svars[var].uses.retain(|a| *a != arg);
    }

    pub fn svar_add_used_in_phi(&mut self, var: SsaId, phi: InsnId) {
        self.svars[var]
            .used_in_phi
            .get_or_insert_with(|| Vec::with_capacity(1))
            .push(phi);
    }

    pub fn svar_remove_used_in_phi(&mut self, var: SsaId, phi: InsnId) {
        let v = &mut self.svars[var];
        if let Some(list) = &mut v.used_in_phi {
            list.retain(|i| *i != phi);
            if list.is_empty() {
                v.used_in_phi = None;
            }
        }
    }

    pub fn svar_update_used_in_phi_list(&mut self, var: SsaId) {
        self.svars[var].used_in_phi = None;
        let uses = self.svars[var].uses.clone();
        for u in uses {
            if let Some(i) = u.parent_insn() {
                if self.insns[i].ty == InsnType::Phi {
                    self.svar_add_used_in_phi(var, i);
                }
            }
        }
    }

    /// `SSAVar.isAssignInPhi`
    pub fn svar_assign_in_phi(&self, var: SsaId) -> bool {
        self.svars[var]
            .assign
            .parent_insn()
            .is_some_and(|i| self.insns[i].ty == InsnType::Phi)
    }

    /// `getAssignInsn` of the variable.
    pub fn svar_assign_insn(&self, var: SsaId) -> Option<InsnId> {
        self.svars[var].assign.parent_insn()
    }

    // ---- phi instructions -------------------------------------------------------------------

    /// `new PhiInsn(regNum, predecessors)`
    pub fn new_phi(reg_num: u32) -> InsnNode {
        let mut n = InsnNode::new(InsnType::Phi, InsnKind::Phi { binds: Vec::new() });
        n.result = Some(RegisterArg::new(reg_num, ArgType::unknown_any()));
        n.flags |= iflag::DONT_INLINE | iflag::DONT_GENERATE;
        n
    }

    pub fn phi_binds(&self, phi: InsnId) -> &Vec<BlockId> {
        match &self.insns[phi].kind {
            InsnKind::Phi { binds } => binds,
            _ => panic!("not a phi insn"),
        }
    }

    fn phi_binds_mut(&mut self, phi: InsnId) -> &mut Vec<BlockId> {
        match &mut self.insns[phi].kind {
            InsnKind::Phi { binds } => binds,
            _ => panic!("not a phi insn"),
        }
    }

    /// `PhiInsn.bindArg(pred)`: new argument of the phi's register for `pred`.
    pub fn phi_bind_arg(&mut self, phi: InsnId, pred: BlockId) -> Result<ArgRef, String> {
        if self.phi_binds(phi).contains(&pred) {
            return Err(format!("Duplicate predecessors in PHI insn: {pred}"));
        }
        let res = self.insns[phi].result.as_ref().expect("phi result");
        let arg = RegisterArg::new(res.reg_num, res.init_type.clone());
        self.insns[phi].args.push(InsnArg::Register(arg));
        self.phi_binds_mut(phi).push(pred);
        Ok(ArgRef::Arg(phi, self.insns[phi].args.len() - 1))
    }

    /// `PhiInsn.getArgByBlock`
    pub fn phi_arg_by_block(&self, phi: InsnId, block: BlockId) -> Option<usize> {
        self.phi_binds(phi).iter().position(|b| *b == block)
    }

    /// `InsnNode.removeArg(index)`: removes the argument and its use; later arguments shift down.
    pub fn insn_remove_arg(&mut self, insn: InsnId, index: usize) -> InsnArg {
        if let InsnArg::Register(r) = &self.insns[insn].args[index] {
            if let Some(v) = r.svar {
                self.svar_remove_use(v, ArgRef::Arg(insn, index));
            }
        }
        let arg = self.insns[insn].args.remove(index);
        for j in index..self.insns[insn].args.len() {
            if let InsnArg::Register(r) = &self.insns[insn].args[j] {
                if let Some(v) = r.svar {
                    for u in self.svars[v].uses.iter_mut() {
                        if *u == ArgRef::Arg(insn, j + 1) {
                            *u = ArgRef::Arg(insn, j);
                        }
                    }
                }
            }
        }
        arg
    }

    /// `PhiInsn.removeArg(index)`
    pub fn phi_remove_arg(&mut self, phi: InsnId, index: usize) {
        let svar = match &self.insns[phi].args[index] {
            InsnArg::Register(r) => r.svar,
            _ => None,
        };
        self.insn_remove_arg(phi, index);
        self.phi_binds_mut(phi).remove(index);
        if let Some(v) = svar {
            self.svar_update_used_in_phi_list(v);
        }
    }

    // ---- InsnRemover ------------------------------------------------------------------------

    /// `InsnRemover.unbindArgUsage`
    pub fn unbind_arg_usage(&mut self, insn: InsnId, index: usize) {
        if let InsnArg::Register(r) = &self.insns[insn].args[index] {
            if let Some(v) = r.svar {
                self.svar_remove_use(v, ArgRef::Arg(insn, index));
            }
        }
    }

    /// `InsnRemover.unbindAllArgs`
    pub fn unbind_all_args(&mut self, insn: InsnId) {
        let count = self.insns[insn].args.len();
        for i in 0..count {
            self.unbind_arg_usage(insn, i);
        }
        if self.insns[insn].ty == InsnType::Phi {
            for i in 0..count {
                if let InsnArg::Register(r) = &self.insns[insn].args[i] {
                    if let Some(v) = r.svar {
                        self.svar_update_used_in_phi_list(v);
                    }
                }
            }
        }
        self.insns[insn].flags |= iflag::REMOVE | iflag::DONT_GENERATE;
    }

    /// `InsnRemover.unbindResult`
    pub fn unbind_result(&mut self, insn: InsnId) -> Result<(), String> {
        let Some(r) = self.insns[insn].result.as_ref() else {
            return Ok(());
        };
        if let Some(v) = r.svar {
            if self.svars[v].assign == ArgRef::Result(insn) {
                self.remove_ssa_var(v)?;
            }
        }
        self.insns[insn].result = None;
        Ok(())
    }

    fn remove_ssa_var(&mut self, var: SsaId) -> Result<(), String> {
        let uses = self.svars[var].uses.clone();
        if uses.is_empty() {
            self.remove_svar(var);
            return Ok(());
        }
        let is_phi = |cfg: &MethodCfg, a: &ArgRef| {
            a.parent_insn()
                .is_some_and(|i| cfg.insns[i].ty == InsnType::Phi)
        };
        if uses.iter().all(|a| is_phi(self, a)) {
            // removing an argument shifts the later ones, so always take the current first use
            while let Some(ArgRef::Arg(p, k)) = self.svars[var].uses.first().copied() {
                self.phi_remove_arg(p, k);
            }
            self.remove_svar(var);
            return Ok(());
        }
        if uses.iter().all(|a| {
            a.parent_insn()
                .is_some_and(|i| self.insns[i].contains_flag(iflag::DONT_GENERATE))
        }) {
            for a in uses {
                self.reg_mut(a).svar = None;
            }
            self.remove_svar(var);
            return Ok(());
        }
        Err(format!(
            "Can't remove SSA var: r{}v{}, still in use, count: {}",
            self.svars[var].reg_num,
            self.svars[var].version,
            uses.len()
        ))
    }

    /// `InsnRemover.unbindInsn`
    pub fn unbind_insn(&mut self, insn: InsnId) -> Result<(), String> {
        self.unbind_all_args(insn);
        self.unbind_result(insn)?;
        self.insns[insn].flags |= iflag::DONT_GENERATE;
        Ok(())
    }

    /// `InsnRemover.removeWithoutUnbind`
    pub fn remove_without_unbind(&mut self, block: BlockId, insn: InsnId) -> bool {
        if let Some(p) = self.arena[block].insns.iter().position(|i| *i == insn) {
            self.arena[block].insns.remove(p);
            return true;
        }
        if !self.insns[insn].contains_flag(iflag::WRAPPED) {
            self.warnings.push(format!(
                "Failed to remove instruction {insn} from block {block}"
            ));
        }
        false
    }

    /// `InsnRemover.remove(mth, block, insn)`
    pub fn remove_insn_in_block(&mut self, block: BlockId, insn: InsnId) -> Result<(), String> {
        self.unbind_insn(insn)?;
        self.remove_without_unbind(block, insn);
        Ok(())
    }

    /// `BlockUtils.getBlockByInsn`
    pub fn block_by_insn(&self, insn: InsnId) -> Option<BlockId> {
        if self.insns[insn].ty == InsnType::Phi {
            return self.blocks.iter().copied().find(|b| {
                self.arena[*b]
                    .phi_list
                    .as_ref()
                    .is_some_and(|l| l.contains(&insn))
            });
        }
        if self.insns[insn].contains_flag(iflag::WRAPPED) {
            return None;
        }
        self.blocks
            .iter()
            .copied()
            .find(|b| self.arena[*b].insns.contains(&insn))
    }

    /// `InsnRemover.remove(mth, insn)`
    pub fn remove_insn(&mut self, insn: InsnId) -> Result<(), String> {
        if self.insns[insn].contains_flag(iflag::WRAPPED) {
            return self.unbind_insn(insn);
        }
        match self.block_by_insn(insn) {
            Some(b) => self.remove_insn_in_block(b, insn),
            None => {
                self.insns[insn].flags |= iflag::DONT_GENERATE;
                self.warnings
                    .push(format!("Not found block with instruction: {insn}"));
                Ok(())
            }
        }
    }
}

impl MethodCfg {
    /// `InsnNode.replaceArg(from, to)` for a direct argument: unbinds the old register's use and
    /// attaches the new argument to its SSA variable.
    pub fn replace_arg(&mut self, insn: InsnId, index: usize, to: InsnArg) {
        self.unbind_arg_usage(insn, index);
        let var = match &to {
            InsnArg::Register(r) => r.svar,
            _ => None,
        };
        self.insns[insn].args[index] = to;
        if let Some(v) = var {
            self.svar_use(v, ArgRef::Arg(insn, index));
        }
    }

    /// `InsnNode.inheritMetadata` (no source lines or comments are modelled yet).
    pub fn inherit_metadata(&mut self, target: InsnId, source: InsnId) {
        let off = self.insns[source].offset;
        let t = &mut self.insns[target];
        if t.ty == InsnType::Return && t.contains_flag(iflag::SYNTHETIC) {
            t.offset = off;
        }
    }
}
