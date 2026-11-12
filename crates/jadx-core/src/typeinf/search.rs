//! `TypeSearch`: slow multi-variable type search (with `TypeSearchState` and `TypeSearchVarInfo`).
//!
//! Used only if the fast type propagation failed for some variables:
//! - find all possible candidate types within the bounds of every variable
//! - build the dynamic constraint list of every variable
//! - run the search by checking all candidates

use std::collections::HashMap;

use super::clsp_order::ordered_clsp;
use super::update::{TypeUpdate, UpdateResult};
use crate::model::bitset::java_sort;
use crate::model::cfg::MethodCfg;
use crate::model::insn::{InsnArg, InsnId, InsnType};
use crate::model::ssa::{ArgRef, BoundKind, SsaId, TypeBound};
use crate::model::types::ArgType;

const VARS_PROCESS_LIMIT: usize = 5_000;
const CANDIDATES_COUNT_LIMIT: usize = 10;
const SEARCH_ITERATION_LIMIT: usize = 1_000_000;

/// `ITypeConstraint` implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstraintKind {
    Move,
    Phi,
}

/// `AbstractTypeConstraint` with its checks.
#[derive(Debug, Clone)]
struct Constraint {
    kind: ConstraintKind,
    insn: InsnId,
    related_vars: Vec<SsaId>,
}

/// `TypeSearchVarInfo`
#[derive(Debug)]
struct VarInfo {
    var: SsaId,
    type_resolved: bool,
    current_type: ArgType,
    candidate_types: Vec<ArgType>,
    current_index: i64,
    constraints: Vec<Constraint>,
}

impl VarInfo {
    fn new(var: SsaId) -> Self {
        VarInfo {
            var,
            type_resolved: false,
            current_type: ArgType::unknown_any(),
            candidate_types: Vec::new(),
            current_index: -1,
            constraints: Vec::new(),
        }
    }

    fn mark_resolved(&mut self, ty: ArgType) {
        self.current_type = ty;
        self.type_resolved = true;
        self.candidate_types = Vec::new();
    }

    fn reset(&mut self) {
        if self.type_resolved {
            return;
        }
        self.current_index = 0;
        self.current_type = self.candidate_types[0].clone();
    }

    /// Switch `current_type` to the next candidate; returns `true` if this is the first candidate.
    fn next_type(&mut self) -> bool {
        if self.type_resolved {
            return false;
        }
        let len = self.candidate_types.len() as i64;
        self.current_index = (self.current_index + 1) % len;
        self.current_type = self.candidate_types[self.current_index as usize].clone();
        self.current_index == 0
    }
}

/// `TypeSearchState`: variables in method order.
struct State {
    infos: Vec<VarInfo>,
    index: HashMap<SsaId, usize>,
}

impl State {
    fn new(cfg: &MethodCfg) -> Self {
        let mut infos = Vec::with_capacity(cfg.svar_list.len());
        let mut index = HashMap::with_capacity(cfg.svar_list.len());
        for v in &cfg.svar_list {
            index.insert(*v, infos.len());
            infos.push(VarInfo::new(*v));
        }
        State { infos, index }
    }

    fn var_index(&self, var: SsaId) -> Result<usize, String> {
        self.index
            .get(&var)
            .copied()
            .ok_or_else(|| format!("TypeSearchVarInfo not found in map for var: {var}"))
    }

    /// `getArgType`: the candidate type of a register, the type of any other argument.
    fn arg_type(&self, cfg: &MethodCfg, at: ArgRef) -> Result<ArgType, String> {
        let is_register = match at {
            ArgRef::Arg(i, k) => matches!(cfg.insn(i).args[k], InsnArg::Register(_)),
            _ => true,
        };
        if is_register {
            let Some(var) = cfg.reg(at).svar else {
                return Err("TypeSearchVarInfo not found in map for var: null".into());
            };
            return Ok(self.infos[self.var_index(var)?].current_type.clone());
        }
        Ok(cfg.arg_type(at))
    }

    fn unresolved(&self) -> Vec<usize> {
        (0..self.infos.len())
            .filter(|i| !self.infos[*i].type_resolved)
            .collect()
    }

    fn resolved(&self) -> Vec<usize> {
        (0..self.infos.len())
            .filter(|i| self.infos[*i].type_resolved)
            .collect()
    }
}

/// `TypeSearch`
pub struct TypeSearch<'a, 'b> {
    cfg: &'b mut MethodCfg,
    upd: &'b TypeUpdate<'a>,
    state: State,
}

impl<'a, 'b> TypeSearch<'a, 'b> {
    pub fn new(cfg: &'b mut MethodCfg, upd: &'b TypeUpdate<'a>) -> Self {
        let state = State::new(cfg);
        TypeSearch { cfg, upd, state }
    }

    pub fn run(&mut self) -> Result<bool, String> {
        let vars = self.cfg.svar_list.clone();
        if vars.len() > VARS_PROCESS_LIMIT {
            self.cfg.warnings.push(format!(
                "Multi-variable search skipped. Vars limit reached: {} (expected less than {})",
                vars.len(),
                VARS_PROCESS_LIMIT
            ));
            return Ok(false);
        }
        for v in &vars {
            self.fill_type_candidates(*v)?;
        }
        for v in &vars {
            self.collect_constraints(*v)?;
        }

        // quick search for variables without dependencies
        for i in self.state.unresolved() {
            self.resolve_independent_variables(i)?;
        }

        let unresolved = self.state.unresolved();
        let search_success = if unresolved.is_empty() {
            true
        } else {
            self.search(&unresolved)? && self.full_check(&unresolved)?
        };
        if search_success {
            return self.apply_resolved_vars();
        }
        Ok(false)
    }

    fn apply_resolved_vars(&mut self) -> Result<bool, String> {
        let mut updated: Vec<usize> = Vec::new();
        for i in self.state.resolved() {
            let var = self.state.infos[i].var;
            let resolved_type = self.state.infos[i].current_type.clone();
            if !resolved_type.is_type_known() {
                // ignore unknown variables
                continue;
            }
            if resolved_type == self.cfg.svars[var].ty {
                // type already set
                continue;
            }
            self.cfg.svar_set_type(var, resolved_type)?;
            updated.push(i);
        }
        let mut apply_success = true;
        for i in updated {
            let var = self.state.infos[i].var;
            let ty = self.state.infos[i].current_type.clone();
            if self.upd.apply_with_wider_ign_same(self.cfg, var, &ty)? == UpdateResult::Reject {
                // the reference adds a debug comment here
                apply_success = false;
            }
        }
        Ok(apply_success)
    }

    fn search(&mut self, vars: &[usize]) -> Result<bool, String> {
        let len = vars.len();
        // prepare vars
        for &i in vars {
            self.state.infos[i].reset();
        }
        // check all types combinations
        let mut n = 0usize;
        let i = 0usize;
        while !self.full_check(vars)? {
            let first = vars[i];
            if self.state.infos[first].next_type() {
                let mut k = i + 1;
                if k >= len {
                    return Ok(false);
                }
                let mut next = vars[k];
                loop {
                    if self.state.infos[next].next_type() {
                        k += 1;
                        if k >= len {
                            return Ok(false);
                        }
                        next = vars[k];
                    } else {
                        break;
                    }
                }
            }
            n += 1;
            if n > SEARCH_ITERATION_LIMIT {
                return Ok(false);
            }
        }
        // mark all vars as resolved
        for &i in vars {
            self.state.infos[i].type_resolved = true;
        }
        Ok(true)
    }

    fn resolve_independent_variables(&mut self, idx: usize) -> Result<bool, String> {
        let mut all_related_resolved = true;
        for c in &self.state.infos[idx].constraints {
            for v in &c.related_vars {
                if !self.state.infos[self.state.var_index(*v)?].type_resolved {
                    all_related_resolved = false;
                }
            }
        }
        if !all_related_resolved {
            return Ok(false);
        }
        // variable is independent, run single search
        self.state.infos[idx].reset();
        loop {
            if self.single_check(idx)? {
                self.state.infos[idx].type_resolved = true;
                return Ok(true);
            }
            if self.state.infos[idx].next_type() {
                break;
            }
        }
        Ok(false)
    }

    fn full_check(&self, vars: &[usize]) -> Result<bool, String> {
        for &v in vars {
            if !self.single_check(v)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn single_check(&self, idx: usize) -> Result<bool, String> {
        let info = &self.state.infos[idx];
        if info.type_resolved {
            return Ok(true);
        }
        for c in &info.constraints {
            if !self.check_constraint(c)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn check_constraint(&self, c: &Constraint) -> Result<bool, String> {
        let res_type = self.state.arg_type(self.cfg, ArgRef::Result(c.insn))?;
        match c.kind {
            ConstraintKind::Move => {
                let arg_type = self.state.arg_type(self.cfg, ArgRef::Arg(c.insn, 0))?;
                let res = self.upd.compare.compare_types(&res_type, &arg_type);
                Ok(res.is_equal() || res.is_wider())
            }
            ConstraintKind::Phi => {
                for k in 0..self.cfg.insn(c.insn).args.len() {
                    let arg_type = self.state.arg_type(self.cfg, ArgRef::Arg(c.insn, k))?;
                    if arg_type != res_type {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }

    fn fill_type_candidates(&mut self, var: SsaId) -> Result<(), String> {
        let idx = self.state.var_index(var)?;
        if let Some(im) = self.cfg.svar_immutable_type(var) {
            self.state.infos[idx].mark_resolved(im);
            return Ok(());
        }
        let current_type = self.cfg.svars[var].ty.clone();
        if current_type.is_type_known() {
            self.state.infos[idx].mark_resolved(current_type);
            return Ok(());
        }

        let bounds: Vec<TypeBound> = self.cfg.svars[var].bounds.clone();
        let mut assigns: Vec<ArgType> = Vec::new();
        let mut uses: Vec<ArgType> = Vec::new();
        for b in &bounds {
            let Some(t) = self.upd.bound_type(self.cfg, None, b) else {
                return Err("bound without type".into());
            };
            let set = if b.kind() == BoundKind::Assign {
                &mut assigns
            } else {
                &mut uses
            };
            if !set.contains(&t) {
                set.push(t);
            }
        }

        let mut candidates: Vec<ArgType> = Vec::new();
        self.add_candidate_types(&bounds, &mut candidates, &assigns);
        self.add_candidate_types(&bounds, &mut candidates, &uses);

        for assign_type in &assigns {
            let wider = self.get_wider_types(assign_type);
            self.add_candidate_types(&bounds, &mut candidates, &wider);
        }
        for use_type in &uses {
            let narrow = self.get_narrow_types(use_type);
            self.add_candidate_types(&bounds, &mut candidates, &narrow);
        }

        self.add_usage_type_candidates(var, &bounds, &mut candidates);

        let info = &mut self.state.infos[idx];
        match candidates.len() {
            0 => {
                info.type_resolved = true;
                info.current_type = ArgType::unknown_any();
                info.candidate_types = Vec::new();
            }
            1 => {
                info.type_resolved = true;
                info.current_type = candidates[0].clone();
                info.candidate_types = Vec::new();
            }
            _ => {
                info.type_resolved = false;
                info.current_type = ArgType::unknown_any();
                let compare = &self.upd.compare;
                // `getReversedComparator`
                java_sort(&mut candidates, |a, b| compare.comparator(b, a));
                info.candidate_types = candidates;
            }
        }
        Ok(())
    }

    fn add_usage_type_candidates(
        &self,
        var: SsaId,
        bounds: &[TypeBound],
        candidates: &mut Vec<ArgType>,
    ) {
        for use_arg in &self.cfg.svars[var].uses {
            let Some(parent) = use_arg.parent_insn() else {
                continue;
            };
            if self.cfg.insn(parent).ty == InsnType::Aput {
                let aput_type = self.cfg.arg_type(ArgRef::Arg(parent, 2));
                if aput_type.is_type_known() {
                    self.add_candidate_type(bounds, candidates, ArgType::array(aput_type));
                }
            }
        }
    }

    fn add_candidate_types(
        &self,
        bounds: &[TypeBound],
        collected: &mut Vec<ArgType>,
        candidate_types: &[ArgType],
    ) {
        for t in candidate_types {
            if self.add_candidate_type(bounds, collected, t.clone()) {
                return;
            }
        }
    }

    fn add_candidate_type(
        &self,
        bounds: &[TypeBound],
        collected: &mut Vec<ArgType>,
        candidate: ArgType,
    ) -> bool {
        if candidate.is_type_known() && self.upd.in_bounds(self.cfg, None, bounds, &candidate) {
            if !collected.contains(&candidate) {
                collected.push(candidate);
            }
            if collected.len() > CANDIDATES_COUNT_LIMIT {
                return true;
            }
        }
        false
    }

    fn get_wider_types(&self, ty: &ArgType) -> Vec<ArgType> {
        if ty.is_type_known() {
            if ty.is_object() {
                if let Some(name) = ty.object_name() {
                    return ordered_clsp(&self.upd.root.clsp)
                        .super_types(&name)
                        .iter()
                        .map(|a| ArgType::object(a))
                        .collect();
                }
            }
        } else {
            return expand_unknown_type(ty);
        }
        Vec::new()
    }

    fn get_narrow_types(&self, ty: &ArgType) -> Vec<ArgType> {
        if ty.is_type_known() {
            if ty.is_object() {
                if *ty == ArgType::object_type() {
                    // a lot of objects to return
                    return vec![ArgType::object_type()];
                }
                if let Some(name) = ty.object_name() {
                    return ordered_clsp(&self.upd.root.clsp)
                        .implementations(&name)
                        .iter()
                        .map(|a| ArgType::object(a))
                        .collect();
                }
            }
        } else {
            return expand_unknown_type(ty);
        }
        Vec::new()
    }

    fn collect_constraints(&mut self, var: SsaId) -> Result<(), String> {
        let idx = self.state.var_index(var)?;
        if self.state.infos[idx].type_resolved {
            self.state.infos[idx].constraints = Vec::new();
            return Ok(());
        }
        let mut constraints = Vec::new();
        let assign = self.cfg.svars[var].assign;
        if let Some(c) = self.make_constraint(assign)? {
            constraints.push(c);
        }
        for at in self.cfg.svars[var].uses.clone() {
            if let Some(c) = self.make_constraint(at)? {
                constraints.push(c);
            }
        }
        self.state.infos[idx].constraints = constraints;
        Ok(())
    }

    fn make_constraint(&self, arg: ArgRef) -> Result<Option<Constraint>, String> {
        let Some(insn) = arg.parent_insn() else {
            return Ok(None);
        };
        if self.cfg.arg_type_immutable(arg) {
            return Ok(None);
        }
        match self.cfg.insn(insn).ty {
            InsnType::Move => {
                if !matches!(self.cfg.insn(insn).args.first(), Some(InsnArg::Register(_))) {
                    return Ok(None);
                }
                Ok(Some(Constraint {
                    kind: ConstraintKind::Move,
                    insn,
                    related_vars: self.collect_related_vars(insn, arg)?,
                }))
            }
            InsnType::Phi => Ok(Some(Constraint {
                kind: ConstraintKind::Phi,
                insn,
                related_vars: self.collect_related_vars(insn, arg)?,
            })),
            _ => Ok(None),
        }
    }

    /// `AbstractTypeConstraint.collectRelatedVars`
    fn collect_related_vars(&self, insn: InsnId, arg: ArgRef) -> Result<Vec<SsaId>, String> {
        let node = self.cfg.insn(insn);
        let mut list = Vec::with_capacity(node.args.len());
        let reg_var = |a: &InsnArg| match a {
            InsnArg::Register(r) => r.svar,
            _ => None,
        };
        if arg == ArgRef::Result(insn) {
            for a in &node.args {
                if matches!(a, InsnArg::Register(_)) {
                    list.push(reg_var(a).ok_or("register without SSA variable")?);
                }
            }
        } else {
            let res = node.result.as_ref().and_then(|r| r.svar);
            list.push(res.ok_or("instruction result without SSA variable")?);
            for (k, a) in node.args.iter().enumerate() {
                if ArgRef::Arg(insn, k) != arg && matches!(a, InsnArg::Register(_)) {
                    list.push(reg_var(a).ok_or("register without SSA variable")?);
                }
            }
        }
        Ok(list)
    }
}

/// `expandUnknownType`
fn expand_unknown_type(ty: &ArgType) -> Vec<ArgType> {
    ty.possible_types()
        .into_iter()
        .map(ArgType::convert_from_primitive)
        .collect()
}
