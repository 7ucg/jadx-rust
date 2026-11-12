//! Variable names (`NameGen`): unique names for declared variables and the names used for
//! variable references.

use std::collections::{HashMap, HashSet};

use super::class_gen::GenEnv;
use crate::model::cfg::MethodCfg;
use crate::model::insn::RegisterArg;
use crate::model::ssa::CodeVarId;
use crate::name_mapper;

pub const THIS_ARG_NAME: &str = "this";
pub const SUPER_ARG_NAME: &str = "super";

/// `NameGen`: names assigned during the generation of one method are kept here, the variables of
/// the method are left untouched.
#[derive(Debug, Clone, Default)]
pub struct NameGen {
    var_names: HashSet<String>,
    fallback: bool,
    assigned: HashMap<CodeVarId, String>,
}

impl NameGen {
    /// `new NameGen(mth, classGen)` for method `mth` of class `cls`.
    pub fn new(env: &GenEnv<'_>, cls: usize, outer: Option<&NameGen>, fallback: bool) -> Self {
        let mut ng = NameGen {
            var_names: HashSet::new(),
            fallback,
            assigned: HashMap::new(),
        };
        if let Some(o) = outer {
            ng.inherit_used_names(o);
        }
        ng.add_names_used_in_class(env, cls);
        ng
    }

    pub fn inherit_used_names(&mut self, other: &NameGen) {
        self.var_names.extend(other.var_names.iter().cloned());
    }

    fn add_names_used_in_class(&mut self, env: &GenEnv<'_>, cls: usize) {
        let node = &env.root.classes[cls];
        for f in &node.fields {
            if f.access & crate::model::node::ACC_STATIC != 0 {
                self.var_names
                    .insert(f.alias.clone().unwrap_or_else(|| f.info.name.clone()));
            }
        }
        for &inner in &env.inner_classes[cls] {
            self.var_names
                .insert(env.short_name(&env.root.classes[inner].info));
        }
        // all root package names avoid collisions with full class names
        self.var_names.extend(env.root_pkgs.iter().cloned());
    }

    /// Name of the variable: assigned during generation, else set before (`CodeVar.getName`).
    pub fn var_name(&self, cfg: &MethodCfg, cv: CodeVarId) -> Option<String> {
        self.assigned
            .get(&cv)
            .cloned()
            .or_else(|| cfg.code_vars[cv].name.clone())
    }

    /// `assignArg`: name of a variable at its declaration.
    pub fn assign_arg(&mut self, cfg: &MethodCfg, cv: CodeVarId) -> String {
        if self.fallback {
            return fallback_name_cv(cfg, cv);
        }
        if cfg.code_vars[cv].is_this {
            return THIS_ARG_NAME.to_string();
        }
        let base = self.make_arg_name(cfg, cv);
        let name = self.unique_var_name(&base);
        self.assigned.insert(cv, name.clone());
        name
    }

    /// `assignNamedArg`: unique name for a named argument.
    pub fn assign_named_arg(&mut self, name: &str) -> String {
        if self.fallback {
            return name.to_string();
        }
        self.unique_var_name(name)
    }

    /// `useArg`: name of a variable reference.
    pub fn use_arg(&self, cfg: &MethodCfg, reg: &RegisterArg) -> String {
        match self.reg_name(cfg, reg) {
            Some(n) if !self.fallback => n,
            _ => format!("r{}", reg.reg_num),
        }
    }

    /// `RegisterArg.getName`
    pub fn reg_name(&self, cfg: &MethodCfg, reg: &RegisterArg) -> Option<String> {
        if reg.is_super {
            return Some(SUPER_ARG_NAME.to_string());
        }
        if reg.is_this {
            return Some(THIS_ARG_NAME.to_string());
        }
        let cv = cfg.svars[reg.svar?].code_var?;
        self.var_name(cfg, cv)
    }

    /// `getLoopLabel`
    pub fn loop_label(&mut self, loop_id: usize) -> String {
        let name = format!("loop{loop_id}");
        self.var_names.insert(name.clone());
        name
    }

    fn unique_var_name(&mut self, name: &str) -> String {
        if !self.var_names.contains(name) {
            self.var_names.insert(name.to_string());
            return name.to_string();
        }
        // code duplication reuse same variable in different places:
        // parse variable name and increment index (the pattern `.*(\d+)$` captures the last digit)
        let (base, mut i) = match name.chars().next_back().and_then(|c| c.to_digit(10)) {
            Some(d) => (&name[..name.len() - 1], 1 + i64::from(d)),
            None => (name, 2),
        };
        loop {
            let new_name = format!("{base}{i}");
            i += 1;
            if !self.var_names.contains(&new_name) {
                self.var_names.insert(new_name.clone());
                return new_name;
            }
        }
    }

    fn make_arg_name(&self, cfg: &MethodCfg, cv: CodeVarId) -> String {
        match self.var_name(cfg, cv) {
            Some(n) if name_mapper::is_valid_and_printable(&n) => n,
            _ => fallback_name_cv(cfg, cv),
        }
    }
}

/// `getFallbackName(CodeVar)`
fn fallback_name_cv(cfg: &MethodCfg, cv: CodeVarId) -> String {
    match cfg.code_vars[cv].ssa_vars.first() {
        None => "v".to_string(),
        Some(&v) => {
            let var = &cfg.svars[v];
            let reg = cfg.try_reg(var.assign).map_or(var.reg_num, |r| r.reg_num);
            format!("r{reg}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let mut ng = NameGen::default();
        assert_eq!(ng.unique_var_name("str"), "str");
        assert_eq!(ng.unique_var_name("str"), "str2");
        assert_eq!(ng.unique_var_name("str"), "str3");
        assert_eq!(ng.unique_var_name("str2"), "str4");
        assert_eq!(ng.unique_var_name("a9"), "a9");
        // only the last digit is the counter
        assert_eq!(ng.unique_var_name("a9"), "a10");
        assert_eq!(ng.unique_var_name("a12"), "a12");
        assert_eq!(ng.unique_var_name("a12"), "a13");
        assert_eq!(ng.loop_label(3), "loop3");
    }
}
