//! Argument helpers shared by all tools: alias normalisation, list and text paging, hashing.
//!
//! The paging contract and the alias rules follow the RDX MCP server (Apache-2.0, Copyright 2026
//! Ch0pin and RDX contributors): list results carry `items`, `total` and `next_offset`; text
//! results are sliced by Unicode scalar offset and carry a `source_hash` of the whole document so
//! a changed document is detected between pages. Modified for this crate: argument access
//! helpers, a different hash function and error type.

use serde_json::{json, Value};

use crate::error::ToolError;

pub const DEFAULT_LIST_LIMIT: usize = 100;
pub const MAX_LIST_LIMIT: usize = 500;
pub const DEFAULT_TEXT_LIMIT: usize = 32_000;
pub const MAX_TEXT_LIMIT: usize = 128_000;

const SLICE_NOTE: &str = "This is a slice. Request the next page with the same selector and offset=next_offset. source_hash identifies the complete document; pass it back as source_hash so a changed document fails with SOURCE_CHANGED instead of paging inconsistently. Absence from this page does not establish absence from the document.";

/// Alias table of a tool: canonical parameter name and the alias names accepted for it.
pub type Aliases = Vec<(&'static str, Vec<&'static str>)>;

/// Rewrite aliases to their canonical parameter names. An alias given together with its canonical
/// parameter must carry the same value.
pub fn normalize_aliases(aliases: &Aliases, args: &Value) -> Result<Value, ToolError> {
    let mut args = args.clone();
    if aliases.is_empty() {
        return Ok(args);
    }
    let object = args
        .as_object_mut()
        .ok_or_else(|| ToolError::invalid("Arguments must be an object"))?;
    for (canonical, names) in aliases {
        for alias in names {
            let Some(value) = object.remove(*alias) else {
                continue;
            };
            match object.get(*canonical) {
                Some(existing) if existing != &value => {
                    return Err(ToolError::new(
                        "CONFLICTING_ARGUMENTS",
                        format!(
                            "{canonical} and its alias {alias} must agree (aliases: {})",
                            names.join(", ")
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    object.insert((*canonical).to_string(), value);
                }
            }
        }
    }
    Ok(args)
}

pub fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    opt_str(args, key).ok_or_else(|| ToolError::invalid(format!("Missing {key}")))
}

pub fn opt_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|v| usize::try_from(v).unwrap_or(usize::MAX))
}

pub fn opt_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Offset and limit of a list request.
pub fn list_window(args: &Value) -> Result<(usize, usize), ToolError> {
    let offset = opt_usize(args, "offset").unwrap_or(0);
    let limit = opt_usize(args, "limit").unwrap_or(DEFAULT_LIST_LIMIT);
    if !(1..=MAX_LIST_LIMIT).contains(&limit) {
        return Err(ToolError::invalid(format!(
            "limit must be 1..{MAX_LIST_LIMIT}"
        )));
    }
    Ok((offset, limit))
}

/// Page of an already materialised list.
pub fn page(items: Vec<Value>, args: &Value) -> Result<Value, ToolError> {
    let (offset, limit) = list_window(args)?;
    if offset > items.len() {
        return Err(ToolError::invalid("Invalid offset"));
    }
    let end = offset.saturating_add(limit).min(items.len());
    Ok(json!({
        "items": items[offset..end],
        "total": items.len(),
        "next_offset": (end < items.len()).then_some(end),
    }))
}

/// Page of a borrowed list: only the items of the requested window are converted.
pub fn page_slice<T>(
    items: &[T],
    args: &Value,
    convert: impl Fn(&T) -> Value,
) -> Result<Value, ToolError> {
    let (offset, limit) = list_window(args)?;
    if offset > items.len() {
        return Err(ToolError::invalid("Invalid offset"));
    }
    let end = offset.saturating_add(limit).min(items.len());
    Ok(json!({
        "items": items[offset..end].iter().map(convert).collect::<Vec<_>>(),
        "total": items.len(),
        "next_offset": (end < items.len()).then_some(end),
    }))
}

/// Page of a lazily produced list: `total` is only reported when the iterator was exhausted.
pub fn page_iter(items: impl Iterator<Item = Value>, args: &Value) -> Result<Value, ToolError> {
    let (offset, limit) = list_window(args)?;
    let mut page = Vec::new();
    let mut seen = 0usize;
    let mut more = false;
    for item in items {
        if seen >= offset {
            if page.len() < limit {
                page.push(item);
            } else {
                more = true;
                break;
            }
        }
        seen += 1;
    }
    if offset > seen {
        return Err(ToolError::invalid("Invalid offset"));
    }
    let end = offset + page.len();
    let mut out = json!({"items": page, "next_offset": more.then_some(end)});
    if !more {
        out["total"] = json!(end);
    }
    Ok(out)
}

/// Stable identity of a text document (FNV-1a, 64 bit).
pub fn text_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// Slice of a text document by Unicode scalar offset.
pub fn text_page(text: &str, args: &Value, format: &str) -> Result<Value, ToolError> {
    let start = opt_usize(args, "offset").unwrap_or(0);
    let limit = opt_usize(args, "limit").unwrap_or(DEFAULT_TEXT_LIMIT);
    if !(1..=MAX_TEXT_LIMIT).contains(&limit) {
        return Err(ToolError::invalid(format!(
            "Text limit must be 1..{MAX_TEXT_LIMIT} characters"
        )));
    }
    let total = text.chars().count();
    if start > total {
        return Err(ToolError::invalid("Invalid offset"));
    }
    let end = start.saturating_add(limit).min(total);
    let hash = text_hash(text);
    if let Some(expected) = opt_str(args, "source_hash") {
        if expected != hash {
            return Err(ToolError::new(
                "SOURCE_CHANGED",
                format!("source_hash {expected} does not match the current document ({hash})"),
            ));
        }
    }
    let slice: String = text.chars().skip(start).take(end - start).collect();
    Ok(json!({
        "text": slice,
        "format": format,
        "source_hash": hash,
        "offset": start,
        "total_chars": total,
        "returned_chars": end - start,
        "next_offset": (end < total).then_some(end),
        "has_more": end < total,
        "full_text_in_response": start == 0 && end == total,
        "offset_unit": "Unicode scalar characters",
        "pagination_note": (start != 0 || end < total).then_some(SLICE_NOTE),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_rewritten_and_conflicts_rejected() {
        let aliases: Aliases = vec![("class_id", vec!["class_name", "type_name"])];
        let out = normalize_aliases(&aliases, &json!({"class_name": "a.B"})).unwrap();
        assert_eq!(out, json!({"class_id": "a.B"}));
        let same = normalize_aliases(&aliases, &json!({"class_id":"a.B","type_name":"a.B"}));
        assert!(same.is_ok());
        let err =
            normalize_aliases(&aliases, &json!({"class_id":"a.B","type_name":"a.C"})).unwrap_err();
        assert_eq!(err.code, "CONFLICTING_ARGUMENTS");
    }

    #[test]
    fn list_paging_reports_next_offset() {
        let items: Vec<Value> = (0..5).map(|i| json!(i)).collect();
        let p = page(items.clone(), &json!({"limit": 2})).unwrap();
        assert_eq!(p["items"], json!([0, 1]));
        assert_eq!(p["next_offset"], json!(2));
        let p = page(items.clone(), &json!({"limit": 2, "offset": 4})).unwrap();
        assert_eq!(p["items"], json!([4]));
        assert!(p["next_offset"].is_null());
        assert!(page(items.clone(), &json!({"offset": 6})).is_err());
        assert!(page(items, &json!({"limit": 0})).is_err());
    }

    #[test]
    fn lazy_paging_stops_early() {
        let p = page_iter(
            (0..1000).map(|i| json!(i)),
            &json!({"limit": 3, "offset": 1}),
        )
        .unwrap();
        assert_eq!(p["items"], json!([1, 2, 3]));
        assert_eq!(p["next_offset"], json!(4));
        assert!(p.get("total").is_none());
        let p = page_iter((0..3).map(|i| json!(i)), &json!({})).unwrap();
        assert_eq!(p["total"], json!(3));
    }

    #[test]
    fn text_paging_is_unicode_aware_and_hash_checked() {
        let text = "a\u{e9}\u{1f600}bcd";
        let p = text_page(text, &json!({"limit": 3}), "java").unwrap();
        assert_eq!(p["text"], json!("a\u{e9}\u{1f600}"));
        assert_eq!(p["next_offset"], json!(3));
        let hash = p["source_hash"].as_str().unwrap().to_string();
        let p2 = text_page(text, &json!({"offset":3,"source_hash":hash}), "java").unwrap();
        assert_eq!(p2["text"], json!("bcd"));
        assert_eq!(p2["has_more"], json!(false));
        let err = text_page(text, &json!({"source_hash": "x"}), "java").unwrap_err();
        assert_eq!(err.code, "SOURCE_CHANGED");
    }
}
