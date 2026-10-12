//! Custom logger example. Port of `gomodbus/cmd/logger/main.go`.
//!
//! Shows how to plug any logger into the library by implementing
//! `common::LoggerInterface`. Logging is an implementation detail, not part of
//! the Modbus spec; the demo reads holding registers per
//! Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (03 Read Holding Registers).

use std::fmt;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rsmodbus::common::*;

/// CustomLogger implements `LoggerInterface` with a `[MODBUS] ` prefix and
/// Go `log.LstdFlags|log.Lmicroseconds` timestamps (`2006/01/02 15:04:05.000000`, UTC).
struct CustomLogger {
    level: AtomicU8,
}

impl CustomLogger {
    /// Creates a new custom logger.
    fn new(level: LogLevel) -> CustomLogger {
        CustomLogger { level: AtomicU8::new(level as u8) }
    }

    /// Writes one line if `at` is enabled (Go `l.logger.Printf("[LEVEL] "+format, ...)`).
    fn log(&self, at: LogLevel, tag: &str, args: fmt::Arguments<'_>) {
        if self.get_level() <= at {
            let line = format!("[MODBUS] {} [{}] {}\n", timestamp(), tag, args);
            let _ = std::io::stdout().write_all(line.as_bytes());
        }
    }
}

/// `YYYY/MM/DD HH:MM:SS.uuuuuu` in UTC (std has no time zone database).
fn timestamp() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let secs = d.as_secs();
    let (days, rem) = (secs / 86400, secs % 86400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}/{month:02}/{day:02} {:02}:{:02}:{:02}.{:06}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_micros()
    )
}

impl LoggerInterface for CustomLogger {
    fn trace(&self, _ctx: &Context, args: fmt::Arguments<'_>) {
        self.log(LogLevel::Trace, "TRACE", args);
    }
    fn debug(&self, _ctx: &Context, args: fmt::Arguments<'_>) {
        self.log(LogLevel::Debug, "DEBUG", args);
    }
    fn info(&self, _ctx: &Context, args: fmt::Arguments<'_>) {
        self.log(LogLevel::Info, "INFO", args);
    }
    fn warn(&self, _ctx: &Context, args: fmt::Arguments<'_>) {
        self.log(LogLevel::Warn, "WARN", args);
    }
    fn error(&self, _ctx: &Context, args: fmt::Arguments<'_>) {
        self.log(LogLevel::Error, "ERROR", args);
    }
    fn get_level(&self) -> LogLevel {
        LogLevel::from_u8(self.level.load(Ordering::Relaxed))
    }
    fn set_level(&self, level: LogLevel) {
        self.level.store(level as u8, Ordering::Relaxed);
    }
    /// Simple implementation that ignores fields, as in the Go example.
    fn with_fields(&self, _fields: Fields) -> Arc<dyn LoggerInterface> {
        Arc::new(CustomLogger::new(self.get_level()))
    }
}

fn main() {
    // Connection parameters (hard-coded like the Go example).
    let args = rsmodbus::cli::ModbusArgs {
        ip: "127.0.0.1".into(),
        port: 502,
        unit_id: 1,
        timeout: Duration::from_secs(5),
        log_level: "debug".into(),
        log_level_id: LogLevel::Debug,
    };

    let logger: Arc<dyn LoggerInterface> = Arc::new(CustomLogger::new(LogLevel::Debug));
    println!("Using custom logger implementation...");

    let client = rsmodbus::cli::create_client_with_logger(&args, logger);

    let ctx = Context::background();
    if let Err(e) = client.connect(&ctx) {
        println!("Failed to connect to Modbus server: {e}");
        return;
    }

    // Read some holding registers (Ref: Section 6.3).
    match client.read_holding_registers(&ctx, Address(0), Quantity(10)) {
        Ok(registers) => {
            println!("Read holding registers:");
            for (i, value) in registers.iter().enumerate() {
                println!("Register {i}: {value} (0x{value:04X})");
            }
        }
        Err(e) => println!("Failed to read holding registers: {e}"),
    }
    let _ = client.disconnect(&ctx);
}
