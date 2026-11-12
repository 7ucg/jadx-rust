//! Error type of the API and the per node error/warning bookkeeping.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Mutex;

use crate::log::{Level, LogSink};

/// Failure of a decompiler operation.
#[derive(Debug)]
pub enum JadxError {
    /// Invalid arguments (`JadxArgsValidateException`).
    Args(String),
    /// The inputs could not be loaded.
    Load(String),
    /// Any other failure of the run.
    Runtime(String),
    Io(std::io::Error),
}

impl fmt::Display for JadxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JadxError::Args(m) | JadxError::Load(m) | JadxError::Runtime(m) => f.write_str(m),
            JadxError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for JadxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            JadxError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for JadxError {
    fn from(e: std::io::Error) -> Self {
        JadxError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, JadxError>;

#[derive(Default)]
struct Entries {
    errors: BTreeMap<String, BTreeSet<String>>,
    warns: BTreeMap<String, BTreeSet<String>>,
}

/// Collects the errors and warnings of all nodes (`ErrorsCounter`). Thread safe; the same
/// message for the same node is recorded once, so processing a class twice does not inflate the
/// counts. Reports are sorted, independent of the processing order.
#[derive(Default)]
pub struct ErrorsCounter {
    inner: Mutex<Entries>,
}

impl ErrorsCounter {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record an error of `node` (for example `class com.a.B`).
    pub fn add_error(&self, node: &str, msg: &str) {
        self.lock()
            .errors
            .entry(node.to_string())
            .or_default()
            .insert(msg.to_string());
    }

    pub fn add_warn(&self, node: &str, msg: &str) {
        self.lock()
            .warns
            .entry(node.to_string())
            .or_default()
            .insert(msg.to_string());
    }

    /// Number of nodes with errors.
    pub fn error_count(&self) -> usize {
        self.lock().errors.len()
    }

    /// Number of warnings.
    pub fn warns_count(&self) -> usize {
        self.lock().warns.values().map(|s| s.len()).sum()
    }

    pub fn clear(&self) {
        let mut g = self.lock();
        g.errors.clear();
        g.warns.clear();
    }

    /// Errors as `(node, messages)`, sorted by node.
    pub fn errors(&self) -> Vec<(String, Vec<String>)> {
        self.lock()
            .errors
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
            .collect()
    }

    /// Print the summary: the list of nodes with errors and the warnings count.
    pub fn print_report(&self, log: &dyn LogSink) {
        let g = self.lock();
        if !g.errors.is_empty() {
            log.log(
                Level::Error,
                &format!("{} errors occurred in following nodes:", g.errors.len()),
            );
            for node in g.errors.keys() {
                log.log(Level::Error, &format!("  {node}"));
            }
        }
        let warn_count: usize = g.warns.values().map(|s| s.len()).sum();
        if warn_count > 0 {
            log.log(
                Level::Warn,
                &format!("{} warnings in {} nodes", warn_count, g.warns.len()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_nodes_once() {
        let c = ErrorsCounter::new();
        c.add_error("class A", "x");
        c.add_error("class A", "x");
        c.add_error("class A", "y");
        c.add_error("class B", "z");
        c.add_warn("class A", "w");
        assert_eq!(c.error_count(), 2);
        assert_eq!(c.warns_count(), 1);
        assert_eq!(c.errors()[0].0, "class A");
        c.clear();
        assert_eq!(c.error_count(), 0);
    }
}
