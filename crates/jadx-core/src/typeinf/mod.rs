//! Type inference (`TypeInferenceVisitor` and helpers).

pub mod update;

use crate::method_utils::{
    first_arg_offset, has_instance_arg, insn_method_details, method_generic_return_type,
    method_origin_decl_class, MethodDetails,
};
use crate::model::cfg::MethodCfg;
use crate::model::insn::{iflag, IndexValue, InsnArg, InsnKind, InsnType, InvokeType};
use crate::model::node::Root;
use crate::model::ssa::{ArgRef, BoundKind, SsaId, TypeBound};
use crate::model::types::ArgType;
use update::{TypeUpdate, UpdateResult};

/// `TypeInferenceVisitor.visit`: types of SSA variables from their bounds.
pub fn run_type_inference(
    cfg: &mut MethodCfg,
    root: &Root,
    cls: usize,
    mth: usize,
) -> Result<(), String> {
    let upd = TypeUpdate::new(root, cls, mth, cfg.units_count);
    cfg.types_done = true;
    let r = (|| -> Result<(), String> {
        assign_immutable_types(cfg);
        init_type_bounds(cfg, root, cls, mth)?;
        run_type_propagation(cfg, &upd)
    })();
    if let Err(e) = r {
        cfg.errors.push(format!("Type inference failed: {e}"));
    }
    Ok(())
}

fn assign_immutable_types(cfg: &mut MethodCfg) {
    for v in cfg.svar_list.clone() {
        if let Some(t) = ssa_immutable_type(cfg, v) {
            cfg.svar_mark_immutable(v, t);
        }
    }
}

fn ssa_immutable_type(cfg: &MethodCfg, var: SsaId) -> Option<ArgType> {
    let assign = cfg.reg(cfg.svars[var].assign);
    if assign.immutable_type {
        return Some(assign.init_type.clone());
    }
    for u in &cfg.svars[var].uses {
        let r = cfg.reg(*u);
        if r.immutable_type {
            return Some(r.init_type.clone());
        }
    }
    None
}

fn init_type_bounds(
    cfg: &mut MethodCfg,
    root: &Root,
    cls: usize,
    mth: usize,
) -> Result<(), String> {
    let _ = (cls, mth);
    for v in cfg.svar_list.clone() {
        attach_bounds(cfg, root, v)?;
    }
    for v in cfg.svar_list.clone() {
        merge_phi_bounds(cfg, v);
    }
    Ok(())
}

fn merge_phi_bounds(cfg: &mut MethodCfg, var: SsaId) {
    let phis = cfg.svars[var].used_in_phi.clone().unwrap_or_default();
    for phi in phis {
        let mut sources: Vec<SsaId> = Vec::new();
        if let Some(v) = cfg.insn(phi).result.as_ref().and_then(|r| r.svar) {
            sources.push(v);
        }
        for a in &cfg.insn(phi).args {
            if let InsnArg::Register(r) = a {
                if let Some(v) = r.svar {
                    sources.push(v);
                }
            }
        }
        for src in sources {
            for b in cfg.svars[src].bounds.clone() {
                cfg.svar_add_bound(var, b);
            }
        }
    }
}

fn add_bound(cfg: &mut MethodCfg, var: SsaId, bound: Option<TypeBound>) {
    let Some(b) = bound else { return };
    let ok = match &b {
        TypeBound::Const { ty, .. } => *ty != ArgType::unknown_any(),
        _ => true,
    };
    if ok {
        cfg.svar_add_bound(var, b);
    }
}

fn attach_bounds(cfg: &mut MethodCfg, root: &Root, var: SsaId) -> Result<(), String> {
    cfg.svars[var].bounds.clear();
    let assign = cfg.svars[var].assign;
    add_assign_bound(cfg, root, var, assign)?;
    for u in cfg.svars[var].uses.clone() {
        let b = make_use_bound(cfg, root, u)?;
        add_bound(cfg, var, b);
    }
    Ok(())
}

fn const_bound(kind: BoundKind, ty: ArgType) -> Option<TypeBound> {
    Some(TypeBound::Const {
        kind,
        ty,
        arg: None,
    })
}

fn add_assign_bound(
    cfg: &mut MethodCfg,
    root: &Root,
    var: SsaId,
    assign: ArgRef,
) -> Result<(), String> {
    if let Some(im) = cfg.svar_immutable_type(var) {
        add_bound(cfg, var, const_bound(BoundKind::Assign, im));
        return Ok(());
    }
    let init_type = cfg.reg(assign).init_type.clone();
    let insn = assign.parent_insn();
    let Some(insn) = insn.filter(|i| cfg.insn(*i).result.is_some()) else {
        add_bound(cfg, var, const_bound(BoundKind::Assign, init_type));
        return Ok(());
    };
    let res_init = cfg
        .insn(insn)
        .result
        .as_ref()
        .map(|r| r.init_type.clone())
        .unwrap();
    match cfg.insn(insn).ty {
        InsnType::NewInstance => {
            let t = match &cfg.insn(insn).kind {
                InsnKind::Index(IndexValue::Type(t)) => t.clone(),
                _ => return Err("new-instance without type".into()),
            };
            add_bound(cfg, var, const_bound(BoundKind::Assign, t));
        }
        InsnType::Constructor => {
            let (cls_info, is_new) = match &cfg.insn(insn).kind {
                InsnKind::Constructor {
                    call_mth,
                    call_type,
                } => (
                    call_mth.decl_class.clone(),
                    *call_type == crate::model::insn::CallType::Constructor,
                ),
                _ => return Err("constructor without call".into()),
            };
            let mut t = cls_info.arg_type().clone();
            if is_new {
                // `replaceAnonymousType`
                if let Some(ci) = root.resolve_class(&t) {
                    let c = &root.classes[ci];
                    if c.dont_generate {
                        if let Some(a) = &c.anonymous {
                            if a.inline_type == crate::model::node::InlineType::Constructor {
                                t = a.base_type.clone();
                            }
                        }
                    }
                }
            }
            add_bound(cfg, var, const_bound(BoundKind::Assign, t));
        }
        InsnType::Const => {
            let t = match cfg.insn(insn).args.first() {
                Some(InsnArg::Literal(l)) => l.ty.clone(),
                _ => return Err("const without literal".into()),
            };
            add_bound(cfg, var, const_bound(BoundKind::Assign, t));
        }
        InsnType::MoveException => match cfg.insn(insn).exc_handler {
            Some(h) => {
                let types: Vec<ArgType> = cfg.handlers[h]
                    .catch_types
                    .iter()
                    .map(|c| c.arg_type().clone())
                    .collect();
                for t in types {
                    add_bound(cfg, var, const_bound(BoundKind::Assign, t));
                }
            }
            None => add_bound(cfg, var, const_bound(BoundKind::Assign, res_init)),
        },
        InsnType::Invoke => {
            let b = make_assign_invoke_bound(cfg, root, insn);
            add_bound(cfg, var, Some(b));
        }
        InsnType::Iget => {
            let b = if res_init.contains_type_variable() {
                TypeBound::FieldGetAssign {
                    insn,
                    init_type: res_init,
                }
            } else {
                TypeBound::Const {
                    kind: BoundKind::Assign,
                    ty: res_init,
                    arg: None,
                }
            };
            add_bound(cfg, var, Some(b));
        }
        InsnType::CheckCast => {
            if !cfg.insn(insn).contains_flag(iflag::SOFT_CAST) {
                add_bound(cfg, var, Some(TypeBound::CheckCastAssign { insn }));
            }
        }
        _ => add_bound(cfg, var, const_bound(BoundKind::Assign, res_init)),
    }
    Ok(())
}

fn make_assign_invoke_bound(cfg: &MethodCfg, root: &Root, insn: usize) -> TypeBound {
    let (invoke_type, call) = match &cfg.insn(insn).kind {
        InsnKind::Invoke { invoke_type, mth } => (*invoke_type, mth.clone()),
        _ => unreachable!("invoke insn expected"),
    };
    let mut bound_type = call.return_type.clone();
    if let Some(generic) = method_generic_return_type(root, cfg.insn(insn)) {
        if generic.contains_type_variable() {
            if !cfg.insn(insn).args.is_empty()
                && invoke_type != InvokeType::Static
                && invoke_type != InvokeType::Super
            {
                return TypeBound::InvokeAssign {
                    insn,
                    generic_return: generic,
                };
            }
        } else {
            bound_type = generic;
        }
    }
    TypeBound::Const {
        kind: BoundKind::Assign,
        ty: bound_type,
        arg: None,
    }
}

fn make_use_bound(cfg: &MethodCfg, root: &Root, at: ArgRef) -> Result<Option<TypeBound>, String> {
    let Some(insn) = at.parent_insn() else {
        return Ok(None);
    };
    if matches!(cfg.insn(insn).ty, InsnType::Invoke | InsnType::Constructor) {
        if let Some(b) = make_invoke_use_bound(cfg, root, insn, at)? {
            return Ok(Some(b));
        }
    }
    if cfg.insn(insn).ty == InsnType::CheckCast && cfg.insn(insn).contains_flag(iflag::SOFT_CAST) {
        return Ok(None);
    }
    Ok(Some(TypeBound::Const {
        kind: BoundKind::Use,
        ty: cfg.reg(at).init_type.clone(),
        arg: Some(at),
    }))
}

fn make_invoke_use_bound(
    cfg: &MethodCfg,
    root: &Root,
    invoke: usize,
    reg: ArgRef,
) -> Result<Option<TypeBound>, String> {
    if !has_instance_arg(cfg.insn(invoke)) {
        return Ok(None);
    }
    let Some(details) = insn_method_details(root, cfg.insn(invoke)) else {
        return Ok(None);
    };
    let instance = ArgRef::Arg(invoke, 0);
    if instance != reg {
        let ArgRef::Arg(_, k) = reg else {
            return Ok(None);
        };
        let idx = k - first_arg_offset(cfg.insn(invoke));
        let arg_types = details.arg_types(root);
        let arg_type = arg_types
            .get(idx)
            .ok_or_else(|| "invoke argument index out of range".to_string())?;
        if !arg_type.contains_type_variable() {
            return Ok(None);
        }
        return Ok(Some(TypeBound::InvokeUse {
            insn: invoke,
            arg: reg,
            generic_arg: arg_type.clone(),
        }));
    }
    if let MethodDetails::App(c, m) = details {
        let decl = method_origin_decl_class(root, c, m);
        return Ok(Some(TypeBound::Const {
            kind: BoundKind::Use,
            ty: decl,
            arg: Some(reg),
        }));
    }
    Ok(None)
}

fn run_type_propagation(cfg: &mut MethodCfg, upd: &TypeUpdate) -> Result<(), String> {
    for v in cfg.svar_list.clone() {
        set_immutable_type(cfg, upd, v)?;
    }
    for v in cfg.svar_list.clone() {
        set_best_type(cfg, upd, v)?;
    }
    Ok(())
}

fn is_overflow(e: &str) -> bool {
    e.starts_with("Type inference error: updates count limit")
}

fn set_immutable_type(cfg: &mut MethodCfg, upd: &TypeUpdate, var: SsaId) -> Result<(), String> {
    if let Some(im) = cfg.svar_immutable_type(var) {
        match upd.apply_with_wider_ign_same(cfg, var, &im) {
            Ok(_) => {}
            Err(e) if is_overflow(&e) => return Err(e),
            Err(e) => cfg
                .warnings
                .push(format!("Failed to set immutable type for var: {e}")),
        }
    }
    Ok(())
}

fn set_best_type(cfg: &mut MethodCfg, upd: &TypeUpdate, var: SsaId) -> Result<(), String> {
    match calculate_from_bounds(cfg, upd, var) {
        Ok(()) => Ok(()),
        Err(e) if is_overflow(&e) => Err(e),
        Err(e) => {
            cfg.warnings
                .push(format!("Failed to calculate best type for var: {e}"));
            Ok(())
        }
    }
}

fn calculate_from_bounds(cfg: &mut MethodCfg, upd: &TypeUpdate, var: SsaId) -> Result<(), String> {
    let Some(best) = select_best_type_from_bounds(cfg, upd, var) else {
        return Ok(());
    };
    let r = upd.apply(cfg, var, &best)?;
    let _ = r == UpdateResult::Reject;
    Ok(())
}

/// `bounds.stream().map(getType).filter(nonNull).max(comparator)`
fn select_best_type_from_bounds(cfg: &MethodCfg, upd: &TypeUpdate, var: SsaId) -> Option<ArgType> {
    let mut best: Option<ArgType> = None;
    for b in &cfg.svars[var].bounds {
        let Some(t) = upd.bound_type(cfg, None, b) else {
            continue;
        };
        best = Some(match best {
            None => t,
            Some(cur) => {
                if upd.compare.comparator(&cur, &t) >= 0 {
                    cur
                } else {
                    t
                }
            }
        });
    }
    best
}
