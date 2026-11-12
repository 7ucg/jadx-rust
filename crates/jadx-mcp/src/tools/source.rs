//! Source, disassembly and source search tools.

use serde_json::{json, Value};

use crate::args::{opt_bool, opt_str, page_iter, req_str, text_page};
use crate::error::{ToolError, ToolResult};
use crate::index::ProjectHandle;
use crate::registry::{Param, ToolSpec};
use crate::server::ToolCtx;

use super::{resolve_method, PackageFilter, TextMatch};

const FORMATS: &[&str] = &["auto", "java", "smali"];
const MAX_LINE_CHARS: usize = 300;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::new(
            "get_class_source",
            "Java source of a class, paged by characters. source_kind tells whether the text is decompiled code or a declaration skeleton with placeholder bodies. format=auto falls back to smali when Java is unavailable.",
            get_class_source,
        )
        .class_selector()
        .param(Param::choice(
            "format",
            FORMATS,
            "auto",
            "auto: Java with smali fallback; java: Java only; smali: disassembly",
        ))
        .text_paged(),
        ToolSpec::new(
            "get_method_source",
            "Source of one method: Java (see source_kind) or its smali block. Select by method_id, or by class_id with method_name and, for overloads, descriptor.",
            get_method_source,
        )
        .param(Param::string(
            "method_id",
            "Full id from get_methods_of_class: a.b.C.name(args)ret, or smali form La/b/C;->name(args)ret",
        ))
        .alias("method_id", &["method_signature", "signature"])
        .param(Param::string("class_id", "Class of the method"))
        .alias("class_id", &["class_name"])
        .param(Param::string("method_name", "Method name (with class_id)"))
        .param(Param::string("descriptor", "Descriptor to pick one overload, e.g. (I)V"))
        .any_of(&[&["method_id"], &["class_id", "method_name"]])
        .param(Param::choice("format", FORMATS, "auto", "auto and java: Java; smali: the method block of the disassembly"))
        .text_paged(),
        ToolSpec::new(
            "get_class_disassembly",
            "Smali disassembly of a class (DEX input only), paged by characters.",
            get_class_disassembly,
        )
        .class_selector()
        .text_paged(),
        ToolSpec::new(
            "search_code",
            "Search the class source text for a literal string. Each hit has class_id, line number and the line. Pages skip hits; the scan restarts for every page.",
            search_code,
        )
        .require(Param::string("query", "Literal text to look for"))
        .alias("query", &["keyword", "text", "value"])
        .param(Param::boolean("case_sensitive", true, ""))
        .param(Param::str_or_list(
            "package_prefix",
            "Only search classes under these package or class prefixes",
        ))
        .paged(),
    ]
}

fn with_class(mut page: Value, class_id: &str, requested: &str, kind: &str) -> Value {
    page["class_id"] = json!(class_id);
    page["requested_class_id"] = json!(requested);
    page["source_kind"] = json!(kind);
    page
}

fn get_class_source(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let requested = req_str(args, "class_id")?;
    let index = handle.resolve_class(requested)?;
    let id = handle.class(index).id.clone();
    let format = opt_str(args, "format").unwrap_or("auto");
    if format != "smali" {
        match handle.class_source(index) {
            Ok(source) => {
                let page = text_page(&source.text, args, "java")?;
                return Ok(with_class(page, &id, requested, source.kind.name()));
            }
            Err(e) if format == "java" => return Err(e),
            Err(e) => {
                let smali = handle.project.class_smali(&id)?;
                let mut page = text_page(&smali, args, "smali")?;
                page["fallback_reason"] = json!(e.to_string());
                return Ok(with_class(page, &id, requested, "smali"));
            }
        }
    }
    let smali = handle.project.class_smali(&id)?;
    let page = text_page(&smali, args, "smali")?;
    Ok(with_class(page, &id, requested, "smali"))
}

fn get_class_disassembly(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let requested = req_str(args, "class_id")?;
    let index = handle.resolve_class(requested)?;
    let id = handle.class(index).id.clone();
    let smali = handle.project.class_smali(&id)?;
    let page = text_page(&smali, args, "smali")?;
    Ok(with_class(page, &id, requested, "smali"))
}

/// The `.method` block of `name` + `descriptor` in the smali text of a class.
pub(crate) fn smali_method_block(smali: &str, name: &str, descriptor: &str) -> Option<String> {
    let suffix = format!(" {name}{descriptor}");
    let mut out = String::new();
    let mut inside = false;
    for line in smali.lines() {
        if !inside {
            if line.starts_with(".method ") && line.ends_with(&suffix) {
                inside = true;
            } else {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
        if line == ".end method" {
            return Some(out);
        }
    }
    None
}

fn get_method_source(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let method = resolve_method(&handle, args)?;
    let format = opt_str(args, "format").unwrap_or("auto");
    let (text, kind, label) = if format == "smali" {
        let smali = handle.project.class_smali(&method.class_id)?;
        let block = smali_method_block(&smali, &method.name, &method.descriptor)
            .ok_or_else(|| ToolError::failed(format!("No smali block found for {}", method.id)))?;
        (block, "smali", "smali")
    } else {
        let source = handle.project.method_source(&method.class_id, &method.id)?;
        (source.text, source.kind.name(), "java")
    };
    let mut page = text_page(&text, args, label)?;
    page["method_id"] = json!(method.id);
    page["class_id"] = json!(method.class_id);
    page["source_kind"] = json!(kind);
    Ok(page)
}

fn search_code(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let query = req_str(args, "query")?;
    let matcher = TextMatch::new(query, false, opt_bool(args, "case_sensitive", true));
    let filter = PackageFilter::from_args(args);
    let hits = hits(&handle, ctx, &matcher, &filter);
    let mut out = page_iter(hits, args)?;
    if ctx.is_cancelled() {
        return Err(ToolError::cancelled());
    }
    out["source_kind"] = json!(handle.project.info().source_kind.name());
    Ok(out)
}

fn hits<'a>(
    handle: &'a ProjectHandle,
    ctx: &'a ToolCtx<'a>,
    matcher: &'a TextMatch,
    filter: &'a PackageFilter,
) -> impl Iterator<Item = Value> + 'a {
    (0..handle.project.classes().len())
        .take_while(move |_| !ctx.is_cancelled())
        .filter(move |i| filter.matches(handle.class(*i)))
        .filter_map(move |i| handle.class_source(i).ok().map(|s| (i, s)))
        .flat_map(move |(i, source)| {
            let id = handle.class(i).id.clone();
            let lines: Vec<(usize, String)> = source
                .text
                .lines()
                .enumerate()
                .filter(|(_, line)| matcher.matches(line))
                .map(|(n, line)| (n + 1, clip(line.trim())))
                .collect();
            lines
                .into_iter()
                .map(move |(line, text)| json!({"class_id": id, "line": line, "text": text}))
        })
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        line.to_string()
    } else {
        let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
        format!("{cut}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALI: &str = ".class public La;\n.method public a()V\n    return-void\n.end method\n\n.method public a(I)I\n    const/4 v0, 0x0\n    return v0\n.end method\n";

    #[test]
    fn extracts_the_block_of_one_overload() {
        let block = smali_method_block(SMALI, "a", "(I)I").unwrap();
        assert!(block.starts_with(".method public a(I)I"));
        assert!(block.ends_with(".end method\n"));
        assert!(!block.contains("return-void"));
        assert!(smali_method_block(SMALI, "a", "()V")
            .unwrap()
            .contains("return-void"));
        assert!(smali_method_block(SMALI, "b", "()V").is_none());
    }
}
