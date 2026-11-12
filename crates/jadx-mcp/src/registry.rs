//! Tool registry: declarative tool specifications, JSON schema generation and argument
//! validation.
//!
//! Adapted from the tool table, schema generation and validation of the RDX MCP server
//! (Apache-2.0, Copyright 2026 Ch0pin and RDX contributors). Modified: tools are described by a
//! typed [`ToolSpec`] instead of one large function, the parameter model is reduced to what this
//! server needs, and handlers are plain functions over a [`ToolCtx`].

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::args::{normalize_aliases, Aliases, MAX_LIST_LIMIT, MAX_TEXT_LIMIT};
use crate::error::{ToolError, ToolResult};
use crate::server::ToolCtx;

pub type Handler = fn(&ToolCtx, &Value) -> ToolResult;

/// Type of a parameter.
#[derive(Debug, Clone)]
pub enum Kind {
    Str,
    /// String or array of strings.
    StrOrList,
    Int {
        min: i64,
        max: i64,
    },
    Bool,
    Enum(&'static [&'static str]),
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub description: &'static str,
    pub default: Option<Value>,
}

impl Param {
    pub fn string(name: &'static str, description: &'static str) -> Self {
        Param {
            name,
            kind: Kind::Str,
            description,
            default: None,
        }
    }

    pub fn str_or_list(name: &'static str, description: &'static str) -> Self {
        Param {
            name,
            kind: Kind::StrOrList,
            description,
            default: None,
        }
    }

    pub fn int(name: &'static str, min: i64, max: i64, description: &'static str) -> Self {
        Param {
            name,
            kind: Kind::Int { min, max },
            description,
            default: None,
        }
    }

    pub fn boolean(name: &'static str, default: bool, description: &'static str) -> Self {
        Param {
            name,
            kind: Kind::Bool,
            description,
            default: Some(json!(default)),
        }
    }

    pub fn choice(
        name: &'static str,
        values: &'static [&'static str],
        default: &'static str,
        description: &'static str,
    ) -> Self {
        Param {
            name,
            kind: Kind::Enum(values),
            description,
            default: Some(json!(default)),
        }
    }

    pub fn with_default(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }

    fn schema(&self) -> Value {
        let mut schema = match &self.kind {
            Kind::Str => json!({"type": "string", "minLength": 1}),
            Kind::StrOrList => json!({
                "type": ["string", "array"],
                "items": {"type": "string", "minLength": 1},
                "maxItems": 64,
            }),
            Kind::Int { min, max } => json!({"type": "integer", "minimum": min, "maximum": max}),
            Kind::Bool => json!({"type": "boolean"}),
            Kind::Enum(values) => json!({"type": "string", "enum": values}),
        };
        if !self.description.is_empty() {
            schema["description"] = json!(self.description);
        }
        if let Some(default) = &self.default {
            schema["default"] = default.clone();
        }
        schema
    }

    fn check(&self, value: &Value) -> Result<(), ToolError> {
        let name = self.name;
        match &self.kind {
            Kind::Str => match value.as_str() {
                Some(s) if !s.is_empty() => Ok(()),
                _ => Err(ToolError::invalid(format!(
                    "{name} must be a non-empty string"
                ))),
            },
            Kind::StrOrList => {
                let ok = match value {
                    Value::String(s) => !s.is_empty(),
                    Value::Array(items) => {
                        items.len() <= 64
                            && items
                                .iter()
                                .all(|i| i.as_str().is_some_and(|s| !s.is_empty()))
                    }
                    _ => false,
                };
                if ok {
                    Ok(())
                } else {
                    Err(ToolError::invalid(format!(
                        "{name} must be a non-empty string or an array of up to 64 non-empty strings"
                    )))
                }
            }
            Kind::Int { min, max } => match value.as_i64() {
                Some(v) if (*min..=*max).contains(&v) => Ok(()),
                _ => Err(ToolError::invalid(format!(
                    "{name} must be an integer in {min}..{max}"
                ))),
            },
            Kind::Bool => {
                if value.is_boolean() {
                    Ok(())
                } else {
                    Err(ToolError::invalid(format!("{name} must be a boolean")))
                }
            }
            Kind::Enum(values) => match value.as_str() {
                Some(s) if values.contains(&s) => Ok(()),
                _ => Err(ToolError::invalid(format!(
                    "{name} must be one of: {}",
                    values.join(", ")
                ))),
            },
        }
    }
}

/// Declaration of one tool.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub params: Vec<Param>,
    /// Parameters that must always be present.
    pub required: Vec<&'static str>,
    /// Alternative parameter sets of which at least one must be present completely.
    pub any_of: Vec<Vec<&'static str>>,
    pub aliases: Aliases,
    pub read_only: bool,
    pub needs_project: bool,
    pub handler: Handler,
}

impl ToolSpec {
    pub fn new(name: &'static str, description: &'static str, handler: Handler) -> Self {
        ToolSpec {
            name,
            description,
            params: Vec::new(),
            required: Vec::new(),
            any_of: Vec::new(),
            aliases: Vec::new(),
            read_only: true,
            needs_project: true,
            handler,
        }
    }

    pub fn param(mut self, param: Param) -> Self {
        self.params.push(param);
        self
    }

    pub fn require(mut self, param: Param) -> Self {
        self.required.push(param.name);
        self.params.push(param);
        self
    }

    pub fn any_of(mut self, alternatives: &[&[&'static str]]) -> Self {
        self.any_of = alternatives.iter().map(|a| a.to_vec()).collect();
        self
    }

    pub fn alias(mut self, canonical: &'static str, names: &[&'static str]) -> Self {
        self.aliases.push((canonical, names.to_vec()));
        self
    }

    pub fn writes(mut self) -> Self {
        self.read_only = false;
        self
    }

    pub fn no_project(mut self) -> Self {
        self.needs_project = false;
        self
    }

    /// Offset and limit of a list result.
    pub fn paged(self) -> Self {
        self.param(Param::int(
            "offset",
            0,
            i64::MAX,
            "Items to skip (use next_offset of the previous page)",
        ))
        .param(
            Param::int("limit", 1, MAX_LIST_LIMIT as i64, "Page size, default 100")
                .with_default(json!(100)),
        )
    }

    /// Offset, limit and source hash of a text result.
    pub fn text_paged(self) -> Self {
        self.param(Param::int(
            "offset",
            0,
            i64::MAX,
            "Unicode character offset (use next_offset of the previous page)",
        ))
        .param(
            Param::int("limit", 1, MAX_TEXT_LIMIT as i64, "Characters per page, default 32000")
                .with_default(json!(32000)),
        )
        .param(Param::string(
            "source_hash",
            "source_hash of an earlier page of the same document; a changed document fails with SOURCE_CHANGED",
        ))
    }

    /// Class selector `class_id` with the accepted aliases, required.
    pub fn class_selector(self) -> Self {
        self.param(Param::string(
            "class_id",
            "Class: dotted name (a.b.Outer$Inner), descriptor (La/b/Outer$Inner;) or a unique short name",
        ))
        .alias("class_id", &["class_name", "type_name"])
        .any_of(&[&["class_id"]])
    }

    fn expanded_any_of(&self) -> Vec<Vec<&'static str>> {
        let mut out = Vec::new();
        for option in &self.any_of {
            out.push(option.clone());
            for (canonical, names) in &self.aliases {
                if option.contains(canonical) {
                    for alias in names {
                        out.push(
                            option
                                .iter()
                                .map(|k| if k == canonical { *alias } else { *k })
                                .collect(),
                        );
                    }
                }
            }
        }
        out
    }

    /// The `tools/list` entry of this tool.
    pub fn definition(&self) -> Value {
        let mut properties = Map::new();
        for param in &self.params {
            properties.insert(param.name.to_string(), param.schema());
        }
        for (canonical, names) in &self.aliases {
            for alias in names {
                properties.insert(
                    (*alias).to_string(),
                    json!({
                        "type": "string",
                        "minLength": 1,
                        "description": format!("Alias for {canonical}; conflicting values are rejected"),
                    }),
                );
            }
        }
        let mut schema = json!({
            "type": "object",
            "properties": properties,
            "required": self.required,
            "additionalProperties": false,
        });
        let alternatives = self.expanded_any_of();
        if !alternatives.is_empty() {
            schema["anyOf"] = Value::Array(
                alternatives
                    .iter()
                    .map(|required| json!({"required": required}))
                    .collect(),
            );
        }
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": false,
                "openWorldHint": false,
            },
        })
    }

    /// Normalise aliases and validate the arguments against this specification.
    pub fn validate(&self, args: &Value) -> Result<Value, ToolError> {
        let normalized = normalize_aliases(&self.aliases, args)?;
        let object = normalized
            .as_object()
            .ok_or_else(|| ToolError::invalid("Arguments must be an object"))?;
        let known: BTreeMap<&str, &Param> = self.params.iter().map(|p| (p.name, p)).collect();
        for key in object.keys() {
            if !known.contains_key(key.as_str()) {
                let accepted = self.accepted_names().join(", ");
                return Err(ToolError::invalid(format!(
                    "Unknown argument: {key}; accepted arguments: {accepted}"
                )));
            }
        }
        for key in &self.required {
            if !object.contains_key(*key) {
                return Err(ToolError::invalid(format!("Missing {key}")));
            }
        }
        for (key, value) in object {
            known[key.as_str()].check(value)?;
        }
        if !self.any_of.is_empty()
            && !self
                .any_of
                .iter()
                .any(|set| set.iter().all(|k| object.contains_key(*k)))
        {
            let sets: Vec<String> = self
                .expanded_any_of()
                .iter()
                .map(|s| s.join(" + "))
                .collect();
            return Err(ToolError::invalid(format!(
                "Missing selector; provide one of: {}",
                sets.join(" | ")
            )));
        }
        Ok(normalized)
    }

    fn accepted_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.params.iter().map(|p| p.name).collect();
        for (_, aliases) in &self.aliases {
            names.extend(aliases.iter().copied());
        }
        names
    }
}

/// All tools of the server.
pub struct Registry {
    tools: Vec<ToolSpec>,
}

impl Registry {
    pub fn new(tools: Vec<ToolSpec>) -> Self {
        Registry { tools }
    }

    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.name == name)
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name).collect()
    }

    pub fn list(&self) -> Value {
        json!({"tools": self.tools.iter().map(ToolSpec::definition).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy(_: &ToolCtx, _: &Value) -> ToolResult {
        Ok(json!({}))
    }

    fn spec() -> ToolSpec {
        ToolSpec::new("demo", "Demo tool", dummy)
            .class_selector()
            .param(Param::choice("mode", &["a", "b"], "a", ""))
            .paged()
    }

    #[test]
    fn schema_lists_aliases_and_alternatives() {
        let def = spec().definition();
        let schema = &def["inputSchema"];
        assert!(schema["properties"]["class_name"].is_object());
        let any_of = schema["anyOf"].as_array().unwrap();
        assert_eq!(any_of.len(), 3);
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(def["annotations"]["readOnlyHint"], json!(true));
    }

    #[test]
    fn validation_reports_unknown_missing_and_bad_values() {
        let s = spec();
        let ok = s
            .validate(&json!({"class_name": "a.B", "limit": 5}))
            .unwrap();
        assert_eq!(ok["class_id"], json!("a.B"));
        let err = s
            .validate(&json!({"class_id": "a.B", "bogus": 1}))
            .unwrap_err();
        assert!(err
            .message
            .starts_with("Unknown argument: bogus; accepted arguments:"));
        let err = s.validate(&json!({})).unwrap_err();
        assert!(err.message.contains("Missing selector"));
        assert!(s.validate(&json!({"class_id": "a", "limit": 0})).is_err());
        assert!(s.validate(&json!({"class_id": "a", "mode": "z"})).is_err());
        assert!(s.validate(&json!({"class_id": ""})).is_err());
        assert!(s.validate(&json!([])).is_err());
    }
}
