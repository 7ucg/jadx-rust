//! Variable names and source lines from method metadata (`DebugInfoApplyVisitor`,
//! `DebugInfoAttachVisitor`).
//!
//! Ported in the late, already-built-SSA state of the method (at the `MOD_VISITOR` driver stage,
//! like [`apply_method_parameters`]) instead of before block splitting as the reference does:
//! instructions mostly keep their original decode offset through the earlier passes, so the
//! debug info (keyed by code offset and register number) can still be matched against the
//! current SSA variables and instructions directly, without needing to carry `RegDebugInfoAttr`
//! style per-argument attachments through SSA construction.

use std::collections::HashMap;

use jadx_input_api::{DebugInfo, LocalVar};

use crate::model::cfg::MethodCfg;
use crate::model::node::Root;
use crate::model::ssa::ArgRef;

/// `DebugInfoApplyVisitor.applyDebugInfo` + `DebugInfoAttachVisitor.attachSourceLines`: local
/// variable names found by the offset range of each SSA variable (`searchDebugInfoByOffset`) and
/// per-instruction source lines, incl. the `USE_LINES_HINTS` trust flag used by region building.
pub fn apply_debug_info(cfg: &mut MethodCfg, debug: &DebugInfo) {
    if !debug.lines.is_empty() {
        apply_source_lines(cfg, &debug.lines);
    }
    if !debug.locals.is_empty() {
        apply_local_var_names(cfg, &debug.locals);
    }
}

fn apply_source_lines(cfg: &mut MethodCfg, lines: &[(u32, u32)]) {
    let map: HashMap<u32, u32> = lines.iter().copied().collect();
    for insn in cfg.insns.iter_mut() {
        if insn.offset >= 0 {
            if let Some(&line) = map.get(&(insn.offset as u32)) {
                insn.source_line = line;
            }
        }
    }
    cfg.use_lines_hints = verify_debug_lines(lines);
}

/// `DebugInfoAttachVisitor.verifyDebugLines`
fn verify_debug_lines(lines: &[(u32, u32)]) -> bool {
    let min_line = lines.iter().map(|&(_, l)| l).min().unwrap_or(u32::MAX);
    if min_line < 3 {
        return false;
    }
    let mut counts: HashMap<u32, u32> = HashMap::new();
    for &(_, l) in lines {
        *counts.entry(l).or_insert(0) += 1;
    }
    // more than 3 repeats of the same line is not trusted (reference allows up to 3, e.g. for
    // indexed `for` loops)
    !counts.values().any(|&c| c > 3)
}

/// `DebugInfoApplyVisitor.searchDebugInfoByOffset`: the offset range of the SSA variable's
/// assignment and uses is compared against every local variable of the same register; the first
/// overlapping, valid and printable name is applied to its code variable.
fn apply_local_var_names(cfg: &mut MethodCfg, locals: &[LocalVar]) {
    for i in 0..cfg.svars.len() {
        let reg_num = cfg.svars[i].reg_num;
        let start_off = insn_offset(cfg, cfg.svars[i].assign);
        let end_off = cfg.svars[i]
            .uses
            .iter()
            .map(|&u| insn_offset(cfg, u))
            .max()
            .unwrap_or(start_off);
        let mut found = None;
        for lv in locals {
            if lv.reg != reg_num {
                continue;
            }
            if !crate::name_mapper::is_valid_and_printable(&lv.name) {
                continue;
            }
            if is_inside(start_off, lv.start, lv.end) || is_inside(end_off, lv.start, lv.end) {
                found = Some(lv.name.clone());
                break;
            }
        }
        if let Some(name) = found {
            if let Some(cv) = cfg.svars[i].code_var {
                cfg.code_vars[cv].name = Some(name);
            }
        }
    }
}

fn insn_offset(cfg: &MethodCfg, arg: ArgRef) -> i32 {
    arg.parent_insn()
        .map(|id| cfg.insn(id).offset)
        .unwrap_or(-1)
}

fn is_inside(v: i32, start: i32, end: i32) -> bool {
    start <= v && v <= end
}

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
