//! Output names of classes, packages, fields and methods: the name in the bytecode unless the
//! renaming pass assigned another one.

use super::info::{ClassInfo, FieldInfo, MethodInfo};
use super::node::Root;

impl Root {
    /// `ClassInfo.getAliasShortName`
    pub fn class_short_name(&self, ci: &ClassInfo) -> String {
        match self.class_index(&ci.raw_name()) {
            Some(i) => self.classes[i]
                .alias
                .clone()
                .unwrap_or_else(|| ci.short_name().to_string()),
            None => ci.short_name().to_string(),
        }
    }

    /// `ClassInfo.getAliasPkg`
    pub fn class_package(&self, ci: &ClassInfo) -> String {
        if let Some(p) = ci.parent_class() {
            return self.class_package(p);
        }
        if self.class_index(&ci.raw_name()).is_none() {
            return ci.package().to_string();
        }
        self.package_name(ci.package())
    }

    /// `ClassInfo.getAliasFullName`
    pub fn class_full_name(&self, ci: &ClassInfo) -> String {
        let short = self.class_short_name(ci);
        if let Some(p) = ci.parent_class() {
            return format!("{}.{short}", self.class_full_name(p));
        }
        let pkg = self.class_package(ci);
        if pkg.is_empty() {
            short
        } else {
            format!("{pkg}.{short}")
        }
    }

    /// `ClassInfo.getAliasNameWithoutPackage`
    pub fn class_name_without_package(&self, ci: &ClassInfo) -> String {
        let short = self.class_short_name(ci);
        match ci.parent_class() {
            None => short,
            Some(p) => format!("{}.{short}", self.class_name_without_package(p)),
        }
    }

    /// Full name of a package after renaming (`PackageNode.getAliasPkgInfo().getFullName()`).
    pub fn package_name(&self, pkg: &str) -> String {
        if pkg.is_empty() {
            return self.pkg_aliases.get("").cloned().unwrap_or_default();
        }
        let (parent, leaf) = match pkg.rfind('.') {
            Some(dot) => (Some(&pkg[..dot]), &pkg[dot + 1..]),
            None => (None, pkg),
        };
        let leaf = self.pkg_aliases.get(pkg).map_or(leaf, String::as_str);
        match parent {
            Some(p) => format!("{}.{leaf}", self.package_name(p)),
            None => leaf.to_string(),
        }
    }

    /// `FieldInfo.getAlias` (or the alias of the field node it resolves to).
    pub fn field_name(&self, field: &FieldInfo) -> String {
        match self.resolve_field(field) {
            Some(f) => f.alias.clone().unwrap_or_else(|| f.info.name.clone()),
            None => field.name.clone(),
        }
    }

    /// `MethodInfo.getAlias` (or the alias of the method node it resolves to).
    pub fn method_name(&self, mth: &MethodInfo) -> String {
        match self.resolve_method(mth) {
            Some((c, m)) => {
                let node = &self.classes[c].methods[m];
                node.alias.clone().unwrap_or_else(|| node.info.name.clone())
            }
            None => mth.name.clone(),
        }
    }
}
