//! Variable declarations and names for the code generator.
//!
//! The declaration of a variable is decided by the variable processing passes (instruction flag
//! `DECLARE_VAR` and the declaration lists of regions). When none of these marks exists on a
//! method, the variables are declared at their first assignment in region order, so that the
//! generated code stays readable.

use super::var_names::apply_variable_names;
use crate::model::cfg::MethodCfg;
use crate::model::insn::{iflag, InsnId};
use crate::model::node::Root;
use crate::model::region::{Container, LoopType, RegionKind};

/// `DebugInfoApplyVisitor.processMethodParametersAttribute`: names and `final` flags of the
/// method arguments.
pub fn apply_method_parameters(cfg: &mut MethodCfg, root: &Root, cls: usize, mth: usize) {
    let m = &root.classes[cls].methods[mth];
    let params = &m.attrs.method_parameters;
    if params.is_empty() || params.len() != m.info.arg_types.len() {
        return;
    }
    for (i, reg) in cfg.arg_regs.clone().iter().enumerate() {
        let (Some(name), acc) = &params[i] else {
            continue;
        };
        if !crate::name_mapper::is_valid_and_printable(name) {
            continue;
        }
        if let Some(cv) = reg.svar.and_then(|v| cfg.svars[v].code_var) {
            cfg.code_vars[cv].name = Some(name.clone());
            if acc & crate::access::ACC_FINAL != 0 {
                cfg.code_vars[cv].is_final = true;
            }
        }
    }
}

/// Declare variables (if no pass did it) and guess variable names.
pub fn prepare_variables(cfg: &mut MethodCfg, root: &Root) {
    let Some(region) = cfg.regions.method_region else {
        return;
    };
    let declared_by_passes = !cfg.regions.declare_vars.is_empty()
        || cfg.blocks.iter().any(|b| {
            cfg.block(*b)
                .insns
                .iter()
                .any(|i| cfg.insn(*i).contains_flag(iflag::DECLARE_VAR))
        });
    if declared_by_passes {
        apply_variable_names(cfg, root);
        return;
    }
    // declare variables at the first assignment for the name guessing
    let all: Vec<_> = cfg.svar_list.clone();
    for v in &all {
        if let Some(cv) = cfg.svars[*v].code_var {
            cfg.code_vars[cv].is_declared = true;
        }
    }
    apply_variable_names(cfg, root);
    for v in &all {
        if let Some(cv) = cfg.svars[*v].code_var {
            cfg.code_vars[cv].is_declared = false;
        }
    }
    let arg_vars: Vec<_> = cfg
        .this_arg
        .iter()
        .chain(cfg.arg_regs.iter())
        .filter_map(|r| r.svar)
        .collect();
    for v in arg_vars {
        if let Some(cv) = cfg.svars[v].code_var {
            cfg.code_vars[cv].is_declared = true;
        }
    }
    let mut insns = Vec::new();
    collect_insns(cfg, Container::Region(region), &mut insns);
    for i in insns {
        if cfg.insn(i).contains_flag(iflag::DONT_GENERATE) {
            continue;
        }
        let Some(cv) = cfg
            .insn(i)
            .result
            .as_ref()
            .and_then(|r| r.svar)
            .and_then(|v| cfg.svars[v].code_var)
        else {
            continue;
        };
        if !cfg.code_vars[cv].is_declared {
            cfg.insns[i].flags |= iflag::DECLARE_VAR;
            cfg.code_vars[cv].is_declared = true;
        }
    }
}

/// Instructions of the container in generation order.
fn collect_insns(cfg: &MethodCfg, c: Container, out: &mut Vec<InsnId>) {
    match c {
        Container::Block(b) => out.extend(cfg.block(b).insns.iter().copied()),
        Container::Insns(i) => out.extend(cfg.regions.insn_conts[i].insns.iter().copied()),
        Container::Region(r) => {
            if let RegionKind::Loop { ty, .. } = &cfg.regions.arena[r].kind {
                match ty {
                    Some(LoopType::For { init, .. }) => out.push(*init),
                    Some(LoopType::ForEach { var_arg_insn, .. }) => out.push(*var_arg_insn),
                    None => {}
                }
            }
            for s in cfg.regions.sub_blocks(r) {
                collect_insns(cfg, s, out);
            }
        }
    }
}
