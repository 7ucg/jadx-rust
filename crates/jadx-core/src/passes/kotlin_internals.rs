//! `ProcessKotlinInternals`: use variable names from the argument of Kotlin `Intrinsics` checks.

use std::collections::HashSet;

use crate::model::cfg::MethodCfg;
use crate::model::info::MethodInfo;
use crate::model::insn::{IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType};
use crate::model::node::{Root, ACC_STATIC};
use crate::name_mapper::is_valid_and_printable;
use crate::passes::insn_utils::svar_set_name;
use jadx_input_api::ConstValue;

const KOTLIN_INTERNAL_PKG: &str = "kotlin.jvm.internal.";
const KOTLIN_INTRINSICS_CLS_SHORT_NAME: &str = "Intrinsics";
const KOTLIN_INTRINSICS_CLS: &str = "kotlin.jvm.internal.Intrinsics";
const KOTLIN_VARNAME_SOURCE_MTH1: &str = "(Ljava/lang/Object;Ljava/lang/String;)V";
const KOTLIN_VARNAME_SOURCE_MTH2: &str =
    "(Ljava/lang/Object;Ljava/lang/String;Ljava/lang/String;)V";

/// Methods of the Kotlin intrinsics class that take a variable name (`None` without such a class).
fn name_methods(root: &Root) -> &Option<HashSet<MethodInfo>> {
    root.kotlin_name_methods.get_or_init(|| {
        let cls = search_intrinsics_class(root)?;
        let mut set = HashSet::new();
        for m in &root.classes[cls].methods {
            if m.access & ACC_STATIC == 0 {
                continue;
            }
            let id = m.info.short_id();
            if id.ends_with(KOTLIN_VARNAME_SOURCE_MTH1) || id.ends_with(KOTLIN_VARNAME_SOURCE_MTH2)
            {
                set.insert(m.info.clone());
            }
        }
        Some(set)
    })
}

fn search_intrinsics_class(root: &Root) -> Option<usize> {
    if let Some(c) = root.class_index(KOTLIN_INTRINSICS_CLS) {
        return Some(c);
    }
    let mut candidates = Vec::new();
    for ci in 0..root.classes.len() {
        if is_intrinsics_class(root, ci) {
            candidates.push(ci);
        }
    }
    if candidates.len() == 1 {
        Some(candidates[0])
    } else {
        None
    }
}

fn is_intrinsics_class(root: &Root, ci: usize) -> bool {
    let info = &root.classes[ci].info;
    if root.class_short_name(info) == KOTLIN_INTRINSICS_CLS_SHORT_NAME
        && root.class_full_name(info) == KOTLIN_INTRINSICS_CLS
    {
        return true;
    }
    if !info.full_name().starts_with(KOTLIN_INTERNAL_PKG) {
        return false;
    }
    let cls = &root.classes[ci];
    if cls.methods.len() < 5 {
        return false;
    }
    let count = cls
        .methods
        .iter()
        .filter(|m| {
            m.access & ACC_STATIC != 0 && m.info.short_id().ends_with(KOTLIN_VARNAME_SOURCE_MTH1)
        })
        .count();
    count > 2
}

pub fn process_kotlin_internals(cfg: &mut MethodCfg, root: &Root) -> Result<(), String> {
    let Some(methods) = name_methods(root) else {
        return Ok(());
    };
    for block in cfg.blocks.clone() {
        for insn in cfg.block(block).insns.clone() {
            if cfg.insn(insn).ty == InsnType::Invoke {
                process_invoke(cfg, root, methods, insn);
            }
        }
    }
    Ok(())
}

fn process_invoke(cfg: &mut MethodCfg, root: &Root, methods: &HashSet<MethodInfo>, insn: InsnId) {
    let args_count = cfg.insn(insn).args.len();
    if args_count < 2 {
        return;
    }
    let InsnKind::Invoke { mth, .. } = &cfg.insn(insn).kind else {
        return;
    };
    if !methods.contains(mth) {
        return;
    }
    let InsnArg::Register(var_arg) = &cfg.insn(insn).args[0] else {
        return;
    };
    let Some(var) = var_arg.svar else {
        return;
    };
    let str_index = match args_count {
        2 => 1,
        3 => 2,
        _ => return,
    };
    if let Some(s) = get_const_string(cfg, root, insn, str_index) {
        let name = trim_name(&s);
        if is_valid_and_printable(&name) {
            // the `Intrinsics` argument always has a code variable at this point
            let _ = svar_set_name(cfg, var, &name);
        }
    }
}

fn get_const_string(
    cfg: &mut MethodCfg,
    root: &Root,
    insn: InsnId,
    index: usize,
) -> Option<String> {
    let InsnArg::Wrapped(w) = &cfg.insn(insn).args[index] else {
        return None;
    };
    let const_insn = w.insn;
    let n = cfg.insn(const_insn);
    match n.ty {
        InsnType::ConstStr => {
            let InsnKind::ConstStr(units) = &n.kind else {
                return None;
            };
            Some(String::from_utf16_lossy(units))
        }
        InsnType::Sget => {
            // revert const field inline
            let InsnKind::Index(IndexValue::Field(f)) = &n.kind else {
                return None;
            };
            let field = root.resolve_field(f)?;
            let ConstValue::Str(units) = field.const_value.as_ref()? else {
                return None;
            };
            let units = units.clone();
            let cs = InsnNode::new(InsnType::ConstStr, InsnKind::ConstStr(units.clone()));
            let id = cfg.add_insn(cs);
            let arg = cfg.wrap_arg(id).ok()?;
            cfg.replace_arg(insn, index, arg);
            Some(String::from_utf16_lossy(&units))
        }
        _ => None,
    }
}

fn trim_name(s: &str) -> String {
    if let Some(r) = s.strip_prefix("$this$") {
        return r.to_string();
    }
    if let Some(r) = s.strip_prefix('$') {
        return r.to_string();
    }
    s.to_string()
}
