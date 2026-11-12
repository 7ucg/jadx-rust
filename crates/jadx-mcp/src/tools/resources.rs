//! Manifest, resource and string tools.

use serde_json::{json, Value};

use crate::args::{opt_bool, opt_str, opt_usize, page, page_iter, page_slice, req_str, text_page};
use crate::error::{ToolError, ToolResult};
use crate::manifest::{self, Filter};
use crate::registry::{Param, ToolSpec};
use crate::server::ToolCtx;

use super::TextMatch;

const MAX_VALUE_CHARS: usize = 4096;
const MAX_LINE_CHARS: usize = 300;

pub fn specs() -> Vec<ToolSpec> {
    let query_aliases: &[&str] = &["keyword", "text", "value"];
    vec![
        ToolSpec::new(
            "get_android_manifest",
            "Decoded AndroidManifest.xml of an APK or XAPK, paged by characters.",
            get_android_manifest,
        )
        .text_paged(),
        ToolSpec::new(
            "get_manifest_summary",
            "Structured manifest summary: package, versions, SDK levels, permissions, application flags and components with exported state, intent filters and launcher marker.",
            get_manifest_summary,
        )
        .param(Param::choice(
            "component_type",
            &["any", "activity", "activity-alias", "service", "receiver", "provider"],
            "any",
            "",
        ))
        .param(Param::choice(
            "exported",
            &["any", "true", "false"],
            "any",
            "Effective exported state: the declared value, or true when undeclared and an intent filter exists",
        )),
        ToolSpec::new(
            "get_all_resource_file_names",
            "List the files of the input archive plus generated resource files (values, public.xml). text=true marks files whose decoded text get_resource_file can return.",
            get_all_resource_file_names,
        )
        .param(Param::string("query", "Case-insensitive substring of the path"))
        .alias("query", query_aliases)
        .paged(),
        ToolSpec::new(
            "get_resource_file",
            "Decoded text of a resource file (binary XML such as layouts, resource tables as values XML), paged by characters.",
            get_resource_file,
        )
        .require(Param::string("path", "Path as listed by get_all_resource_file_names"))
        .alias("path", &["file", "file_path"])
        .text_paged(),
        ToolSpec::new(
            "search_resources",
            "Search the decoded text of all resource files (manifest, layouts, values) for a literal string; hits carry path, line number and line.",
            search_resources,
        )
        .require(Param::string("query", "Literal text to look for"))
        .alias("query", query_aliases)
        .param(Param::boolean("case_sensitive", false, ""))
        .param(Param::string("path_prefix", "Only search files whose path starts with this, e.g. res/values"))
        .paged(),
        ToolSpec::new(
            "get_strings",
            "Strings of the DEX string tables, optionally only of the DEX file defining a class. Includes every string of the table, whether or not code references it.",
            get_strings,
        )
        .param(Param::string("query", "Substring filter"))
        .alias("query", query_aliases)
        .param(Param::choice("match_mode", &["contains", "exact"], "contains", ""))
        .param(Param::boolean("case_sensitive", true, ""))
        .param(Param::string("class_id", "Restrict to the DEX file that defines this class"))
        .alias("class_id", &["class_name"])
        .paged(),
        ToolSpec::new(
            "find_string_references",
            "Find code that loads a string literal (const-string instructions): class, method and code offset per hit. Strings built at run time and strings in annotations or static initial values are not covered.",
            find_string_references,
        )
        .require(Param::string("query", "String value or substring"))
        .alias("query", &["value", "string", "literal", "text"])
        .param(Param::choice("match_mode", &["contains", "exact"], "contains", ""))
        .param(Param::boolean("case_sensitive", true, ""))
        .paged(),
    ]
}

fn get_android_manifest(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let text = handle
        .project
        .manifest()?
        .ok_or_else(|| ToolError::new("NOT_FOUND", "The input has no AndroidManifest.xml"))?;
    text_page(&text, args, "xml")
}

fn get_manifest_summary(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let text = handle
        .project
        .manifest()?
        .ok_or_else(|| ToolError::new("NOT_FOUND", "The input has no AndroidManifest.xml"))?;
    let filter = Filter {
        component_type: opt_str(args, "component_type")
            .filter(|t| *t != "any")
            .map(str::to_string),
        exported: match opt_str(args, "exported") {
            Some("true") => Some(true),
            Some("false") => Some(false),
            _ => None,
        },
    };
    manifest::summary(&text, &filter)
        .ok_or_else(|| ToolError::failed("The manifest could not be parsed"))
}

fn get_all_resource_file_names(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let files = handle.project.resource_files()?;
    let query = opt_str(args, "query").map(|q| TextMatch::new(q, false, false));
    let items = files
        .iter()
        .filter(|f| query.as_ref().is_none_or(|q| q.matches(&f.path)))
        .map(|f| json!({"path": f.path, "size": f.size, "text": f.text}))
        .collect();
    page(items, args)
}

fn get_resource_file(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let path = req_str(args, "path")?;
    match handle.project.resource_text(path)? {
        Some(text) => {
            let mut out = text_page(&text, args, "text")?;
            out["path"] = json!(path);
            Ok(out)
        }
        None => Err(ToolError::new(
            "NOT_FOUND",
            format!("No decoded text for {path}; binary files are not exposed"),
        )),
    }
}

fn search_resources(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let matcher = TextMatch::new(
        req_str(args, "query")?,
        false,
        opt_bool(args, "case_sensitive", false),
    );
    let prefix = opt_str(args, "path_prefix").unwrap_or("");
    let files: Vec<String> = handle
        .project
        .resource_files()?
        .into_iter()
        .filter(|f| f.text && f.path.starts_with(prefix))
        .map(|f| f.path)
        .collect();
    let project = handle.project.clone();
    let items = files
        .into_iter()
        .take_while(|_| !ctx.is_cancelled())
        .filter_map(move |path| {
            project
                .resource_text(&path)
                .ok()
                .flatten()
                .map(|text| (path, text))
        })
        .flat_map(|(path, text)| {
            let found: Vec<(usize, String)> = text
                .lines()
                .enumerate()
                .filter(|(_, l)| matcher.matches(l))
                .map(|(n, l)| (n + 1, clip(l.trim())))
                .collect();
            found
                .into_iter()
                .map(move |(line, text)| json!({"path": path, "line": line, "text": text}))
                .collect::<Vec<_>>()
        });
    let out = page_iter(items, args)?;
    if ctx.is_cancelled() {
        return Err(ToolError::cancelled());
    }
    Ok(out)
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_LINE_CHARS {
        text.to_string()
    } else {
        let cut: String = text.chars().take(MAX_LINE_CHARS).collect();
        format!("{cut}...")
    }
}

fn get_strings(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let class_id = match opt_str(args, "class_id") {
        Some(c) => Some(handle.class(handle.resolve_class(c)?).id.clone()),
        None => None,
    };
    let strings = handle.project.strings(class_id.as_deref())?;
    let query = opt_str(args, "query").map(|q| {
        TextMatch::new(
            q,
            opt_str(args, "match_mode") == Some("exact"),
            opt_bool(args, "case_sensitive", true),
        )
    });
    let items: Vec<&String> = strings
        .iter()
        .filter(|s| query.as_ref().is_none_or(|q| q.matches(s)))
        .collect();
    page_slice(&items, args, |s| json!({"value": truncate_value(s)}))
}

fn truncate_value(value: &str) -> Value {
    if value.chars().count() <= MAX_VALUE_CHARS {
        json!(value)
    } else {
        json!(value.chars().take(MAX_VALUE_CHARS).collect::<String>())
    }
}

fn find_string_references(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let matcher = TextMatch::new(
        req_str(args, "query")?,
        opt_str(args, "match_mode") == Some("exact"),
        opt_bool(args, "case_sensitive", true),
    );
    let offset = opt_usize(args, "offset").unwrap_or(0);
    let limit = opt_usize(args, "limit").unwrap_or(100);
    // one hit more than the page ends at tells whether another page exists
    let wanted = offset.saturating_add(limit).saturating_add(1);
    let cancelled = || ctx.is_cancelled();
    let uses = handle
        .project
        .string_uses(&|s| matcher.matches(s), wanted, &cancelled)?;
    let items = uses.into_iter().map(|u| {
        json!({
            "class_id": u.class_id,
            "method_id": u.method_id,
            "offset": u.offset,
            "value": truncate_value(&u.value),
        })
    });
    page_iter(items, args)
}
