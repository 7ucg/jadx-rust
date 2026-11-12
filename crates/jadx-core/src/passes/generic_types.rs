//! `GenericTypesVisitor`: attach the generic types of the variable to constructor calls.

use crate::model::cfg::MethodCfg;
use crate::model::insn::{GenericInfo, InsnId, InsnType};
use crate::model::node::Root;
use crate::model::types::ArgType;

pub fn generic_types(cfg: &mut MethodCfg, root: &Root) -> Result<(), String> {
    for block in cfg.blocks.clone() {
        for insn in cfg.block(block).insns.clone() {
            if cfg.insn(insn).ty == InsnType::Constructor {
                attach_generic_types_info(cfg, root, insn);
            }
        }
    }
    Ok(())
}

fn attach_generic_types_info(cfg: &mut MethodCfg, root: &Root, insn: InsnId) {
    let Some(res) = cfg.insn(insn).result.as_ref() else {
        return;
    };
    let Some(var) = res.svar else {
        return;
    };
    let Some(cv) = cfg.svars[var].code_var else {
        return;
    };
    let Some(arg_type) = cfg.code_vars[cv].ty.clone() else {
        return;
    };
    let Some(generics) = arg_type.generic_types() else {
        return;
    };
    let generics: Vec<ArgType> = generics.to_vec();
    let cls_type = match &cfg.insn(insn).kind {
        crate::model::insn::InsnKind::Constructor { call_mth, .. } => {
            call_mth.decl_class.arg_type().clone()
        }
        _ => return,
    };
    if let Some(ci) = root.resolve_class(&cls_type) {
        if root.classes[ci].generics.is_empty() {
            return;
        }
    }
    cfg.insns[insn].generic_info = Some(GenericInfo {
        types: generics,
        explicit: false,
    });
}
