//! Tool errors: a stable machine readable code plus a human readable message.

use serde_json::{json, Value};

/// Failure of a tool call. Reported to the client as a tool result with `isError: true`, never
/// as a JSON-RPC protocol error.
#[derive(Debug, Clone)]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
    pub details: Option<Value>,
}

pub type ToolResult = Result<Value, ToolError>;

impl ToolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        ToolError {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("INVALID_ARGUMENT", message)
    }

    pub fn no_project() -> Self {
        Self::new(
            "NO_PROJECT",
            "No project is loaded; call open_project with a path first",
        )
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new("UNSUPPORTED", message)
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new("TOOL_ERROR", message)
    }

    pub fn cancelled() -> Self {
        Self::new("CANCELLED", "request cancelled")
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// The structured error object returned in `structuredContent`.
    pub fn to_json(&self) -> Value {
        let mut error = json!({"code": self.code, "message": self.message});
        if let Some(details) = &self.details {
            error["details"] = details.clone();
        }
        json!({ "error": error })
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}
