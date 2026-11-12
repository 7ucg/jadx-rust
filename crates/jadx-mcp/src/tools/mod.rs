//! Tool implementations and the tool table.

mod classes;
mod project;
mod resources;
mod source;
mod xrefs;

use serde_json::{json, Value};

use crate::args::opt_str;
use crate::engine::{split_field_id, split_method_id};
use crate::error::ToolError;
use crate::index::{normalize_class_name, ProjectHandle};
use crate::project::{ClassEntry, FieldEntry, MethodEntry};
use crate::registry::{Registry, ToolSpec};
use crate::skeleton::{class_keyword, flag_names, FlagKind};

/// All tools of the server, in the order of `tools/list`.
pub fn registry() -> Registry {
    let mut tools: Vec<ToolSpec> = Vec::new();
    tools.extend(project::specs());
    tools.extend(classes::specs());
    tools.extend(source::specs());
    tools.extend(resources::specs());
    tools.extend(xrefs::specs());
    Registry::new(tools)
}

/// Compact class record used in lists.
pub(crate) fn class_json(c: &ClassEntry) -> Value {
    json!({
        "class_id": c.id,
        "kind": class_keyword(c.access),
        "flags": flag_names(c.access, FlagKind::Class),
        "super_class": c.super_class,
        "interfaces": c.interfaces,
        "method_count": c.method_count,
        "field_count": c.field_count,
    })
}

pub(crate) fn method_json(m: &MethodEntry) -> Value {
    json!({
        "method_id": m.id,
        "class_id": m.class_id,
        "name": m.name,
        "descriptor": m.descriptor,
        "return_type": m.return_type,
        "arg_types": m.arg_types,
        "flags": flag_names(m.access, FlagKind::Method),
        "has_code": m.has_code,
        "signature": m.signature,
    })
}

pub(crate) fn field_json(f: &FieldEntry) -> Value {
    json!({
        "field_id": f.id,
        "class_id": f.class_id,
        "name": f.name,
        "descriptor": f.descriptor,
        "type": f.ty,
        "flags": flag_names(f.access, FlagKind::Field),
        "constant_value": f.const_value,
        "signature": f.signature,
    })
}

/// Case handling of a text match.
pub(crate) struct TextMatch {
    needle: String,
    exact: bool,
    case_sensitive: bool,
}

impl TextMatch {
    pub fn new(needle: &str, exact: bool, case_sensitive: bool) -> Self {
        TextMatch {
            needle: if case_sensitive {
                needle.to_string()
            } else {
                needle.to_lowercase()
            },
            exact,
            case_sensitive,
        }
    }

    pub fn matches(&self, haystack: &str) -> bool {
        if self.case_sensitive {
            if self.exact {
                haystack == self.needle
            } else {
                haystack.contains(&self.needle)
            }
        } else {
            let lower = haystack.to_lowercase();
            if self.exact {
                lower == self.needle
            } else {
                lower.contains(&self.needle)
            }
        }
    }
}

/// `package_prefix`: one prefix or a list, matched at `.` boundaries; the pseudo prefix
/// `obfuscated-root` selects the default package and packages whose segments are all at most
/// three characters long.
pub(crate) struct PackageFilter {
    prefixes: Vec<String>,
}

impl PackageFilter {
    pub fn from_args(args: &Value) -> Self {
        let prefixes = match args.get("package_prefix") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        PackageFilter {
            prefixes: prefixes
                .into_iter()
                .map(|p| p.trim_end_matches('.').to_string())
                .collect(),
        }
    }

    pub fn matches(&self, class: &ClassEntry) -> bool {
        if self.prefixes.is_empty() {
            return true;
        }
        self.prefixes.iter().any(|p| {
            if p == "obfuscated-root" {
                return class.package.is_empty()
                    || class.package.split('.').all(|seg| seg.chars().count() <= 3);
            }
            let id = class.id.as_str();
            id == p
                || id
                    .strip_prefix(p.as_str())
                    .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('$'))
        })
    }
}

/// Split a member reference into its class part and the rest.
///
/// Accepts `a.b.C.name(args)ret` and `a.b.C.name:descriptor` as well as the smali form
/// `La/b/C;->name(args)ret` and `La/b/C;->name:descriptor`.
pub(crate) fn split_member_ref(text: &str) -> Option<(String, String)> {
    if let Some((class, rest)) = text.split_once(";->") {
        return Some((normalize_class_name(&format!("{class};")), rest.to_string()));
    }
    if text.contains('(') {
        let (class, short) = split_method_id(text)?;
        return Some((class.to_string(), short.to_string()));
    }
    let (class, name, descriptor) = split_field_id(text)?;
    Some((class.to_string(), format!("{name}:{descriptor}")))
}

/// Resolve the method selected by `method_id`, or `class_id` plus `method_name` (and optionally
/// `descriptor`).
pub(crate) fn resolve_method(
    handle: &ProjectHandle,
    args: &Value,
) -> Result<MethodEntry, ToolError> {
    if let Some(id) = opt_str(args, "method_id") {
        let (class_part, short) = split_member_ref(id)
            .filter(|(_, s)| s.contains('('))
            .ok_or_else(|| ToolError::invalid(format!("Not a method id: {id}")))?;
        let index = handle.resolve_class(&class_part)?;
        let class_id = &handle.class(index).id;
        let methods = handle.project.methods(class_id).unwrap_or_default();
        if let Some(found) = methods
            .iter()
            .find(|m| format!("{}{}", m.name, m.descriptor) == short)
        {
            return Ok(found.clone());
        }
        let name = short.split('(').next().unwrap_or("");
        let candidates: Vec<&str> = methods
            .iter()
            .filter(|m| m.name == name)
            .map(|m| m.id.as_str())
            .collect();
        return Err(ToolError::new(
            "SYMBOL_NOT_FOUND",
            format!("No method {short} in {class_id}"),
        )
        .with_details(json!({"candidates": candidates})));
    }
    let class_ref = opt_str(args, "class_id")
        .ok_or_else(|| ToolError::invalid("Provide method_id, or class_id with method_name"))?;
    let name = opt_str(args, "method_name")
        .ok_or_else(|| ToolError::invalid("Provide method_id, or class_id with method_name"))?;
    let index = handle.resolve_class(class_ref)?;
    let class_id = &handle.class(index).id;
    let methods = handle.project.methods(class_id).unwrap_or_default();
    let descriptor = opt_str(args, "descriptor");
    let matching: Vec<&MethodEntry> = methods
        .iter()
        .filter(|m| m.name == name && descriptor.is_none_or(|d| m.descriptor == d))
        .collect();
    match matching.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(ToolError::new(
            "SYMBOL_NOT_FOUND",
            format!("No method {name} in {class_id}"),
        )
        .with_details(
            json!({"methods": methods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()}),
        )),
        many => Err(ToolError::new(
            "AMBIGUOUS_METHOD",
            format!(
                "{} overloads of {name} in {class_id}; pass descriptor or method_id",
                many.len()
            ),
        )
        .with_details(
            json!({"candidates": many.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()}),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_refs_in_both_notations() {
        assert_eq!(
            split_member_ref("a.b.C.run(I)V"),
            Some(("a.b.C".into(), "run(I)V".into()))
        );
        assert_eq!(
            split_member_ref("La/b/C$D;->run(I)V"),
            Some(("a.b.C$D".into(), "run(I)V".into()))
        );
        assert_eq!(
            split_member_ref("a.b.C.x:I"),
            Some(("a.b.C".into(), "x:I".into()))
        );
        assert_eq!(
            split_member_ref("La/b/C;->x:I"),
            Some(("a.b.C".into(), "x:I".into()))
        );
    }

    #[test]
    fn package_filter_matches_on_boundaries() {
        let class = |id: &str| ClassEntry {
            id: id.into(),
            full_name: id.into(),
            short_name: id.rsplit('.').next().unwrap().into(),
            package: id.rsplit_once('.').map(|p| p.0).unwrap_or("").into(),
            access: 1,
            super_class: None,
            interfaces: vec![],
            is_inner: false,
            method_count: 0,
            field_count: 0,
        };
        let f = PackageFilter::from_args(&json!({"package_prefix": "com.app."}));
        assert!(f.matches(&class("com.app.Main")));
        assert!(f.matches(&class("com.app.sub.Main")));
        assert!(!f.matches(&class("com.apple.Main")));
        let f = PackageFilter::from_args(&json!({"package_prefix": ["x", "y.z"]}));
        assert!(f.matches(&class("y.z.A")) && f.matches(&class("x.A")));
        let f = PackageFilter::from_args(&json!({"package_prefix": "obfuscated-root"}));
        assert!(f.matches(&class("a.b.c")) && f.matches(&class("Top")));
        assert!(!f.matches(&class("com.application.Main")));
    }

    #[test]
    fn text_match_modes() {
        assert!(TextMatch::new("Foo", false, false).matches("a.foobar"));
        assert!(!TextMatch::new("Foo", false, true).matches("a.foobar"));
        assert!(TextMatch::new("foo", true, false).matches("FOO"));
        assert!(!TextMatch::new("foo", true, true).matches("foobar"));
    }
}
