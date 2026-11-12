//! `RenameVisitor` with the default alias provider: invalid, non printable and colliding names of
//! classes, fields, methods and packages get a replacement name that is used in the output.

use std::collections::{HashMap, HashSet};

use crate::model::node::{ClassNode, Root};
use crate::name_mapper::{
    is_all_chars_printable, is_valid_identifier, remove_invalid_chars, remove_invalid_chars_middle,
    remove_non_printable_characters,
};

const ACC_INTERFACE: u32 = 0x200;
const ACC_ABSTRACT: u32 = 0x400;
const ACC_CONSTRUCTOR: u32 = 0x10000;
const DEFAULT_PACKAGE_NAME: &str = "defpackage";
const ANONYMOUS_CLASS_PREFIX: &str = "AnonymousClass";

/// Options of the pass (the reference defaults).
#[derive(Debug, Clone, Copy, Default)]
pub struct RenameOptions {
    /// File system is case sensitive: skip the checks for names that differ by case only.
    pub fs_case_sensitive: bool,
}

/// `DeobfAliasProvider` (counters of generated names).
#[derive(Default)]
struct AliasProvider {
    pkg_index: usize,
    cls_index: usize,
    fld_index: usize,
    mth_index: usize,
}

/// Package tree node with its current output name (`PackageNode`).
struct Pkg {
    full: String,
    name: String,
    parent: Option<usize>,
    /// Full name of the output package (`aliasPkgInfo.getFullName`).
    alias_full: String,
    /// Last segment of the output package (`aliasPkgInfo.getName`).
    alias_name: String,
}

pub fn rename_all(root: &mut Root, opts: &RenameOptions) {
    let mut provider = AliasProvider::default();
    let classes = root.sorted_class_order();
    for &ci in &classes {
        check_class_name(root, &mut provider, ci);
        check_fields(root, &mut provider, ci);
        check_methods(root, &mut provider, ci);
    }
    let mut pkgs = collect_packages(root);
    for i in 0..pkgs.len() {
        check_package(&mut pkgs, i, &mut provider);
    }
    if !opts.fs_case_sensitive {
        // check for package directory conflicts on case insensitive filesystems
        let mut paths = HashSet::new();
        for i in 0..pkgs.len() {
            if !paths.insert(pkgs[i].alias_full.to_lowercase()) {
                let name = for_package(&mut provider, &pkgs[i].name);
                set_leaf_alias(&mut pkgs, i, &name);
                // verify the new name also doesn't conflict
                if !paths.insert(pkgs[i].alias_full.to_lowercase()) {
                    let name = for_package(&mut provider, &pkgs[i].name);
                    set_leaf_alias(&mut pkgs, i, &name);
                }
            }
        }
    }
    update_packages(&mut pkgs);
    root.pkg_aliases = pkgs
        .iter()
        .filter(|p| p.alias_name != p.name)
        .map(|p| (p.full.clone(), p.alias_name.clone()))
        .collect();
    if !opts.fs_case_sensitive {
        // check for class file conflicts on case insensitive filesystems (run after package rename)
        let mut paths = HashSet::new();
        for &ci in &classes {
            if !paths.insert(alias_full_path(root, ci).to_lowercase()) {
                let name = for_class(root, &mut provider, ci);
                set_class_short_name(root, ci, &name);
                append_reason(
                    &mut root.classes[ci].alias_reason,
                    "case insensitive filesystem",
                );
                // verify the new name also doesn't conflict
                if !paths.insert(alias_full_path(root, ci).to_lowercase()) {
                    let name = for_class(root, &mut provider, ci);
                    set_class_short_name(root, ci, &name);
                }
            }
        }
    }
    process_root_packages(root, &mut provider, &classes);
}

fn alias_full_path(root: &Root, ci: usize) -> String {
    let info = &root.classes[ci].info;
    let file_name = root.class_name_without_package(info).replace('.', "_");
    let pkg = root.class_package(info);
    if pkg.is_empty() {
        file_name
    } else {
        format!("{}/{file_name}", pkg.replace('.', "/"))
    }
}

// ---- classes ---------------------------------------------------------------------------------

fn class_short_name(root: &Root, ci: usize) -> String {
    let c = &root.classes[ci];
    c.alias
        .clone()
        .unwrap_or_else(|| c.info.short_name().to_string())
}

/// `ClassInfo.changeShortName`
fn set_class_short_name(root: &mut Root, ci: usize, name: &str) {
    let c = &mut root.classes[ci];
    c.alias = if name.is_empty() || name == c.info.short_name() {
        None
    } else {
        Some(name.to_string())
    };
}

/// `RenameReasonAttr.append`
fn append_reason(reason: &mut Option<String>, text: &str) {
    match reason {
        Some(r) if !r.is_empty() => {
            r.push_str(" and ");
            r.push_str(text);
        }
        _ => *reason = Some(text.to_string()),
    }
}

const REASON_NOT_VALID: &str = "not valid java name";
const REASON_NOT_PRINTABLE: &str = "contains not printable characters";

fn check_class_name(root: &mut Root, provider: &mut AliasProvider, ci: usize) {
    if root.classes[ci].package_info {
        return;
    }
    let cls_name = class_short_name(root, ci);
    let Some(new_short_name) = fix_cls_short_name(&cls_name) else {
        // rename failed, use deobfuscator
        let name = for_class(root, provider, ci);
        set_class_short_name(root, ci, &name);
        append_reason(&mut root.classes[ci].alias_reason, REASON_NOT_PRINTABLE);
        return;
    };
    if new_short_name != cls_name {
        set_class_short_name(root, ci, &new_short_name);
        append_reason(&mut root.classes[ci].alias_reason, "invalid class name");
    }
    if root.classes[ci].info.is_inner() {
        // check inner classes names
        let mut parent = root.classes[ci].info.parent_class().cloned();
        while let Some(p) = parent {
            if root.class_short_name(&p) == new_short_name {
                let name = for_class(root, provider, ci);
                set_class_short_name(root, ci, &name);
                append_reason(
                    &mut root.classes[ci].alias_reason,
                    "collision with other inner class name",
                );
                break;
            }
            parent = p.parent_class().cloned();
        }
    }
}

fn fix_cls_short_name(cls_name: &str) -> Option<String> {
    let first = cls_name.chars().next()?;
    if cls_name.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!(
            "{ANONYMOUS_CLASS_PREFIX}{}",
            remove_invalid_chars_middle(cls_name)
        ));
    }
    if first == '$' || first.is_numeric() {
        return Some(format!("C{}", remove_invalid_chars_middle(cls_name)));
    }
    let clean = remove_non_printable_characters(cls_name);
    if clean.is_empty() {
        return None;
    }
    let clean = remove_invalid_chars(cls_name, "C");
    if !is_valid_identifier(&clean) {
        return Some(format!("C{clean}"));
    }
    Some(clean)
}

// ---- fields and methods ------------------------------------------------------------------------

fn check_fields(root: &mut Root, provider: &mut AliasProvider, ci: usize) {
    let mut names = HashSet::new();
    for fi in 0..root.classes[ci].fields.len() {
        let f = &root.classes[ci].fields[fi];
        let name = f.alias.clone().unwrap_or_else(|| f.info.name.clone());
        let not_unique = !names.insert(name.clone());
        let not_valid = !is_valid_identifier(&name);
        let not_printable = !is_all_chars_printable(&name);
        if not_unique || not_valid || not_printable {
            let new = for_field(provider, &f.info.name);
            let f = &mut root.classes[ci].fields[fi];
            f.alias = Some(new);
            if not_valid {
                append_reason(&mut f.alias_reason, REASON_NOT_VALID);
            }
            if not_printable {
                append_reason(&mut f.alias_reason, REASON_NOT_PRINTABLE);
            }
            if not_unique {
                append_reason(&mut f.alias_reason, "collision with other field name");
            }
        }
    }
}

fn mth_name(root: &Root, ci: usize, mi: usize) -> String {
    let m = &root.classes[ci].methods[mi];
    m.alias.clone().unwrap_or_else(|| m.info.name.clone())
}

fn check_methods(root: &mut Root, provider: &mut AliasProvider, ci: usize) {
    let methods: Vec<usize> = (0..root.classes[ci].methods.len())
        .filter(|&mi| root.classes[ci].methods[mi].access & ACC_CONSTRUCTOR == 0)
        .collect();
    for &mi in &methods {
        let name = mth_name(root, ci, mi);
        let not_valid = !is_valid_identifier(&name);
        let not_printable = !is_all_chars_printable(&name);
        if not_valid || not_printable {
            let new = for_method(root, provider, ci, mi);
            rename_method(root, ci, mi, &new);
            let m = &mut root.classes[ci].methods[mi];
            if not_valid {
                append_reason(&mut m.alias_reason, REASON_NOT_VALID);
            }
            if not_printable {
                append_reason(&mut m.alias_reason, REASON_NOT_PRINTABLE);
            }
        }
    }
    // rename methods with same signature
    let mut names = HashSet::new();
    for &mi in &methods {
        let m = &root.classes[ci].methods[mi];
        let signature =
            crate::model::info::make_short_id(&mth_name(root, ci, mi), &m.info.arg_types, None);
        if !names.insert(signature) && can_rename(root, ci, mi) {
            let new = for_method(root, provider, ci, mi);
            rename_method(root, ci, mi, &new);
            root.classes[ci].methods[mi].alias_reason =
                Some("collision with other method in class".to_string());
        }
    }
}

/// `MethodNode.rename`: all methods of the override hierarchy get the new name.
fn rename_method(root: &mut Root, ci: usize, mi: usize, new: &str) {
    match root.classes[ci].methods[mi].override_set {
        Some(set) => {
            for (c, m) in root.override_sets[set].clone() {
                root.classes[c].methods[m].alias = Some(new.to_string());
            }
        }
        None => root.classes[ci].methods[mi].alias = Some(new.to_string()),
    }
}

fn can_rename(root: &Root, ci: usize, mi: usize) -> bool {
    let m = &root.classes[ci].methods[mi];
    if m.dont_rename {
        return false;
    }
    if let Some(set) = m.override_set {
        for &(c, i) in &root.override_sets[set] {
            if (c, i) != (ci, mi) && c == ci {
                // ignore rename if exists related method from same class (bridge method in most
                // cases): such rename will also rename current method and will not help to
                // resolve name collision
                return false;
            }
        }
    }
    true
}

// ---- packages ----------------------------------------------------------------------------------

/// All packages (with their parents) sorted by full name.
fn collect_packages(root: &Root) -> Vec<Pkg> {
    let mut names: HashSet<String> = HashSet::new();
    for c in &root.classes {
        let pkg = c.info.package();
        if pkg.is_empty() {
            names.insert(String::new());
            continue;
        }
        let mut end = 0;
        loop {
            end = pkg[end..].find('.').map_or(pkg.len(), |d| end + d);
            names.insert(pkg[..end].to_string());
            if end == pkg.len() {
                break;
            }
            end += 1;
        }
    }
    let mut list: Vec<String> = names.into_iter().collect();
    list.sort_by_cached_key(|n| n.encode_utf16().collect::<Vec<u16>>());
    let index: HashMap<String, usize> = list
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();
    list.iter()
        .map(|full| {
            let (parent, name) = match full.rfind('.') {
                Some(d) => (index.get(&full[..d]).copied(), full[d + 1..].to_string()),
                None => (None, full.clone()),
            };
            Pkg {
                full: full.clone(),
                name: name.clone(),
                parent,
                alias_full: full.clone(),
                alias_name: name,
            }
        })
        .collect()
}

fn check_package(pkgs: &mut [Pkg], i: usize, provider: &mut AliasProvider) {
    if pkgs[i].alias_full.is_empty() {
        pkgs[i].alias_full = DEFAULT_PACKAGE_NAME.to_string();
        pkgs[i].alias_name = DEFAULT_PACKAGE_NAME.to_string();
        return;
    }
    let name = pkgs[i].alias_name.clone();
    if !is_valid_identifier(&name) || !is_all_chars_printable(&name) {
        let new = for_package(provider, &pkgs[i].name);
        set_leaf_alias(pkgs, i, &new);
    }
}

/// `PackageNode.setLeafAlias` (without updating the sub packages)
fn set_leaf_alias(pkgs: &mut [Pkg], i: usize, alias: &str) {
    if pkgs[i].name == alias {
        pkgs[i].alias_full = pkgs[i].full.clone();
        pkgs[i].alias_name = pkgs[i].name.clone();
        return;
    }
    let full = match pkgs[i].parent {
        Some(p) => format!("{}.{alias}", pkgs[p].alias_full),
        None => alias.to_string(),
    };
    pkgs[i].alias_full = full;
    pkgs[i].alias_name = alias.to_string();
}

/// `RootNode.runPackagesUpdate`: sub packages follow the names of their parents.
fn update_packages(pkgs: &mut [Pkg]) {
    for i in 0..pkgs.len() {
        if let Some(p) = pkgs[i].parent {
            pkgs[i].alias_full = format!("{}.{}", pkgs[p].alias_full, pkgs[i].alias_name);
        }
    }
}

fn process_root_packages(root: &mut Root, provider: &mut AliasProvider, classes: &[usize]) {
    let mut root_pkgs: HashSet<String> = HashSet::new();
    for c in &root.classes {
        let pkg = c.info.package();
        root_pkgs.insert(pkg.split('.').next().unwrap_or("").to_string());
    }
    // rename field if collide with any root package
    for &ci in classes {
        for fi in 0..root.classes[ci].fields.len() {
            let f = &root.classes[ci].fields[fi];
            let name = f.alias.as_deref().unwrap_or(&f.info.name);
            if root_pkgs.contains(name) {
                let new = for_field(provider, &f.info.name);
                let f = &mut root.classes[ci].fields[fi];
                f.alias = Some(new);
                f.alias_reason = Some("collision with root package name".to_string());
            }
        }
    }
}

// ---- alias provider ---------------------------------------------------------------------------

fn prepare_name_part(name: &str) -> String {
    remove_invalid_chars_middle(name)
}

fn for_package(provider: &mut AliasProvider, name: &str) -> String {
    let s = format!("p{:03}{}", provider.pkg_index, prepare_name_part(name));
    provider.pkg_index += 1;
    s
}

fn for_class(root: &Root, provider: &mut AliasProvider, ci: usize) -> String {
    let cls = &root.classes[ci];
    let prefix = make_cls_prefix(root, cls);
    let s = format!(
        "{prefix}C{:04}{}",
        provider.cls_index,
        prepare_name_part(cls.info.short_name())
    );
    provider.cls_index += 1;
    s
}

fn for_field(provider: &mut AliasProvider, name: &str) -> String {
    let s = format!("f{}{}", provider.fld_index, prepare_name_part(name));
    provider.fld_index += 1;
    s
}

fn for_method(root: &Root, provider: &mut AliasProvider, ci: usize, mi: usize) -> String {
    let m = &root.classes[ci].methods[mi];
    let prefix = if m.override_set.is_some() { "mo" } else { "m" };
    let s = format!(
        "{prefix}{}{}",
        provider.mth_index,
        prepare_name_part(&m.info.name)
    );
    provider.mth_index += 1;
    s
}

/// Prefix for a class name that bases on class properties, extended super classes or implemented
/// interfaces.
fn make_cls_prefix(root: &Root, cls: &ClassNode) -> String {
    if cls.is_enum() {
        return "Enum".to_string();
    }
    let mut result = String::new();
    if cls.access & ACC_INTERFACE != 0 {
        result.push_str("Interface");
    } else if cls.access & ACC_ABSTRACT != 0 {
        result.push_str("Abstract");
    }
    result.push_str(&get_base_name(root, cls));
    result
}

/// Process current class and all super classes to get meaningful parent name.
fn get_base_name(root: &Root, cls: &ClassNode) -> String {
    let mut current = cls;
    for _ in 0..root.classes.len() + 1 {
        let super_cls = current.super_class.as_ref();
        if let Some(name) = super_cls.and_then(|s| s.object_name()) {
            // e.g. Activity or Fragment, AsyncTask
            if name.starts_with("android.app.") || name.starts_with("android.os.") {
                return get_cls_name(&name);
            }
        }
        for iface in &cls.interfaces {
            let name = iface.object_name().unwrap_or_default();
            if name == "java.lang.Runnable" {
                return "Runnable".to_string();
            }
            if name.starts_with("java.util.concurrent.") // e.g. Callable
                || name.starts_with("android.view.") // e.g. View.OnClickListener
                || name.starts_with("android.content.")
            // e.g. DialogInterface.OnClickListener
            {
                return get_cls_name(&name);
            }
        }
        let Some(s) = super_cls else {
            break;
        };
        match root.resolve_class(s) {
            Some(next) => current = &root.classes[next],
            None => break,
        }
    }
    String::new()
}

fn get_cls_name(name: &str) -> String {
    let cls = name.rsplit('.').next().unwrap_or(name);
    cls.chars().filter(|c| *c != '$').collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_names_are_fixed() {
        assert_eq!(fix_cls_short_name("1").as_deref(), Some("AnonymousClass1"));
        assert_eq!(fix_cls_short_name("1a").as_deref(), Some("C1a"));
        assert_eq!(fix_cls_short_name("$a").as_deref(), Some("C$a"));
        assert_eq!(fix_cls_short_name("Abc").as_deref(), Some("Abc"));
        assert_eq!(fix_cls_short_name("").as_deref(), None);
        assert_eq!(fix_cls_short_name("a-b").as_deref(), Some("ab"));
    }

    #[test]
    fn provider_names() {
        let mut p = AliasProvider::default();
        assert_eq!(for_package(&mut p, "auth-api"), "p000authapi");
        assert_eq!(for_field(&mut p, "a-b"), "f0ab");
        assert_eq!(for_field(&mut p, "x"), "f1x");
    }
}
