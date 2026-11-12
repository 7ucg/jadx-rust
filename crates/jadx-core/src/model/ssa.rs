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
    /// Register argument that is not (yet) placed in an instruction (`MethodCfg::floating`).
    Floating(usize),
}

impl ArgRef {
    /// `getParentInsn`
    pub fn parent_insn(self) -> Option<InsnId> {
        match self {
            ArgRef::Result(i) | ArgRef::Arg(i, _) => Some(i),
            ArgRef::This | ArgRef::Method(_) | ArgRef::Floating(_) => None,
        }
    }
}

/// Index into `MethodCfg::code_vars`.
pub type CodeVarId = usize;

/// `CodeVar`: a source level variable made of one or more SSA variables.
#[derive(Debug, Clone, Default)]
pub struct CodeVar {
    pub name: Option<String>,
    /// Set before type inference only for immutable types.
    pub ty: Option<ArgType>,
    pub ssa_vars: Vec<SsaId>,
    pub is_final: bool,
    pub is_this: bool,
    pub is_declared: bool,
}

impl CodeVar {
    /// `mergeFlagsFrom`
    pub fn merge_flags_from(&mut self, other: &CodeVar) {
        self.is_declared |= other.is_declared;
        self.is_this |= other.is_this;
        self.is_final |= other.is_final;
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
    /// Set by `InitCodeVariables`.
    pub code_var: Option<CodeVarId>,
    /// Current type (`getTypeInfo().getType()`), unknown until type inference.
    pub ty: ArgType,
    /// `getTypeInfo().getBounds()` (insertion ordered set).
    pub bounds: Vec<TypeBound>,
}

/// `BoundEnum`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundKind {
    Assign,
    Use,
}

/// `ITypeBound`
#[derive(Debug, Clone)]
pub enum TypeBound {
    /// `TypeBoundConst`
    Const {
        kind: BoundKind,
        ty: ArgType,
        arg: Option<ArgRef>,
    },
    /// `TypeBoundCheckCastAssign`
    CheckCastAssign { insn: InsnId },
    /// `TypeBoundFieldGetAssign`
    FieldGetAssign { insn: InsnId, init_type: ArgType },
    /// `TypeBoundInvokeAssign`
    InvokeAssign {
        insn: InsnId,
        generic_return: ArgType,
    },
    /// `TypeBoundInvokeUse`
    InvokeUse {
        insn: InsnId,
        arg: ArgRef,
        generic_arg: ArgType,
    },
}

impl PartialEq for TypeBound {
    fn eq(&self, other: &Self) -> bool {
        use TypeBound::*;
        match (self, other) {
            (
                Const {
                    kind: k1, ty: t1, ..
                },
                Const {
                    kind: k2, ty: t2, ..
                },
            ) => k1 == k2 && t1 == t2,
            (CheckCastAssign { insn: a }, CheckCastAssign { insn: b }) => a == b,
            (FieldGetAssign { insn: a, .. }, FieldGetAssign { insn: b, .. }) => a == b,
            (InvokeAssign { insn: a, .. }, InvokeAssign { insn: b, .. }) => a == b,
            (InvokeUse { insn: a, .. }, InvokeUse { insn: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl TypeBound {
    pub fn kind(&self) -> BoundKind {
        match self {
            TypeBound::Const { kind, .. } => *kind,
            TypeBound::CheckCastAssign { .. }
            | TypeBound::FieldGetAssign { .. }
            | TypeBound::InvokeAssign { .. } => BoundKind::Assign,
            TypeBound::InvokeUse { .. } => BoundKind::Use,
        }
    }

    pub fn is_dynamic(&self) -> bool {
        !matches!(self, TypeBound::Const { .. })
    }
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
            ArgRef::Result(i) => self.insns[i]
                .result
                .as_ref()
                .unwrap_or_else(|| panic!("result register of insn {i} {:?}", self.insns[i].ty)),
            ArgRef::Arg(i, k) => match &self.insns[i].args[k] {
                InsnArg::Register(reg) => reg,
                other => panic!("not a register: {other:?}"),
            },
            ArgRef::This => self.this_arg.as_ref().expect("this register"),
            ArgRef::Method(k) => &self.arg_regs[k],
            ArgRef::Floating(k) => &self.floating[k],
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
            ArgRef::Floating(k) => &mut self.floating[k],
        }
    }

    /// Register at `r` if the location still holds one.
    pub fn try_reg_mut(&mut self, r: ArgRef) -> Option<&mut RegisterArg> {
        match r {
            ArgRef::Result(i) => self.insns[i].result.as_mut(),
            ArgRef::Arg(i, k) => match self.insns[i].args.get_mut(k) {
                Some(InsnArg::Register(reg)) => Some(reg),
                _ => None,
            },
            ArgRef::This => self.this_arg.as_mut(),
            ArgRef::Method(k) => self.arg_regs.get_mut(k),
            ArgRef::Floating(k) => self.floating.get_mut(k),
        }
    }

    /// Park a register argument that is not part of an instruction yet.
    pub fn add_floating(&mut self, reg: RegisterArg) -> ArgRef {
        self.floating.push(reg);
        ArgRef::Floating(self.floating.len() - 1)
    }

    /// `SSAVar.setAssign`: the previous assign argument is detached from the variable.
    pub fn svar_set_assign(&mut self, var: SsaId, assign: ArgRef) {
        let old = self.svars[var].assign;
        if old != assign {
            if let Some(r) = self.try_reg_mut(old) {
                r.svar = None;
            }
            self.svars[var].assign = assign;
        }
    }

    /// `InsnNode.setResult`
    pub fn set_result(&mut self, insn: InsnId, res: Option<RegisterArg>) {
        let var = res.as_ref().and_then(|r| r.svar);
        self.insns[insn].result = res;
        if let Some(v) = var {
            self.svar_set_assign(v, ArgRef::Result(insn));
        }
    }

    /// `InsnNode.rebindArgs`
    pub fn rebind_args(&mut self, insn: InsnId) -> Result<(), String> {
        if let Some(res) = &self.insns[insn].result {
            let Some(v) = res.svar else {
                return Err("No SSA var for result arg".into());
            };
            self.svar_set_assign(v, ArgRef::Result(insn));
        }
        for k in 0..self.insns[insn].args.len() {
            match &self.insns[insn].args[k] {
                InsnArg::Register(r) => {
                    let Some(v) = r.svar else {
                        return Err("No SSA var for argument".into());
                    };
                    self.svar_use(v, ArgRef::Arg(insn, k));
                    self.svar_update_used_in_phi_list(v);
                }
                InsnArg::Wrapped(w) => {
                    let wrapped = w.insn;
                    self.rebind_args(wrapped)?;
                }
                InsnArg::Literal(_) | InsnArg::Named(_) => {}
            }
        }
        Ok(())
    }

    /// `RegisterArg.duplicateWithNewSSAVar`: a new floating register with its own SSA variable
    /// (version = highest live version of the register + 1).
    pub fn duplicate_with_new_svar(&mut self, src: &RegisterArg) -> (ArgRef, SsaId) {
        let mut dup = src.clone();
        dup.svar = None;
        let reg = dup.reg_num;
        let at = self.add_floating(dup);
        let mut v: i64 = -1;
        for id in &self.svar_list {
            let sv = &self.svars[*id];
            if sv.reg_num == reg {
                v = v.max(sv.version as i64);
            }
        }
        let var = self.make_new_svar(reg, (v + 1) as u32, at);
        (at, var)
    }

    /// `MethodNode.makeSyntheticRegArg`: a new register with its own variable and code variable.
    pub fn make_synthetic_reg_arg(&mut self, ty: ArgType) -> Result<RegisterArg, String> {
        let mut reg = RegisterArg::new(0, ty.clone());
        reg.synthetic = true;
        let at = self.add_floating(reg);
        let mut v: i64 = -1;
        for id in &self.svar_list {
            let sv = &self.svars[*id];
            if sv.reg_num == 0 {
                v = v.max(i64::from(sv.version));
            }
        }
        let var = self.make_new_svar(0, (v + 1) as u32, at);
        crate::passes::init_code_vars::init_code_var(self, var)?;
        self.svar_set_type(var, ty)?;
        Ok(self.reg(at).clone())
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
            code_var: None,
            ty: ArgType::unknown_any(),
            bounds: Vec::new(),
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

    /// `SSAVar.getImmutableType`
    pub fn svar_immutable_type(&self, var: SsaId) -> Option<ArgType> {
        let r = self.try_reg(self.svars[var].assign)?;
        r.immutable_type.then(|| r.init_type.clone())
    }

    /// `SSAVar.getPhiList`
    pub fn svar_phi_list(&self, var: SsaId) -> Vec<InsnId> {
        let v = &self.svars[var];
        let mut list = Vec::new();
        if let Some(a) = v.assign.parent_insn() {
            if self.insns[a].ty == InsnType::Phi {
                list.push(a);
            }
        }
        if let Some(u) = &v.used_in_phi {
            list.extend(u.iter().copied());
        }
        list
    }

    /// `SSAVar.setCodeVar`
    pub fn svar_set_code_var(&mut self, var: SsaId, cv: CodeVarId) {
        self.svars[var].code_var = Some(cv);
        if !self.code_vars[cv].ssa_vars.contains(&var) {
            self.code_vars[cv].ssa_vars.push(var);
        }
        if let Some(t) = self.svar_immutable_type(var) {
            self.code_vars[cv].ty = Some(t);
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

    /// `PhiInsn.bindArg(arg, pred)`
    pub fn phi_bind_arg_reg(
        &mut self,
        phi: InsnId,
        arg: RegisterArg,
        pred: BlockId,
    ) -> Result<ArgRef, String> {
        if self.phi_binds(phi).contains(&pred) {
            return Err(format!("Duplicate predecessors in PHI insn: {pred}"));
        }
        let var = arg.svar;
        self.insns[phi].args.push(InsnArg::Register(arg));
        self.phi_binds_mut(phi).push(pred);
        let at = ArgRef::Arg(phi, self.insns[phi].args.len() - 1);
        if let Some(v) = var {
            self.svar_use(v, at);
        }
        Ok(at)
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
        match &self.insns[insn].args[index] {
            InsnArg::Register(r) => {
                if let Some(v) = r.svar {
                    self.svar_remove_use(v, ArgRef::Arg(insn, index));
                }
            }
            InsnArg::Wrapped(w) => {
                let wrapped = w.insn;
                // errors while unbinding a wrapped instruction cannot be reported here
                let _ = self.unbind_insn(wrapped);
            }
            InsnArg::Literal(_) | InsnArg::Named(_) => {}
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
        if self.insns[insn].ty == InsnType::Phi {
            // `PhiInsn.replaceArg`
            let (InsnArg::Register(new), InsnArg::Register(old)) =
                (&to, &self.insns[insn].args[index])
            else {
                return;
            };
            let (new_var, old_var) = (new.svar, old.svar);
            if let Some(v) = new_var {
                self.svar_add_used_in_phi(v, insn);
            }
            self.insns[insn].args[index] = to;
            if let Some(v) = new_var {
                self.svar_use(v, ArgRef::Arg(insn, index));
            }
            if let Some(v) = old_var {
                self.svar_remove_use(v, ArgRef::Arg(insn, index));
                self.svar_update_used_in_phi_list(v);
            }
            return;
        }
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

impl MethodCfg {
    /// `RegisterArg.getType`: the type of the SSA variable (unknown without one).
    pub fn reg_type(&self, at: ArgRef) -> ArgType {
        match self.try_reg(at).and_then(|r| r.svar) {
            Some(v) => self.svars[v].ty.clone(),
            None => ArgType::unknown_any(),
        }
    }

    /// `InsnNode.copyWithoutResult`: copy in the arena; register arguments are attached to their
    /// SSA variables as new uses.
    pub fn copy_without_result(&mut self, id: InsnId) -> InsnId {
        let src = self.insns[id].clone();
        let mut copy = src.clone();
        copy.result = None;
        copy.args = Vec::with_capacity(src.args.len());
        let new_id = self.add_insn(copy);
        for (k, a) in src.args.iter().enumerate() {
            let dup = self.duplicate_arg(a);
            let var = match &dup {
                InsnArg::Register(r) => r.svar,
                _ => None,
            };
            self.insns[new_id].args.push(dup);
            if let Some(v) = var {
                self.svar_use(v, ArgRef::Arg(new_id, k));
            }
        }
        new_id
    }

    /// `InsnArg.duplicate`
    pub fn duplicate_arg(&mut self, arg: &InsnArg) -> InsnArg {
        match arg {
            InsnArg::Register(r) => InsnArg::Register(r.clone()),
            InsnArg::Literal(l) => InsnArg::Literal(l.clone()),
            InsnArg::Named(n) => InsnArg::Named(n.clone()),
            InsnArg::Wrapped(w) => {
                let copy = self.copy_without_result(w.insn);
                InsnArg::Wrapped(super::insn::WrapArg {
                    insn: copy,
                    ty: w.ty.clone(),
                })
            }
        }
    }

    /// `InsnArg.wrapArg`: wrap an instruction (not `CONST`/`MOVE`) into an argument.
    pub fn wrap_arg(&mut self, insn: InsnId) -> Result<InsnArg, String> {
        let ty = self.insns[insn].ty;
        if matches!(ty, InsnType::Const | InsnType::Move) {
            return Err("Don't wrap MOVE or CONST insns".into());
        }
        self.insns[insn].flags |= iflag::WRAPPED;
        let mut arg_ty = ArgType::unknown_any();
        match ty {
            InsnType::ConstStr => {
                arg_ty = ArgType::string();
                self.set_result_type(insn, ArgType::string());
            }
            InsnType::ConstClass => {
                arg_ty = ArgType::class();
                self.set_result_type(insn, ArgType::class());
            }
            _ => {
                if self.insns[insn].result.is_some() {
                    arg_ty = self.reg_type(ArgRef::Result(insn));
                }
            }
        }
        Ok(InsnArg::Wrapped(super::insn::WrapArg { insn, ty: arg_ty }))
    }

    /// `RegisterArg.setType` on the result (updates the SSA variable type).
    fn set_result_type(&mut self, insn: InsnId, ty: ArgType) {
        if let Some(v) = self.insns[insn].result.as_ref().and_then(|r| r.svar) {
            self.svars[v].ty = ty;
        }
    }
}

impl MethodCfg {
    /// `InsnArg.getType` for any argument location.
    pub fn arg_type(&self, at: ArgRef) -> ArgType {
        if let ArgRef::Arg(i, k) = at {
            match &self.insns[i].args[k] {
                InsnArg::Literal(l) => return l.ty.clone(),
                InsnArg::Wrapped(w) => return w.ty.clone(),
                InsnArg::Named(n) => return n.ty.clone(),
                InsnArg::Register(_) => {}
            }
        }
        self.reg_type(at)
    }

    /// `InsnArg.isTypeImmutable`
    pub fn arg_type_immutable(&self, at: ArgRef) -> bool {
        if let ArgRef::Arg(i, k) = at {
            if !matches!(&self.insns[i].args[k], InsnArg::Register(_)) {
                return false;
            }
        }
        let Some(r) = self.try_reg(at) else {
            return false;
        };
        match r.svar {
            Some(v) => self.svar_type_immutable(v),
            None => r.immutable_type,
        }
    }

    /// `SSAVar.isTypeImmutable`
    pub fn svar_type_immutable(&self, var: SsaId) -> bool {
        self.try_reg(self.svars[var].assign)
            .is_some_and(|r| r.immutable_type)
    }

    /// Register at `r` if the location still holds one.
    pub fn try_reg(&self, r: ArgRef) -> Option<&RegisterArg> {
        match r {
            ArgRef::Result(i) => self.insns[i].result.as_ref(),
            ArgRef::Arg(i, k) => match self.insns[i].args.get(k) {
                Some(InsnArg::Register(reg)) => Some(reg),
                _ => None,
            },
            ArgRef::This => self.this_arg.as_ref(),
            ArgRef::Method(k) => self.arg_regs.get(k),
            ArgRef::Floating(k) => self.floating.get(k),
        }
    }

    /// `InsnArg.setType`
    pub fn set_arg_type(&mut self, at: ArgRef, ty: ArgType) -> Result<(), String> {
        if let ArgRef::Arg(i, k) = at {
            match &mut self.insns[i].args[k] {
                InsnArg::Literal(l) => {
                    l.ty = ty;
                    return Ok(());
                }
                InsnArg::Wrapped(w) => {
                    w.ty = ty;
                    return Ok(());
                }
                InsnArg::Named(n) => {
                    n.ty = ty;
                    return Ok(());
                }
                InsnArg::Register(_) => {}
            }
        }
        let Some(r) = self.try_reg(at) else {
            // location of an already removed variable
            return Ok(());
        };
        let Some(var) = r.svar else {
            return Err("Can't change type for register without SSA variable".into());
        };
        self.svar_set_type(var, ty)
    }

    /// `SSAVar.setType`
    pub fn svar_set_type(&mut self, var: SsaId, ty: ArgType) -> Result<(), String> {
        if let Some(im) = self.svar_immutable_type(var) {
            if im != ty {
                return Err(format!(
                    "Can't change immutable type {im} to {ty} for r{}v{}",
                    self.svars[var].reg_num, self.svars[var].version
                ));
            }
        }
        self.svars[var].ty = ty.clone();
        if let Some(cv) = self.svars[var].code_var {
            self.code_vars[cv].ty = Some(ty);
        }
        Ok(())
    }

    /// `SSAVar.markAsImmutable`
    pub fn svar_mark_immutable(&mut self, var: SsaId, ty: ArgType) {
        let at = self.svars[var].assign;
        let r = self.reg_mut(at);
        r.immutable_type = true;
        if r.init_type != ty {
            r.init_type = ty;
        }
    }

    /// Add a bound if not present (`LinkedHashSet.add`).
    pub fn svar_add_bound(&mut self, var: SsaId, bound: TypeBound) {
        if !self.svars[var].bounds.contains(&bound) {
            self.svars[var].bounds.push(bound);
        }
    }
}

impl MethodCfg {
    /// Consistency check of SSA variables, their uses and the registers of the block
    /// instructions (developer aid, see `JADX_RS_VALIDATE`).
    pub fn validate_ssa(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        for v in self.svar_list.clone() {
            let (a, uses) = (self.svars[v].assign, self.svars[v].uses.clone());
            match self.try_reg_mut(a).map(|r| r.svar) {
                Some(s) if s == Some(v) => {}
                other => problems.push(format!("var {v}: assign {a:?} holds {other:?}")),
            }
            for u in uses {
                match self.try_reg_mut(u).map(|r| r.svar) {
                    Some(s) if s == Some(v) => {}
                    other => problems.push(format!("var {v}: use {u:?} holds {other:?}")),
                }
            }
        }
        for b in self.blocks.clone() {
            for i in self.arena[b].insns.clone() {
                for k in 0..self.insns[i].args.len() {
                    let var = match &self.insns[i].args[k] {
                        InsnArg::Register(r) => r.svar,
                        _ => continue,
                    };
                    match var {
                        Some(v) if self.svar_list.contains(&v) => {
                            if !self.svars[v].uses.contains(&ArgRef::Arg(i, k)) {
                                problems.push(format!(
                                    "insn {i} ({:?} @{:x}) arg {k}: not in uses of var {v}",
                                    self.insns[i].ty, self.insns[i].offset
                                ));
                            }
                        }
                        Some(v) => problems.push(format!(
                            "insn {i} ({:?} @{:x}) arg {k}: removed var {v} (assign {:?})",
                            self.insns[i].ty, self.insns[i].offset, self.svars[v].assign
                        )),
                        None => {}
                    }
                }
            }
        }
        problems
    }
}
