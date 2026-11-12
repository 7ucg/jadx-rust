//! `ProcessVariables`: decide where the variables are declared (at the first assignment, or at the
//! start of the method) and remove results that are never used.
//!
//! The decision is stored as the `DECLARE_VAR` flag of the assigning instruction and as the list of
//! code variables declared in the method region (`Region::declare_vars`).

use std::collections::{HashMap, HashSet};

use crate::method_utils::{insn_method_details, MethodDetails};
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{iflag, InsnId, InsnType};
use crate::model::region::{Container, LoopType, RegionId, RegionKind};
use crate::model::ssa::{ArgRef, CodeVarId, SsaId};
use crate::model::types::ArgType;
use crate::passes::constructor::MethodEnv;
use crate::passes::driver::MethodState;
use crate::passes::insn_utils::register_args;
use crate::passes::regions::maker::Res;
use crate::passes::regions::traversal::{traverse_method, RegionVisitor};
use crate::passes::regions::utils::{container_insns, is_region_contains_region};
use crate::type_compare::{TypeCompare, TypeCompareEnum};

/// `UsePlace`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UsePlace {
    region: RegionId,
    block: Container,
}

/// `VarUsage`
#[derive(Debug, Default)]
struct VarUsage {
    assigns: Vec<UsePlace>,
    uses: Vec<UsePlace>,
}

pub fn process_variables(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    methods: &HashMap<(usize, usize), MethodState>,
) -> Result<(), String> {
    if cfg.svar_list.is_empty() {
        return Ok(());
    }
    remove_unused_results(cfg, env, methods)?;

    let code_vars = collect_code_vars(cfg)?;
    if code_vars.is_empty() {
        return Ok(());
    }
    check_code_vars(cfg, env, &code_vars);

    // collect all variables usage
    let mut collector = CollectUsage {
        stack: Vec::new(),
        usage: HashMap::new(),
        any_usage: false,
    };
    traverse_method(cfg, &mut collector)?;
    if !collector.any_usage {
        return Ok(());
    }
    let usage_map = collector.usage;

    for cv in &code_vars {
        let mut list: Vec<&VarUsage> = Vec::new();
        let mut vars: Vec<SsaId> = Vec::new();
        for &ssa in &cfg.code_vars[*cv].ssa_vars {
            if let Some(u) = usage_map.get(&ssa) {
                list.push(u);
                vars.push(ssa);
            }
        }
        declare_var(cfg, *cv, &list, &vars)?;
    }
    Ok(())
}

// ---- removeUnusedResults ------------------------------------------------------------------

struct RemoveUnused<'a> {
    env: &'a MethodEnv<'a>,
    methods: &'a HashMap<(usize, usize), MethodState>,
}

impl RegionVisitor for RemoveUnused<'_> {
    fn process_block(&mut self, cfg: &mut MethodCfg, block: Container) -> Res<()> {
        for insn in container_insns(cfg, block) {
            let Some(result) = cfg.insn(insn).result.clone() else {
                continue;
            };
            let svar = result.svar;
            if !self.is_var_unused(cfg, svar) {
                continue;
            }
            let ty = cfg.insn(insn).ty;
            let mut remove = false;
            if matches!(ty, InsnType::Invoke | InsnType::Constructor) {
                // remove unused result
                remove = true;
            } else if can_remove_insn(cfg, insn) {
                // remove whole insn
                cfg.insns[insn].flags |= iflag::REMOVE | iflag::DONT_GENERATE;
                remove = true;
            }
            if remove {
                cfg.insns[insn].result = None;
                if let Some(v) = svar {
                    cfg.remove_svar(v);
                    for u in cfg.svars[v].uses.clone() {
                        if let Some(r) = cfg.try_reg_mut(u) {
                            r.svar = None;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Remove insn if a result is not used
fn can_remove_insn(cfg: &MethodCfg, insn: InsnId) -> bool {
    crate::passes::insn_utils::is_const_insn(cfg.insn(insn).ty)
        || matches!(cfg.insn(insn).ty, InsnType::Cast | InsnType::CheckCast)
}

impl RemoveUnused<'_> {
    fn is_var_unused(&self, cfg: &mut MethodCfg, var: Option<SsaId>) -> bool {
        let Some(var) = var else {
            return true;
        };
        let use_list = cfg.svars[var].uses.clone();
        if use_list.is_empty() {
            return true;
        }
        if cfg.svars[var].is_used_in_phi() {
            return false;
        }
        use_list.iter().all(|a| self.is_arg_unused(cfg, *a))
    }

    fn is_arg_unused(&self, cfg: &mut MethodCfg, arg: ArgRef) -> bool {
        if cfg.try_reg(arg).is_some_and(|r| r.remove) {
            return true;
        }
        // check constructors for removed args
        let Some(parent) = arg.parent_insn() else {
            return false;
        };
        let ArgRef::Arg(_, pos) = arg else {
            return false;
        };
        let p = cfg.insn(parent);
        if p.ty != InsnType::Constructor || p.details.is_none() {
            return false;
        }
        let Some(MethodDetails::App(c, m)) = insn_method_details(self.env.root, p) else {
            return false;
        };
        if self.env.root.classes[c].methods[m].skip_args.is_none() {
            return false;
        }
        // the argument registers of the called method
        let Some(callee) = self.methods.get(&(c, m)).and_then(|s| s.cfg.as_ref()) else {
            return false;
        };
        let Some(mth_arg) = callee.arg_regs.get(pos) else {
            return false;
        };
        let arg_ty = cfg.arg_type(arg);
        let callee_ty = mth_arg
            .svar
            .map(|v| callee.svars[v].ty.clone())
            .unwrap_or_else(ArgType::unknown_any);
        if mth_arg.remove && arg_ty == callee_ty {
            if let Some(r) = cfg.try_reg_mut(arg) {
                r.dont_generate = true;
            }
            return true;
        }
        false
    }
}

fn remove_unused_results(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    methods: &HashMap<(usize, usize), MethodState>,
) -> Result<(), String> {
    traverse_method(cfg, &mut RemoveUnused { env, methods })
}

// ---- code variables ------------------------------------------------------------------------

fn collect_code_vars(cfg: &mut MethodCfg) -> Result<Vec<CodeVarId>, String> {
    let mut order: Vec<CodeVarId> = Vec::new();
    let mut map: HashMap<CodeVarId, Vec<SsaId>> = HashMap::new();
    for &ssa in &cfg.svar_list {
        let cv = cfg.svars[ssa]
            .code_var
            .ok_or_else(|| "no code variable".to_string())?;
        if cfg.code_vars[cv].is_this {
            continue;
        }
        map.entry(cv)
            .or_insert_with(|| {
                order.push(cv);
                Vec::new()
            })
            .push(ssa);
    }
    for cv in &order {
        let list = map[cv].clone();
        if list.len() > 1 {
            // `ssaVar.setCodeVar(codeVar)` also applies the immutable type of an argument
            for &ssa in &list {
                cfg.svar_set_code_var(ssa, *cv);
            }
        }
        cfg.code_vars[*cv].ssa_vars = list;
    }
    Ok(order)
}

fn check_code_vars(cfg: &mut MethodCfg, env: &MethodEnv, code_vars: &[CodeVarId]) {
    let mut unknown_types_count = 0;
    for &cv in code_vars {
        match cfg.code_vars[cv].ty.clone() {
            None => {
                cfg.code_vars[cv].ty = Some(ArgType::unknown_any());
                unknown_types_count += 1;
            }
            Some(code_var_type) => {
                for &ssa in &cfg.code_vars[cv].ssa_vars.clone() {
                    let Some(ssa_type) = cfg.svar_immutable_type(ssa) else {
                        continue;
                    };
                    if !ssa_type.is_type_known() {
                        continue;
                    }
                    let result =
                        TypeCompare::new(&env.root.clsp).compare_types(&ssa_type, &code_var_type);
                    if result == TypeCompareEnum::Conflict || result.is_narrow() {
                        cfg.warnings.push(format!(
                            "Incorrect type for immutable var: ssa={ssa_type}, code={code_var_type}"
                        ));
                    }
                }
            }
        }
    }
    if unknown_types_count != 0 {
        cfg.warnings.push(format!(
            "Unknown variable types count: {unknown_types_count}"
        ));
    }
}

// ---- usage collection ------------------------------------------------------------------------

struct CollectUsage {
    stack: Vec<RegionId>,
    usage: HashMap<SsaId, VarUsage>,
    /// a usage entry was created (also for arguments without a variable)
    any_usage: bool,
}

impl RegionVisitor for CollectUsage {
    fn enter_region(&mut self, _cfg: &mut MethodCfg, region: RegionId) -> Res<bool> {
        self.stack.push(region);
        Ok(true)
    }

    fn leave_region(&mut self, _cfg: &mut MethodCfg, _region: RegionId) -> Res<()> {
        self.stack.pop();
        Ok(())
    }

    fn process_block(&mut self, cfg: &mut MethodCfg, block: Container) -> Res<()> {
        let Some(&region) = self.stack.last() else {
            return Ok(());
        };
        let place = UsePlace { region, block };
        // `regionProcess`
        if let RegionKind::Loop {
            ty: Some(LoopType::For { init, incr }),
            ..
        } = &cfg.regions.arena[region].kind
        {
            let (init, incr) = (*init, *incr);
            self.process_insn(cfg, init, place);
            self.process_insn(cfg, incr, place);
        }
        for insn in container_insns(cfg, block) {
            self.process_insn(cfg, insn, place);
        }
        Ok(())
    }
}

impl CollectUsage {
    fn process_insn(&mut self, cfg: &MethodCfg, insn: InsnId, place: UsePlace) {
        // result
        if let Some(result) = &cfg.insn(insn).result {
            if !result.dont_generate {
                self.any_usage = true;
                if let Some(v) = result.svar {
                    self.usage.entry(v).or_default().assigns.push(place);
                }
            }
        }
        // args
        for a in register_args(cfg, insn) {
            let Some(reg) = cfg.try_reg(a) else {
                continue;
            };
            if !reg.dont_generate {
                self.any_usage = true;
                if let Some(v) = reg.svar {
                    self.usage.entry(v).or_default().uses.push(place);
                }
            }
        }
    }
}

// ---- declaration ---------------------------------------------------------------------------

fn declare_var(
    cfg: &mut MethodCfg,
    cv: CodeVarId,
    usage_list: &[&VarUsage],
    vars: &[SsaId],
) -> Result<(), String> {
    if cfg.code_vars[cv].is_declared {
        return Ok(());
    }
    let mut merged = VarUsage::default();
    for u in usage_list {
        merged.assigns.extend(u.assigns.iter().copied());
        merged.uses.extend(u.uses.iter().copied());
    }
    if merged.assigns.is_empty() && merged.uses.is_empty() {
        return Ok(());
    }
    // check if variable can be declared at one of assigns
    if check_declare_at_assign(cfg, usage_list, vars, &merged) {
        return Ok(());
    }
    // region not found, declare at method start
    if let Some(r) = cfg.regions.method_region {
        declare_var_in_region(cfg, r, cv);
    }
    Ok(())
}

fn check_declare_at_assign(
    cfg: &mut MethodCfg,
    list: &[&VarUsage],
    vars: &[SsaId],
    merged: &VarUsage,
) -> bool {
    if merged.assigns.is_empty() {
        return false;
    }
    for (i, u) in list.iter().enumerate() {
        for assign in &u.assigns {
            if can_declare_at(cfg, merged, assign) {
                return check_declare_at_assign_var(cfg, vars[i]);
            }
        }
    }
    false
}

fn can_declare_at(cfg: &MethodCfg, usage: &VarUsage, place: &UsePlace) -> bool {
    let region = place.region;
    // workaround for declare variables used in several loops
    if matches!(cfg.regions.arena[region].kind, RegionKind::Loop { .. }) {
        for u in &usage.assigns {
            if !is_region_contains_region(cfg, Container::Region(region), u.region) {
                return false;
            }
        }
    }
    // can't declare in else-if chain between 'else' and next 'if'
    if cfg.regions.arena[region].flags & bflag::ELSE_IF_CHAIN != 0 {
        return false;
    }
    is_all_use_after(cfg, place, &usage.assigns) && is_all_use_after(cfg, place, &usage.uses)
}

/// Check if all `use_places` are after `check_place`
fn is_all_use_after(cfg: &MethodCfg, check_place: &UsePlace, use_places: &[UsePlace]) -> bool {
    let region = check_place.region;
    let block = check_place.block;
    let mut to_check: HashSet<UsePlace> = use_places.iter().copied().collect();
    let mut block_found = false;
    for sub in cfg.regions.sub_blocks(region) {
        if !block_found && sub == block {
            block_found = true;
        }
        if block_found {
            to_check.retain(|p| !is_container_contains_use_place(cfg, sub, p));
            if to_check.is_empty() {
                return true;
            }
        }
    }
    false
}

fn is_container_contains_use_place(cfg: &MethodCfg, sub: Container, place: &UsePlace) -> bool {
    if sub == place.block {
        return true;
    }
    if matches!(sub, Container::Region(_)) {
        return is_region_contains_region(cfg, sub, place.region);
    }
    false
}

fn check_declare_at_assign_var(cfg: &mut MethodCfg, var: SsaId) -> bool {
    let assign = cfg.svars[var].assign;
    let Some(parent) = assign.parent_insn() else {
        return false;
    };
    let n = cfg.insn(parent);
    if n.contains_flag(iflag::WRAPPED) || n.ty == InsnType::Phi {
        return false;
    }
    // the assign argument has to be the result of the instruction
    if !matches!(assign, ArgRef::Result(_)) || n.result.is_none() {
        return false;
    }
    cfg.insns[parent].flags |= iflag::DECLARE_VAR;
    if let Some(cv) = cfg.svars[var].code_var {
        cfg.code_vars[cv].is_declared = true;
    }
    true
}

fn declare_var_in_region(cfg: &mut MethodCfg, region: RegionId, cv: CodeVarId) {
    if cfg.code_vars[cv].is_declared {
        cfg.warnings
            .push("Try to declare already declared variable".to_string());
        return;
    }
    cfg.regions.arena[region].declare_vars.push(cv);
    cfg.code_vars[cv].is_declared = true;
}
