//! Logger interface. Port of `gomodbus/common/logger.go`.
//!
//! Implementation detail, not part of the Modbus spec.
//!
//! Go's `format string, args ...interface{}` becomes a single
//! [`std::fmt::Arguments`] built with `format_args!`, so nothing is formatted
//! when the level is disabled:
//!
//! ```ignore
//! logger.debug(&ctx, format_args!("reading {} registers", n));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use super::context::Context;

/// Structured log fields (Go `map[string]interface{}`; values are pre-rendered
/// with their `Display`/`%v` text). Sorted by key for stable output.
pub type Fields = BTreeMap<String, String>;

/// LogLevel represents a logging level (ordered: `Trace < Debug < ... < None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// The most verbose logging level.
    Trace = 0,
    /// Debug messages.
    Debug = 1,
    /// General information.
    Info = 2,
    /// Warnings.
    Warn = 3,
    /// Errors.
    Error = 4,
    /// Disables all logging.
    None = 5,
}

/// Go `LevelTrace`.
pub const LEVEL_TRACE: LogLevel = LogLevel::Trace;
/// Go `LevelDebug`.
pub const LEVEL_DEBUG: LogLevel = LogLevel::Debug;
/// Go `LevelInfo`.
pub const LEVEL_INFO: LogLevel = LogLevel::Info;
/// Go `LevelWarn`.
pub const LEVEL_WARN: LogLevel = LogLevel::Warn;
/// Go `LevelError`.
pub const LEVEL_ERROR: LogLevel = LogLevel::Error;
/// Go `LevelNone`.
pub const LEVEL_NONE: LogLevel = LogLevel::None;

impl LogLevel {
    /// Converts a stored discriminant back to a level (values > 5 map to `None`).
    pub fn from_u8(v: u8) -> LogLevel {
        match v {
            0 => LogLevel::Trace,
            1 => LogLevel::Debug,
            2 => LogLevel::Info,
            3 => LogLevel::Warn,
            4 => LogLevel::Error,
            _ => LogLevel::None,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::None => "NONE",
        })
    }
}

/// LoggerInterface defines the interface for a logger (Go `LoggerInterface`).
///
/// Share loggers as `Arc<dyn LoggerInterface>`. `hexdump` folds Go's optional
/// `LoggerInterfaceHexdump` interface into the trait with a no-op default, so
/// callers simply call `logger.hexdump(..)` (no type assertion needed).
pub trait LoggerInterface: Send + Sync {
    /// Logs a trace message.
    fn trace(&self, ctx: &Context, args: fmt::Arguments<'_>);
    /// Logs a debug message.
    fn debug(&self, ctx: &Context, args: fmt::Arguments<'_>);
    /// Logs an info message.
    fn info(&self, ctx: &Context, args: fmt::Arguments<'_>);
    /// Logs a warning message.
    fn warn(&self, ctx: &Context, args: fmt::Arguments<'_>);
    /// Logs an error message.
    fn error(&self, ctx: &Context, args: fmt::Arguments<'_>);
    /// Returns a new logger with the given fields added.
    fn with_fields(&self, fields: Fields) -> Arc<dyn LoggerInterface>;
    /// Returns the current log level.
    fn get_level(&self) -> LogLevel;
    /// Sets the log level.
    fn set_level(&self, level: LogLevel);
    /// Logs a hexdump of the given data (optional, extra-verbose protocol debug).
    /// Default: does nothing.
    fn hexdump(&self, _ctx: &Context, _data: &[u8]) {}
}

/// Go `LoggerInterfaceHexdump`: the optional hexdump capability. In Rust it is
/// part of [`LoggerInterface`] (`hexdump`); this alias exists only so the Go
/// name can be found.
pub use self::LoggerInterface as LoggerInterfaceHexdump;
