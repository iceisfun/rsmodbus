//! Logging implementations. Port of Go package `logging` (`logger.go`, `noop_logger.go`).
//!
//! Implementation detail, not part of the Modbus spec: leveled, structured
//! logging with optional hexdumps of raw frames (see Section 4.1 for the frame
//! layout being dumped).
//!
//! ```
//! use rsmodbus::common::{Context, LogLevel, LoggerInterface};
//! use rsmodbus::logging::Logger;
//!
//! let logger = Logger::new().with_level(LogLevel::Debug); // stdout by default
//! logger.info(&Context::background(), format_args!("hello {}", 42));
//! ```

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::{Context, Fields, LogLevel, LoggerInterface};

/// Shared, lockable output sink (several loggers derived via `with_fields` share one).
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Logger implements [`LoggerInterface`] (including `hexdump`).
///
/// Defaults match Go: level `Info`, writer stdout, no fields. Build with the
/// consuming `with_*` methods (Go functional options `WithLevel`, `WithWriter`,
/// `WithFields`).
pub struct Logger {
    level: AtomicU8,
    writer: SharedWriter,
    fields: Fields,
    // True when the sink is stderr: a failed write must not be re-reported on stderr.
    writer_is_stderr: bool,
}

impl Default for Logger {
    fn default() -> Self {
        Logger::new()
    }
}

impl Logger {
    /// Creates a logger writing to stdout at `Info` level (Go `NewLogger()`).
    pub fn new() -> Logger {
        Logger {
            level: AtomicU8::new(LogLevel::Info as u8),
            writer: Arc::new(Mutex::new(Box::new(std::io::stdout()))),
            fields: Fields::new(),
            writer_is_stderr: false,
        }
    }

    /// Sets the log level (Go option `WithLevel`).
    pub fn with_level(self, level: LogLevel) -> Logger {
        self.level.store(level as u8, Ordering::SeqCst);
        self
    }

    /// Sets the output writer (Go option `WithWriter`).
    pub fn with_writer(mut self, writer: Box<dyn Write + Send>) -> Logger {
        self.writer = Arc::new(Mutex::new(writer));
        self.writer_is_stderr = false;
        self
    }

    /// Writes to stderr (Go `WithWriter(os.Stderr)`).
    pub fn with_stderr(mut self) -> Logger {
        self.writer = Arc::new(Mutex::new(Box::new(std::io::stderr())));
        self.writer_is_stderr = true;
        self
    }

    /// Adds fields at construction time (Go option `WithFields`; merges into existing fields).
    pub fn with_initial_fields(mut self, fields: Fields) -> Logger {
        for (k, v) in fields {
            self.fields.insert(k, v);
        }
        self
    }

    // Current level.
    fn level(&self) -> LogLevel {
        LogLevel::from_u8(self.level.load(Ordering::SeqCst))
    }

    // Renders ` key="value" key2="value2"` (empty string when there are no fields).
    fn fields_suffix(&self) -> String {
        if self.fields.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = self.fields.iter().map(|(k, v)| format!("{}={:?}", k, v)).collect();
        format!(" {}", parts.join(" "))
    }

    // Writes `output`; on failure falls back to stderr (unless stderr is the sink).
    fn write_out(&self, output: &str, what: &str) {
        let mut w = match self.writer.lock() {
            Ok(w) => w,
            Err(p) => p.into_inner(),
        };
        if let Err(e) = w.write_all(output.as_bytes()) {
            // Cannot log the error (that would loop); write directly to stderr as a last resort.
            if !self.writer_is_stderr {
                eprintln!("ERROR: Failed to write {}: {}", what, e);
            }
        }
    }

    // Internal: formats and writes one entry, like Go's `log`.
    fn log(&self, level: &str, args: std::fmt::Arguments<'_>) {
        let message = std::fmt::format(args);
        let mut entry = format!("[{}] {}: {}", rfc3339_now(), level, message);
        entry.push_str(&self.fields_suffix());
        if !entry.ends_with('\n') {
            entry.push('\n');
        }
        self.write_out(&entry, "log entry");
    }
}

impl LoggerInterface for Logger {
    fn trace(&self, _ctx: &Context, args: std::fmt::Arguments<'_>) {
        if self.level() <= LogLevel::Trace {
            self.log("TRACE", args);
        }
    }

    fn debug(&self, _ctx: &Context, args: std::fmt::Arguments<'_>) {
        if self.level() <= LogLevel::Debug {
            self.log("DEBUG", args);
        }
    }

    fn info(&self, _ctx: &Context, args: std::fmt::Arguments<'_>) {
        if self.level() <= LogLevel::Info {
            self.log("INFO", args);
        }
    }

    fn warn(&self, _ctx: &Context, args: std::fmt::Arguments<'_>) {
        if self.level() <= LogLevel::Warn {
            self.log("WARN", args);
        }
    }

    fn error(&self, _ctx: &Context, args: std::fmt::Arguments<'_>) {
        if self.level() <= LogLevel::Error {
            self.log("ERROR", args);
        }
    }

    // Returns a new logger sharing level snapshot and writer, with existing fields
    // copied and `fields` added.
    fn with_fields(&self, fields: Fields) -> Arc<dyn LoggerInterface> {
        let mut merged = self.fields.clone();
        for (k, v) in fields {
            merged.insert(k, v);
        }
        Arc::new(Logger {
            level: AtomicU8::new(self.level.load(Ordering::SeqCst)),
            writer: Arc::clone(&self.writer),
            fields: merged,
            writer_is_stderr: self.writer_is_stderr,
        })
    }

    fn get_level(&self) -> LogLevel {
        self.level()
    }

    fn set_level(&self, level: LogLevel) {
        self.level.store(level as u8, Ordering::SeqCst);
    }

    /// Outputs a hexdump of the given data, only when the level is `Trace`.
    /// Format: `offset   00 01 02 03 04 05 06 07 | 08 09 0a 0b 0c 0d 0e 0f`
    fn hexdump(&self, _ctx: &Context, data: &[u8]) {
        // Only hexdump if the logger is at TRACE level
        if self.level() > LogLevel::Trace {
            return;
        }

        let mut out = format!("[{}] TRACE: HEXDUMP\n", rfc3339_now());
        // First line is the header for the columns
        out.push_str("offset   00 01 02 03 04 05 06 07 | 08 09 0a 0b 0c 0d 0e 0f\n");

        // Process the data 16 bytes at a time
        for (row, chunk) in data.chunks(16).enumerate() {
            out.push_str(&format!("{:08x}", row * 16));
            for j in 0..16 {
                // Separator between left and right halves
                if j == 8 {
                    out.push_str(" |");
                }
                out.push(' ');
                match chunk.get(j) {
                    Some(b) => out.push_str(&format!("{:02x}", b)),
                    // Spaces keep alignment on a short last row
                    None => out.push_str("  "),
                }
            }
            out.push('\n');
        }

        let fields = self.fields_suffix();
        if !fields.is_empty() {
            out.push_str(&fields);
            out.push('\n');
        }
        self.write_out(&out, "hexdump");
    }
}

/// NoopLogger is a logger that does nothing (Go `NoopLogger`).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopLogger;

impl NoopLogger {
    /// Creates a new NoopLogger (Go `NewNoopLogger`).
    pub fn new() -> NoopLogger {
        NoopLogger
    }
}

impl LoggerInterface for NoopLogger {
    fn trace(&self, _ctx: &Context, _args: std::fmt::Arguments<'_>) {}
    fn debug(&self, _ctx: &Context, _args: std::fmt::Arguments<'_>) {}
    fn info(&self, _ctx: &Context, _args: std::fmt::Arguments<'_>) {}
    fn warn(&self, _ctx: &Context, _args: std::fmt::Arguments<'_>) {}
    fn error(&self, _ctx: &Context, _args: std::fmt::Arguments<'_>) {}

    fn with_fields(&self, _fields: Fields) -> Arc<dyn LoggerInterface> {
        Arc::new(NoopLogger)
    }

    fn get_level(&self) -> LogLevel {
        LogLevel::None
    }

    fn set_level(&self, _level: LogLevel) {}
}

/// Convenience: a shared no-op logger.
pub fn noop() -> Arc<dyn LoggerInterface> {
    Arc::new(NoopLogger)
}

/// Convenience: a shared default (stdout, Info) logger, i.e. Go `logging.NewLogger()`.
pub fn default_logger() -> Arc<dyn LoggerInterface> {
    Arc::new(Logger::new())
}

// Formats "now" as RFC 3339 in UTC (`2006-01-02T15:04:05Z`), like Go's
// `time.RFC3339` but always UTC because std has no timezone database.
fn rfc3339_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format_rfc3339(secs)
}

// Seconds since the Unix epoch -> RFC 3339 UTC string (civil-from-days algorithm).
fn format_rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // In-memory sink for assertions.
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    #[test]
    fn rfc3339_known_values() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(format_rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn levels_filter() {
        let buf = Buf::default();
        let l = Logger::new().with_writer(Box::new(buf.clone())).with_level(LogLevel::Warn);
        let ctx = Context::background();
        l.debug(&ctx, format_args!("no"));
        l.info(&ctx, format_args!("no"));
        l.warn(&ctx, format_args!("w {}", 1));
        l.error(&ctx, format_args!("e"));
        let t = buf.text();
        assert!(t.contains("WARN: w 1\n"));
        assert!(t.contains("ERROR: e\n"));
        assert!(!t.contains("no"));
        l.set_level(LogLevel::None);
        assert_eq!(l.get_level(), LogLevel::None);
        l.error(&ctx, format_args!("silent"));
        assert!(!buf.text().contains("silent"));
    }

    #[test]
    fn default_level_is_info() {
        assert_eq!(Logger::new().get_level(), LogLevel::Info);
    }

    #[test]
    fn fields_are_appended_and_inherited() {
        let buf = Buf::default();
        let l = Logger::new().with_writer(Box::new(buf.clone()));
        let mut f = Fields::new();
        f.insert("unit".into(), "1".into());
        let l2 = l.with_fields(f);
        let mut f = Fields::new();
        f.insert("a".into(), "x y".into());
        let l3 = l2.with_fields(f);
        l3.info(&Context::background(), format_args!("msg"));
        let t = buf.text();
        assert!(t.trim_end().ends_with(r#"INFO: msg a="x y" unit="1""#), "{}", t);
    }

    #[test]
    fn hexdump_only_at_trace() {
        let buf = Buf::default();
        let l = Logger::new().with_writer(Box::new(buf.clone()));
        l.hexdump(&Context::background(), &[1, 2, 3]);
        assert!(buf.text().is_empty());
        l.set_level(LogLevel::Trace);
        let data: Vec<u8> = (0u8..20).collect();
        l.hexdump(&Context::background(), &data);
        let t = buf.text();
        assert!(t.contains("TRACE: HEXDUMP\n"));
        assert!(t.contains("offset   00 01 02 03 04 05 06 07 | 08 09 0a 0b 0c 0d 0e 0f\n"));
        assert!(t.contains("00000000 00 01 02 03 04 05 06 07 | 08 09 0a 0b 0c 0d 0e 0f\n"));
        let short = format!("00000010 10 11 12 13{} |{}\n", " ".repeat(12), " ".repeat(24));
        assert!(t.contains(&short), "{}", t);
    }

    #[test]
    fn noop_logger() {
        let n = NoopLogger::new();
        n.error(&Context::background(), format_args!("x"));
        assert_eq!(n.get_level(), LogLevel::None);
        assert_eq!(n.with_fields(Fields::new()).get_level(), LogLevel::None);
    }
}
