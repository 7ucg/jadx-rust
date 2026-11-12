//! Message sink of the decompiler: the front end decides what to print.

/// Severity of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }
}

/// Receiver of log messages.
pub trait LogSink: Send + Sync {
    fn log(&self, level: Level, msg: &str);
}

/// Drops every message.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullLog;

impl LogSink for NullLog {
    fn log(&self, _level: Level, _msg: &str) {}
}
