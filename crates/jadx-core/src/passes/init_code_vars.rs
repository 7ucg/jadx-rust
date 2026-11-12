//! `InitCodeVariables`: group SSA variables connected by phi nodes into code variables.

use crate::model::cfg::MethodCfg;
use crate::model::insn::{InsnArg, InsnId};
use crate::model::ssa::{ArgRef, CodeVar, SsaId};

pub fn init_code_vars(cfg: &mut MethodCfg) -> Result<(), String> {
    if cfg.this_arg.is_some() {
        init_code_var_for(cfg, ArgRef::This);
    }
    for i in 0..cfg.arg_regs.len() {
        init_code_var_for(cfg, ArgRef::Method(i));
    }
    for v in cfg.svar_list.clone() {
        init_code_var(cfg, v)?;
    }
    Ok(())
}

fn init_code_var_for(cfg: &mut MethodCfg, at: ArgRef) {
    let var = match cfg.reg(at).svar {
        Some(v) => v,
        None => {
            let (reg, ver) = {
                let r = cfg.reg(at);
                let v = cfg
                    .svar_list
                    .iter()
                    .filter(|s| cfg.svars[**s].reg_num == r.reg_num)
                    .map(|s| cfg.svars[*s].version as i64)
                    .max()
                    .unwrap_or(-1);
                (r.reg_num, (v + 1) as u32)
            };
            cfg.make_new_svar(reg, ver, at)
        }
    };
    // errors cannot occur for single variables
    let _ = init_code_var(cfg, var);
}

fn init_code_var(cfg: &mut MethodCfg, var: SsaId) -> Result<(), String> {
    if cfg.svars[var].code_var.is_some() {
        return Ok(());
    }
    let mut cv = CodeVar::default();
    let assign = cfg.reg(cfg.svars[var].assign);
    if assign.is_this {
        cv.name = Some("this".to_string());
        cv.is_this = true;
    }
    if assign.method_arg || assign.custom_declare {
        cv.is_declared = true;
    }
    let cv_id = cfg.code_vars.len();
    cfg.code_vars.push(cv);
    set_code_var(cfg, var, cv_id)
}

fn set_code_var(cfg: &mut MethodCfg, var: SsaId, cv: usize) -> Result<(), String> {
    let phi_list = cfg.svar_phi_list(var);
    if phi_list.is_empty() {
        cfg.svar_set_code_var(var, cv);
        return Ok(());
    }
    let mut vars: Vec<SsaId> = vec![var];
    collect_connected_vars(cfg, &phi_list, &mut vars);
    if vars.len() > 1 {
        let mut im: Vec<crate::model::types::ArgType> = Vec::new();
        for v in &vars {
            if let Some(t) = cfg.svar_immutable_type(*v) {
                if t.is_type_known() && !im.contains(&t) {
                    im.push(t);
                }
            }
        }
        if im.len() == 1 {
            cfg.code_vars[cv].ty = Some(im.remove(0));
        } else if im.len() > 1 {
            return Err("Several immutable types in one variable".into());
        }
    }
    for v in vars {
        if let Some(other) = cfg.svars[v].code_var {
            let o = cfg.code_vars[other].clone();
            cfg.code_vars[cv].merge_flags_from(&o);
        }
        cfg.svar_set_code_var(v, cv);
    }
    Ok(())
}

fn collect_connected_vars(cfg: &MethodCfg, phis: &[InsnId], vars: &mut Vec<SsaId>) {
    for &phi in phis {
        if let Some(rv) = cfg.insn(phi).result.as_ref().and_then(|r| r.svar) {
            if !vars.contains(&rv) {
                vars.push(rv);
                collect_connected_vars(cfg, &cfg.svar_phi_list(rv), vars);
            }
        }
        for a in &cfg.insn(phi).args {
            if let InsnArg::Register(r) = a {
                if let Some(sv) = r.svar {
                    if !vars.contains(&sv) {
                        vars.push(sv);
                        collect_connected_vars(cfg, &cfg.svar_phi_list(sv), vars);
                    }
                }
            }
        }
    }
}
