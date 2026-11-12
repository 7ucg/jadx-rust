//! Variable names from method metadata (`DebugInfoApplyVisitor`).

use crate::model::cfg::MethodCfg;
use crate::model::node::Root;

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
