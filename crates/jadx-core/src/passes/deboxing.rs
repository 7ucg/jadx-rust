//! `DeboxingVisitor`: replace `valueOf` calls of primitive wrappers on literals with the literal.

use crate::model::cfg::MethodCfg;
use crate::model::info::MethodInfo;
use crate::model::insn::{iflag, InsnArg, InsnId, InsnKind, InsnNode, InsnType, InvokeType};
use crate::model::ssa::{ArgRef, SsaId};
use crate::model::types::ArgType;
use crate::passes::constructor::MethodEnv;
use crate::passes::insn_utils::replace_insn;

/// `valueOf` methods of the wrapper classes of the primitive types handled by the pass.
const VALUE_OF: [(&str, &str); 6] = [
    ("I", "java.lang.Integer"),
    ("Z", "java.lang.Boolean"),
    ("B", "java.lang.Byte"),
    ("S", "java.lang.Short"),
    ("C", "java.lang.Character"),
    ("J", "java.lang.Long"),
];

fn is_value_of(m: &MethodInfo) -> bool {
    if m.name != "valueOf" || m.arg_types.len() != 1 {
        return false;
    }
    let decl = m.decl_class.full_name();
    VALUE_OF.iter().any(|(desc, cls)| {
        decl == *cls
            && crate::model::info::type_signature(&m.arg_types[0]) == *desc
            && m.return_type.object_name().as_deref() == Some(cls)
    })
}

pub fn deboxing(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    let mut replaced = false;
    for block in cfg.blocks.clone() {
        let count = cfg.block(block).insns.len();
        for i in 0..count {
            let insn = cfg.block(block).insns[i];
            if cfg.insn(insn).ty != InsnType::Invoke {
                continue;
            }
            if let Some(new_insn) = check_for_replace(cfg, insn) {
                replace_insn(cfg, block, i, new_insn)?;
                replaced = true;
            }
        }
    }
    if replaced {
        crate::passes::const_inline::const_inline(cfg, env)?;
    }
    Ok(())
}

fn check_for_replace(cfg: &mut MethodCfg, insn: InsnId) -> Option<InsnNode> {
    let n = cfg.insn(insn);
    let InsnKind::Invoke { invoke_type, mth } = &n.kind else {
        return None;
    };
    if *invoke_type != InvokeType::Static || n.result.is_none() {
        return None;
    }
    if !is_value_of(mth) {
        return None;
    }
    let primitive_type = mth.arg_types[0].clone();
    let box_type = mth.return_type.clone();
    let InsnArg::Literal(lit) = n.args.first()?.clone() else {
        return None;
    };
    let res_arg = n.result.clone()?;
    let res_var = res_arg.svar?;
    let mut lit = lit;
    if is_need_explicit_cast(cfg, res_var, &primitive_type, &box_type) {
        lit.explicit_primitive = true;
    }
    lit.ty = primitive_type.clone();
    let forbid_inline = if can_change_type_to_primitive(cfg, res_var, &box_type) {
        let _ = cfg.set_arg_type(ArgRef::Result(insn), primitive_type);
        false
    } else {
        true
    };
    let mut const_insn = InsnNode::plain(InsnType::Const);
    const_insn.args.push(InsnArg::Literal(lit));
    const_insn.result = cfg.insn(insn).result.clone();
    if forbid_inline {
        const_insn.flags |= iflag::DONT_INLINE;
    }
    Some(const_insn)
}

fn is_need_explicit_cast(
    cfg: &MethodCfg,
    res_var: SsaId,
    primitive_type: &ArgType,
    box_type: &ArgType,
) -> bool {
    if *primitive_type == ArgType::long() {
        return true;
    }
    if *primitive_type != ArgType::int() {
        let mut use_types: Vec<ArgType> = Vec::new();
        let mut add = |t: ArgType| {
            if !use_types.contains(&t) {
                use_types.push(t);
            }
        };
        for u in &cfg.svars[res_var].uses {
            add(cfg.arg_type(*u));
            if let Some(r) = cfg.try_reg(*u) {
                add(r.init_type.clone());
            }
        }
        add(cfg.svars[res_var].ty.clone());
        use_types.retain(|t| t != box_type && t != primitive_type);
        return !use_types.is_empty();
    }
    false
}

fn can_change_type_to_primitive(cfg: &MethodCfg, res_var: SsaId, box_type: &ArgType) -> bool {
    let Some(cv) = cfg.svars[res_var].code_var else {
        return false;
    };
    for &ssa_var in &cfg.code_vars[cv].ssa_vars {
        if cfg.svar_type_immutable(ssa_var) {
            return false;
        }
        let Some(assign_insn) = cfg.svar_assign_insn(ssa_var) else {
            // method arg
            return false;
        };
        let a = cfg.insn(assign_insn);
        if matches!(a.ty, InsnType::Const | InsnType::Move)
            && a.args.first().is_some_and(|arg| {
                let t = match arg {
                    InsnArg::Literal(l) => l.ty.clone(),
                    InsnArg::Wrapped(w) => w.ty.clone(),
                    InsnArg::Named(n) => n.ty.clone(),
                    InsnArg::Register(r) => r
                        .svar
                        .map_or_else(ArgType::unknown_any, |v| cfg.svars[v].ty.clone()),
                };
                t.is_object()
            })
        {
            return false;
        }
        if let Some(r) = &a.result {
            if r.init_type.is_object() && r.init_type != *box_type {
                // some of related vars have another object type
                return false;
            }
        }
        for &use_arg in &cfg.svars[ssa_var].uses {
            let Some(parent) = use_arg.parent_insn() else {
                return false;
            };
            let p = cfg.insn(parent);
            if p.ty == InsnType::Invoke {
                let is_instance = match &p.kind {
                    InsnKind::Invoke { invoke_type, .. } => *invoke_type != InvokeType::Static,
                    _ => false,
                };
                if is_instance {
                    if let (Some(InsnArg::Register(inst)), Some(u)) =
                        (p.args.first(), cfg.try_reg(use_arg))
                    {
                        if inst.reg_num == u.reg_num && inst.svar == u.svar {
                            return false;
                        }
                    }
                }
            }
            if p.ty == InsnType::If {
                // `==` between two boxes compares references; unboxing one would compare values
                for k in 0..p.args.len() {
                    if ArgRef::Arg(parent, k) == use_arg {
                        continue;
                    }
                    if cfg.arg_type(ArgRef::Arg(parent, k)).is_object() {
                        return false;
                    }
                }
            }
        }
    }
    true
}
