//! Class, package, member and hierarchy tools.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::args::{opt_bool, opt_str, opt_usize, page, page_iter, page_slice, req_str};
use crate::error::{ToolError, ToolResult};
use crate::index::ProjectHandle;
use crate::project::ClassEntry;
use crate::registry::{Param, ToolSpec};
use crate::server::ToolCtx;
use crate::skeleton::{class_keyword, flag_names, FlagKind};

use super::{class_json, field_json, method_json, PackageFilter, TextMatch};

pub fn specs() -> Vec<ToolSpec> {
    let package_prefix = || {
        Param::str_or_list(
            "package_prefix",
            "Package or class prefix matched at '.' boundaries, a list of such prefixes (OR), or \"obfuscated-root\" (default package and packages whose segments are all at most 3 characters)",
        )
    };
    let query_aliases: &[&str] = &["keyword", "text", "value"];
    vec![
        ToolSpec::new(
            "get_all_classes",
            "List the classes of the project (dotted raw names; inner classes use '$'). Use the returned class_id for all class tools.",
            get_all_classes,
        )
        .param(package_prefix())
        .param(Param::string("query", "Case-insensitive substring of the class name"))
        .alias("query", query_aliases)
        .paged(),
        ToolSpec::new(
            "search_classes_by_keyword",
            "Search class names for a literal substring (case-insensitive unless case_sensitive is set). Matches the full name, so package fragments work as well.",
            search_classes,
        )
        .require(Param::string("query", "Substring to look for"))
        .alias("query", query_aliases)
        .param(package_prefix())
        .param(Param::boolean("case_sensitive", false, ""))
        .param(Param::choice(
            "kind",
            &["any", "class", "interface", "enum", "annotation"],
            "any",
            "Restrict to one kind of type",
        ))
        .paged(),
        ToolSpec::new(
            "list_packages",
            "List packages with their class counts. depth collapses names to their first N segments, package_prefix restricts the listing.",
            list_packages,
        )
        .param(package_prefix())
        .param(Param::int("depth", 1, 64, "Collapse package names to this many segments"))
        .paged(),
        ToolSpec::new(
            "get_class_info",
            "Metadata of one class: flags, super class, interfaces, member counts, inner classes and direct subclasses.",
            get_class_info,
        )
        .class_selector(),
        ToolSpec::new(
            "get_methods_of_class",
            "List the methods of a class with ids, descriptors and flags.",
            get_methods_of_class,
        )
        .class_selector()
        .paged(),
        ToolSpec::new(
            "get_fields_of_class",
            "List the fields of a class with ids, types, flags and constant values.",
            get_fields_of_class,
        )
        .class_selector()
        .paged(),
        ToolSpec::new(
            "find_methods",
            "Find methods by name across the project (or inside one class). Returns full method ids to use with get_method_source and find_usages.",
            find_methods,
        )
        .require(Param::string("name", "Method name or substring (constructors are <init>)"))
        .alias("name", &["query", "method_name"])
        .param(Param::choice("match_mode", &["contains", "exact"], "contains", ""))
        .param(Param::boolean("case_sensitive", true, ""))
        .param(Param::string("class_id", "Only search inside this class"))
        .alias("class_id", &["class_name"])
        .param(package_prefix())
        .paged(),
        ToolSpec::new(
            "find_fields",
            "Find fields by name across the project (or inside one class), optionally by type descriptor.",
            find_fields,
        )
        .require(Param::string("name", "Field name or substring"))
        .alias("name", &["query", "field_name"])
        .param(Param::choice("match_mode", &["contains", "exact"], "contains", ""))
        .param(Param::boolean("case_sensitive", true, ""))
        .param(Param::string("descriptor", "Only fields of this type descriptor, e.g. Ljava/lang/String;"))
        .param(Param::string("class_id", "Only search inside this class"))
        .alias("class_id", &["class_name"])
        .param(package_prefix())
        .paged(),
        ToolSpec::new(
            "find_direct_subclasses",
            "Classes that extend (or, for an interface, directly implement or extend) the given type. The type may be a framework class named by a project class.",
            find_direct_subclasses,
        )
        .class_selector()
        .paged(),
        ToolSpec::new(
            "find_implementations",
            "All classes below a type in the hierarchy: direct and transitive subclasses and implementers.",
            find_implementations,
        )
        .class_selector()
        .paged(),
        ToolSpec::new(
            "get_class_hierarchy",
            "Super class chain with interfaces (up) and the subclass tree (down) of a class.",
            get_class_hierarchy,
        )
        .class_selector()
        .param(Param::int("depth", 0, 10, "Levels of subclasses to include, default 3").with_default(json!(3)))
        .param(Param::int("max_nodes", 1, 2000, "Cap on the number of subclass nodes, default 200").with_default(json!(200))),
    ]
}

fn kind_matches(access: u32, kind: &str) -> bool {
    kind == "any" || class_keyword(access).trim_start_matches('@') == kind
}

fn get_all_classes(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let filter = PackageFilter::from_args(args);
    let query = opt_str(args, "query").map(|q| TextMatch::new(q, false, false));
    let items: Vec<&ClassEntry> = handle
        .project
        .classes()
        .iter()
        .filter(|c| filter.matches(c))
        .filter(|c| query.as_ref().is_none_or(|q| q.matches(&c.id)))
        .collect();
    page_slice(&items, args, |c| class_json(c))
}

fn search_classes(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let filter = PackageFilter::from_args(args);
    let query = TextMatch::new(
        req_str(args, "query")?,
        false,
        opt_bool(args, "case_sensitive", false),
    );
    let kind = opt_str(args, "kind").unwrap_or("any");
    let items: Vec<&ClassEntry> = handle
        .project
        .classes()
        .iter()
        .filter(|c| filter.matches(c) && kind_matches(c.access, kind))
        .filter(|c| query.matches(&c.id) || query.matches(&c.full_name))
        .collect();
    page_slice(&items, args, |c| class_json(c))
}

fn list_packages(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let filter = PackageFilter::from_args(args);
    let depth = opt_usize(args, "depth");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for class in handle
        .project
        .classes()
        .iter()
        .filter(|c| filter.matches(c))
    {
        let package = match depth {
            Some(d) => class
                .package
                .split('.')
                .take(d)
                .collect::<Vec<_>>()
                .join("."),
            None => class.package.clone(),
        };
        *counts.entry(package).or_default() += 1;
    }
    let items = counts
        .into_iter()
        .map(|(package, classes)| {
            json!({"package": if package.is_empty() { "(default)" } else { &package }, "class_count": classes})
        })
        .collect();
    page(items, args)
}

fn resolve(handle: &ProjectHandle, args: &Value) -> Result<usize, ToolError> {
    handle.resolve_class(req_str(args, "class_id")?)
}

fn get_class_info(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let index = resolve(&handle, args)?;
    let class = handle.class(index);
    let prefix = format!("{}$", class.id);
    let inner: Vec<&str> = handle
        .project
        .classes()
        .iter()
        .filter(|c| c.id.starts_with(&prefix))
        .map(|c| c.id.as_str())
        .collect();
    let subclasses: Vec<&str> = handle
        .direct_subtypes(&class.id)
        .iter()
        .map(|i| handle.class(*i).id.as_str())
        .collect();
    let info = handle.project.info();
    let mut out = class_json(class);
    out["descriptor"] = json!(class.descriptor());
    out["full_name"] = json!(class.full_name);
    out["short_name"] = json!(class.short_name);
    out["package"] = json!(class.package);
    out["access"] = json!(class.access);
    out["is_inner"] = json!(class.is_inner);
    out["inner_classes"] = json!(inner);
    out["direct_subtypes"] = json!(subclasses);
    out["smali_available"] = json!(info.has_smali);
    out["source_kind"] = json!(info.source_kind.name());
    Ok(out)
}

fn get_methods_of_class(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let index = resolve(&handle, args)?;
    let methods = handle
        .project
        .methods(&handle.class(index).id)
        .unwrap_or_default();
    let mut out = page(methods.iter().map(method_json).collect(), args)?;
    out["class_id"] = json!(handle.class(index).id);
    Ok(out)
}

fn get_fields_of_class(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let index = resolve(&handle, args)?;
    let fields = handle
        .project
        .fields(&handle.class(index).id)
        .unwrap_or_default();
    let mut out = page(fields.iter().map(field_json).collect(), args)?;
    out["class_id"] = json!(handle.class(index).id);
    Ok(out)
}

/// Class indexes a member search runs over.
fn search_scope(handle: &ProjectHandle, args: &Value) -> Result<Vec<usize>, ToolError> {
    if let Some(class) = opt_str(args, "class_id") {
        return Ok(vec![handle.resolve_class(class)?]);
    }
    let filter = PackageFilter::from_args(args);
    Ok(handle
        .project
        .classes()
        .iter()
        .enumerate()
        .filter(|(_, c)| filter.matches(c))
        .map(|(i, _)| i)
        .collect())
}

fn member_match(args: &Value) -> Result<TextMatch, ToolError> {
    Ok(TextMatch::new(
        req_str(args, "name")?,
        opt_str(args, "match_mode") == Some("exact"),
        opt_bool(args, "case_sensitive", true),
    ))
}

fn find_methods(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let scope = search_scope(&handle, args)?;
    let name = member_match(args)?;
    let items = scope
        .into_iter()
        .take_while(|_| !ctx.is_cancelled())
        .flat_map(|i| {
            handle
                .project
                .methods(&handle.class(i).id)
                .unwrap_or_default()
        })
        .filter(|m| name.matches(&m.name))
        .map(|m| method_json(&m));
    let out = page_iter(items, args)?;
    if ctx.is_cancelled() {
        return Err(ToolError::cancelled());
    }
    Ok(out)
}

fn find_fields(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let scope = search_scope(&handle, args)?;
    let name = member_match(args)?;
    let descriptor = opt_str(args, "descriptor");
    let items = scope
        .into_iter()
        .take_while(|_| !ctx.is_cancelled())
        .flat_map(|i| {
            handle
                .project
                .fields(&handle.class(i).id)
                .unwrap_or_default()
        })
        .filter(|f| name.matches(&f.name) && descriptor.is_none_or(|d| f.descriptor == d))
        .map(|f| field_json(&f));
    let out = page_iter(items, args)?;
    if ctx.is_cancelled() {
        return Err(ToolError::cancelled());
    }
    Ok(out)
}

fn relation(handle: &ProjectHandle, class: usize, target: &str) -> &'static str {
    if handle.class(class).super_class.as_deref() == Some(target) {
        "extends"
    } else {
        "implements"
    }
}

fn subtype_json(handle: &ProjectHandle, index: usize, target: &str, direct: bool) -> Value {
    let mut out = class_json(handle.class(index));
    out["relation"] = json!(relation(handle, index, target));
    out["direct"] = json!(direct);
    out
}

fn anchor(handle: &ProjectHandle, args: &Value) -> Result<(String, bool), ToolError> {
    let requested = req_str(args, "class_id")?;
    let id = handle.resolve_hierarchy_name(requested)?;
    let in_project = handle.index_of(&id).is_some();
    Ok((id, in_project))
}

fn find_direct_subclasses(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let (id, in_project) = anchor(&handle, args)?;
    let items = handle
        .direct_subtypes(&id)
        .iter()
        .map(|i| subtype_json(&handle, *i, &id, true))
        .collect();
    let mut out = page(items, args)?;
    out["class_id"] = json!(id);
    out["in_project"] = json!(in_project);
    Ok(out)
}

fn find_implementations(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let (id, in_project) = anchor(&handle, args)?;
    let direct: std::collections::HashSet<usize> =
        handle.direct_subtypes(&id).iter().copied().collect();
    let items = handle
        .all_subtypes(&id)
        .into_iter()
        .map(|i| {
            let parent = if direct.contains(&i) {
                id.clone()
            } else {
                handle.class(i).super_class.clone().unwrap_or_default()
            };
            let mut v = subtype_json(&handle, i, &parent, direct.contains(&i));
            v["relation"] = json!(if direct.contains(&i) {
                relation(&handle, i, &id)
            } else {
                "inherits"
            });
            v
        })
        .collect();
    let mut out = page(items, args)?;
    out["class_id"] = json!(id);
    out["in_project"] = json!(in_project);
    Ok(out)
}

fn get_class_hierarchy(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let (id, in_project) = anchor(&handle, args)?;
    let depth = opt_usize(args, "depth").unwrap_or(3);
    let max_nodes = opt_usize(args, "max_nodes").unwrap_or(200);

    // ancestors: the super class chain, each with its direct interfaces
    let mut ancestors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut current = handle.index_of(&id);
    let mut next_name = current.and_then(|i| handle.class(i).super_class.clone());
    while let Some(name) = next_name.take() {
        if !seen.insert(name.clone()) {
            break;
        }
        current = handle.index_of(&name);
        let (interfaces, kind) = match current {
            Some(i) => (
                handle.class(i).interfaces.clone(),
                Some(class_keyword(handle.class(i).access)),
            ),
            None => (Vec::new(), None),
        };
        ancestors.push(json!({
            "class_id": name,
            "in_project": current.is_some(),
            "kind": kind,
            "interfaces": interfaces,
        }));
        next_name = current.and_then(|i| handle.class(i).super_class.clone());
    }
    let own_interfaces = handle
        .index_of(&id)
        .map(|i| handle.class(i).interfaces.clone())
        .unwrap_or_default();

    // descendants: breadth first with a node budget
    let mut budget = max_nodes;
    let mut truncated = false;
    let tree = descendants(&handle, &id, depth, &mut budget, &mut truncated);
    Ok(json!({
        "class_id": id,
        "in_project": in_project,
        "kind": handle.index_of(&id).map(|i| class_keyword(handle.class(i).access)),
        "flags": handle.index_of(&id).map(|i| flag_names(handle.class(i).access, FlagKind::Class)),
        "interfaces": own_interfaces,
        "ancestors": ancestors,
        "subclasses": tree,
        "subclasses_truncated": truncated,
    }))
}

fn descendants(
    handle: &ProjectHandle,
    id: &str,
    depth: usize,
    budget: &mut usize,
    truncated: &mut bool,
) -> Vec<Value> {
    if depth == 0 {
        if !handle.direct_subtypes(id).is_empty() {
            *truncated = true;
        }
        return Vec::new();
    }
    let mut out = Vec::new();
    for &i in handle.direct_subtypes(id) {
        if *budget == 0 {
            *truncated = true;
            break;
        }
        *budget -= 1;
        let child = handle.class(i);
        out.push(json!({
            "class_id": child.id,
            "kind": class_keyword(child.access),
            "relation": relation(handle, i, id),
            "subclasses": descendants(handle, &child.id, depth - 1, budget, truncated),
        }));
    }
    out
}
