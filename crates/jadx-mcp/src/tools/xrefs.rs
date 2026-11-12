//! Cross reference tools over the usage information of the project.

use serde_json::{json, Value};

use crate::args::{opt_str, page, req_str};
use crate::error::{ToolError, ToolResult};
use crate::index::ProjectHandle;
use crate::project::Symbol;
use crate::registry::{Param, ToolSpec};
use crate::server::ToolCtx;

use super::{resolve_method, split_member_ref};

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::new(
            "find_usages",
            "Who uses a symbol. For a method: the methods that call it. For a field: the methods that read or write it. For a class: the classes and methods that refer to it. The symbol kind is detected from the id; a short class name resolves when unique.",
            find_usages,
        )
        .require(Param::string(
            "symbol",
            "Class id, method id (a.b.C.name(args)ret or La/b/C;->name(args)ret) or field id (a.b.C.name:descriptor)",
        ))
        .alias(
            "symbol",
            &[
                "id",
                "class_id",
                "class_name",
                "method_id",
                "method_signature",
                "signature",
                "field_id",
                "field",
            ],
        )
        .param(Param::choice(
            "kind",
            &["auto", "class", "method", "field"],
            "auto",
            "Force the symbol kind when the id is ambiguous",
        ))
        .paged(),
        ToolSpec::new(
            "get_method_callees",
            "Methods called by a method: project methods (resolved) and methods outside the project (external).",
            get_method_callees,
        )
        .param(Param::string("method_id", "Full method id"))
        .alias("method_id", &["method_signature", "signature"])
        .param(Param::string("class_id", "Class of the method (with method_name)"))
        .alias("class_id", &["class_name"])
        .param(Param::string("method_name", "Method name (with class_id)"))
        .param(Param::string("descriptor", "Descriptor to pick one overload"))
        .any_of(&[&["method_id"], &["class_id", "method_name"]])
        .paged(),
    ]
}

/// Decide the kind of a symbol id.
fn detect_kind(symbol: &str) -> &'static str {
    if symbol.contains('(') {
        "method"
    } else if symbol.contains(";->") || symbol.contains(':') {
        "field"
    } else {
        "class"
    }
}

fn resolve_symbol(handle: &ProjectHandle, args: &Value) -> Result<Symbol, ToolError> {
    let text = req_str(args, "symbol")?;
    let kind = match opt_str(args, "kind") {
        None | Some("auto") => detect_kind(text),
        Some(k) => match k {
            "class" => "class",
            "method" => "method",
            _ => "field",
        },
    };
    match kind {
        "class" => Ok(Symbol::Class(
            handle.class(handle.resolve_class(text)?).id.clone(),
        )),
        "method" => {
            let method = resolve_method(handle, &json!({"method_id": text}))?;
            Ok(Symbol::Method(method.id))
        }
        _ => {
            let (class_part, rest) = split_member_ref(text)
                .filter(|(_, r)| r.contains(':') && !r.contains('('))
                .ok_or_else(|| ToolError::invalid(format!("Not a field id: {text}")))?;
            let class = handle.class(handle.resolve_class(&class_part)?);
            let id = format!("{}.{rest}", class.id);
            let exists = handle
                .project
                .fields(&class.id)
                .unwrap_or_default()
                .iter()
                .any(|f| f.id == id);
            if !exists {
                let name = rest.split(':').next().unwrap_or("");
                let candidates: Vec<String> = handle
                    .project
                    .fields(&class.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| f.name == name)
                    .map(|f| f.id)
                    .collect();
                return Err(ToolError::new(
                    "SYMBOL_NOT_FOUND",
                    format!("No field {rest} in {}", class.id),
                )
                .with_details(json!({"candidates": candidates})));
            }
            Ok(Symbol::Field(id))
        }
    }
}

fn find_usages(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let symbol = resolve_symbol(&handle, args)?;
    let usages = handle.project.usages(&symbol)?;
    let (kind, id) = match &symbol {
        Symbol::Class(id) => ("class", id),
        Symbol::Method(id) => ("method", id),
        Symbol::Field(id) => ("field", id),
    };
    let mut items: Vec<Value> = Vec::new();
    for class in &usages.classes {
        items.push(json!({"type": "class", "id": class, "class_id": class}));
    }
    for method in &usages.methods {
        let class_id = method
            .find('(')
            .and_then(|p| method[..p].rfind('.').map(|d| &method[..d]))
            .unwrap_or("");
        items.push(json!({"type": "method", "id": method, "class_id": class_id}));
    }
    let mut out = page(items, args)?;
    out["symbol"] = json!(id);
    out["kind"] = json!(kind);
    Ok(out)
}

fn get_method_callees(ctx: &ToolCtx, args: &Value) -> ToolResult {
    let handle = ctx.project()?;
    let method = resolve_method(&handle, args)?;
    let callees = handle.project.callees(&method.id)?;
    let mut items: Vec<Value> = callees
        .resolved
        .iter()
        .map(|m| json!({"method_id": m, "in_project": true}))
        .collect();
    items.extend(
        callees
            .external
            .iter()
            .map(|m| json!({"method_id": m, "in_project": false})),
    );
    let mut out = page(items, args)?;
    out["method_id"] = json!(method.id);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_kinds_are_detected() {
        assert_eq!(detect_kind("a.B.run(I)V"), "method");
        assert_eq!(detect_kind("La/B;->run(I)V"), "method");
        assert_eq!(detect_kind("a.B.x:I"), "field");
        assert_eq!(detect_kind("La/B;->x:I"), "field");
        assert_eq!(detect_kind("a.B$C"), "class");
    }
}
