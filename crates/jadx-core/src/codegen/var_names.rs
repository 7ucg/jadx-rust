//! `ApplyVariableNames`: guess variable names from the instruction that assigns the variable or
//! from the variable type.

use super::string_utils::{capitalize_first_char, escape_name};
use crate::model::cfg::MethodCfg;
use crate::model::info::ClassInfo;
use crate::model::insn::{InsnArg, InsnId, InsnKind, InsnType};
use crate::model::node::Root;
use crate::model::ssa::{CodeVarId, SsaId};
use crate::model::types::{ArgType, CLASS_CLASS};
use crate::name_mapper::is_valid_and_printable;

const OBJ_ALIAS: &[(&str, &str)] = &[
    ("java.lang.String", "str"),
    ("java.lang.Class", "cls"),
    ("java.lang.Throwable", "th"),
    ("java.lang.Object", "obj"),
    ("java.util.Iterator", "it"),
    ("java.util.HashMap", "map"),
    ("java.lang.Boolean", "bool"),
    ("java.lang.Short", "sh"),
    ("java.lang.Integer", "num"),
    ("java.lang.Character", "ch"),
    ("java.lang.Byte", "b"),
    ("java.lang.Float", "f"),
    ("java.lang.Long", "l"),
    ("java.lang.Double", "d"),
    ("java.lang.StringBuilder", "sb"),
    ("java.lang.Exception", "exc"),
];

const GOOD_VAR_NAMES: &[&str] = &["size", "length", "list", "map", "next"];
const INVOKE_PREFIXES: &[&str] = &["get", "set", "to", "parse", "read", "format"];

/// The visitor over the variables of one method (`ApplyVariableNames.visit`): names are applied
/// to declared code variables that have no valid name yet.
pub fn apply_variable_names(cfg: &mut MethodCfg, root: &Root) {
    let svars = cfg.svar_list.clone();
    for v in svars {
        let Some(cv) = cfg.svars[v].code_var else {
            continue;
        };
        if let Some(name) = guess_name(cfg, root, cv) {
            cfg.code_vars[cv].name = Some(name);
        }
    }
}

fn guess_name(cfg: &MethodCfg, root: &Root, cv: CodeVarId) -> Option<String> {
    let var = &cfg.code_vars[cv];
    if var.is_this {
        return Some(super::name_gen::THIS_ARG_NAME.to_string());
    }
    if !var.is_declared {
        // name is not used in code
        return None;
    }
    if var.name.as_deref().is_some_and(is_valid_and_printable) {
        // the current name is valid, keep it
        return None;
    }
    let ty = var.ty.clone().unwrap_or_else(ArgType::unknown_any);
    if !var.ssa_vars.is_empty() {
        let mth_arg = var.ssa_vars.iter().any(|&v| {
            cfg.try_reg(cfg.svars[v].assign)
                .is_some_and(|r| r.method_arg)
        });
        if mth_arg {
            // for method args use defined type and ignore usage
            return Some(make_name_for_type(root, &ty));
        }
        for &v in &var.ssa_vars {
            if let Some(name) = make_name_for_ssa_var(cfg, root, v, &ty) {
                return Some(name);
            }
        }
    }
    Some(make_name_for_type(root, &ty))
}

fn make_name_for_ssa_var(
    cfg: &MethodCfg,
    root: &Root,
    v: SsaId,
    cv_type: &ArgType,
) -> Option<String> {
    if let Some(cv) = cfg.svars[v].code_var {
        if let Some(n) = &cfg.code_vars[cv].name {
            return Some(n.clone());
        }
    }
    let assign = cfg.svar_assign_insn(v)?;
    let name = make_name_from_insn(cfg, root, assign, cv_type)?;
    is_valid_and_printable(&name).then_some(name)
}

fn make_name_from_insn(
    cfg: &MethodCfg,
    root: &Root,
    insn: InsnId,
    cv_type: &ArgType,
) -> Option<String> {
    let node = &cfg.insns[insn];
    match node.ty {
        InsnType::Invoke => make_name_from_invoke(root, node, cv_type),
        InsnType::Constructor => {
            let InsnKind::Constructor { call_mth, .. } = &node.kind else {
                return None;
            };
            if let Some((c, m)) = root.resolve_method(call_mth) {
                if root.classes[c].methods[m].anonymous_constructor {
                    // don't use name of anonymous class
                    return None;
                }
            }
            Some(make_name_for_class(root, &call_mth.decl_class))
        }
        InsnType::ArrayLength => Some("length".to_string()),
        InsnType::Arith | InsnType::Ternary | InsnType::Cast => {
            for arg in &node.args {
                if let InsnArg::Wrapped(w) = arg {
                    if let Some(n) = make_name_from_insn(cfg, root, w.insn, cv_type) {
                        return Some(n);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn make_name_for_type(root: &Root, ty: &ArgType) -> String {
    if let Some(p) = ty.as_primitive() {
        return p.short_name().to_lowercase();
    }
    if ty.is_array() {
        return format!("{}Arr", make_name_for_type(root, ty.array_root_element()));
    }
    make_name_for_object(root, ty)
}

fn make_name_for_object(root: &Root, ty: &ArgType) -> String {
    if let ArgType::GenericType { name, .. } = ty {
        return escape_name(&name.to_lowercase());
    }
    if ty.is_object() {
        let obj = ty.object_name().unwrap_or_default();
        if let Some(alias) = alias_for_object(&obj) {
            return alias.to_string();
        }
        return match root.ctx.class_info(ty) {
            Ok(ci) => make_name_for_checked_class(&ci),
            Err(_) => escape_name(&ty.to_string()),
        };
    }
    escape_name(&ty.to_string())
}

fn make_name_for_checked_class(ci: &ClassInfo) -> String {
    let short = ci.short_name();
    if let Some(v) = from_name(short) {
        return v;
    }
    let lower = escape_name(&short.to_lowercase());
    if short == lower {
        return format!("{lower}Var");
    }
    lower
}

fn make_name_for_class(_root: &Root, ci: &ClassInfo) -> String {
    if let Some(alias) = alias_for_object(ci.full_name()) {
        return alias.to_string();
    }
    make_name_for_checked_class(ci)
}

fn from_name(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    if name.to_uppercase() == name {
        // all characters are upper case
        return Some(name.to_lowercase());
    }
    let v1: String = first.to_lowercase().chain(chars).collect();
    if v1 != name {
        return Some(v1);
    }
    if name.encode_utf16().count() < 3 {
        return Some(format!("{name}Var"));
    }
    None
}

fn alias_for_object(name: &str) -> Option<&'static str> {
    OBJ_ALIAS.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

fn make_name_from_invoke(
    root: &Root,
    insn: &crate::model::insn::InsnNode,
    cv_type: &ArgType,
) -> Option<String> {
    let InsnKind::Invoke { mth: call_mth, .. } = &insn.kind else {
        return None;
    };
    let name = call_mth.name.as_str();
    let decl_class = &call_mth.decl_class;
    if name == "getInstance" {
        // e.g. Cipher.getInstance
        return Some(make_name_for_class(root, decl_class));
    }
    if let Some(short) = cut_prefix(name) {
        return from_name(short);
    }
    if name == "iterator" {
        return Some("it".to_string());
    }
    if name == "toString" {
        return Some(make_name_for_class(root, decl_class));
    }
    if name == "forName" && decl_class.arg_type() == &ArgType::class() {
        return alias_for_object(CLASS_CLASS).map(str::to_string);
    }
    // use method name as a variable name not the best idea in most cases
    if !GOOD_VAR_NAMES.contains(&name) {
        let type_name = make_name_for_type(root, cv_type);
        if !type_name.eq_ignore_ascii_case(name) {
            return Some(format!("{type_name}{}", capitalize_first_char(name)));
        }
    }
    Some(name.to_string())
}

fn cut_prefix(name: &str) -> Option<&str> {
    INVOKE_PREFIXES.iter().find_map(|p| name.strip_prefix(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_type_names() {
        assert_eq!(from_name("URL").as_deref(), Some("url"));
        assert_eq!(from_name("List").as_deref(), Some("list"));
        assert_eq!(from_name("Ab").as_deref(), Some("ab"));
        assert_eq!(from_name("x").as_deref(), Some("xVar"));
        assert_eq!(from_name("abc"), None);
        assert_eq!(from_name(""), None);
        assert_eq!(cut_prefix("getName"), Some("Name"));
        assert_eq!(cut_prefix("run"), None);
    }
}
