//! `FinishTypeInference`: check the resulting types.

use std::collections::BTreeSet;

use crate::model::cfg::MethodCfg;
use crate::model::ssa::SsaId;
use crate::model::types::ArgType;

/// `FinishTypeInference.visit`
pub fn finish_type_inference(cfg: &mut MethodCfg) -> Result<(), String> {
    if cfg.svar_list.is_empty() {
        return Ok(());
    }
    for var in cfg.svar_list.clone() {
        if !cfg.svars[var].ty.is_type_known() {
            let info = detailed_var_info(cfg, var);
            cfg.warnings
                .push(format!("Type inference failed for: {info}"));
        }
        let Some(cv) = cfg.svars[var].code_var else {
            return Err(format!(
                "Code variable not set in r{}v{}",
                cfg.svars[var].reg_num, cfg.svars[var].version
            ));
        };
        if cfg.code_vars[cv].ty.is_none() {
            cfg.code_vars[cv].ty = Some(ArgType::unknown_any());
        }
    }
    Ok(())
}

/// `SSAVar.getDetailedVarInfo` (without debug info names).
pub fn detailed_var_info(cfg: &MethodCfg, var: SsaId) -> String {
    let v = &cfg.svars[var];
    let mut types: BTreeSet<String> = BTreeSet::new();
    let mut args = Vec::with_capacity(1 + v.uses.len());
    args.push(v.assign);
    args.extend(v.uses.iter().copied());
    for at in args {
        let init_type = &cfg.reg(at).init_type;
        if init_type.is_type_known() {
            types.insert(init_type.to_string());
        }
        let ty = cfg.reg_type(at);
        if ty.is_type_known() {
            types.insert(ty.to_string());
        }
    }
    let mut s = format!("r{}v{}", v.reg_num, v.version);
    if !types.is_empty() {
        let list: Vec<String> = types.into_iter().collect();
        s.push_str(&format!(", types: [{}]", list.join(", ")));
    }
    s
}
