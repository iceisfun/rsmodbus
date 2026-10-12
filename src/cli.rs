//! Command-line helpers shared by the `src/bin/*` programs.
//! Port of `gomodbus/cmd/args/args.go`.
//!
//! Implementation detail, not part of the Modbus spec. Hand-written replacement
//! for Go's `flag` package (no dependencies): accepts `-name value`,
//! `--name value`, `-name=value` and `--name=value`; bool flags accept `-name`
//! and `-name=false`.

use std::time::Duration;

use crate::common::LogLevel;

/// Parses a Go-style duration such as `5s`, `100ms`, `1m30s`, `1.5h`, `0`
/// (Go `time.ParseDuration`; units ns, us/µs, ms, s, m, h).
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let orig = s;
    let mut rest = s;
    if rest.starts_with('-') {
        return Err(format!("negative duration {orig:?}"));
    }
    rest = rest.strip_prefix('+').unwrap_or(rest);
    if rest == "0" {
        return Ok(Duration::ZERO);
    }
    if rest.is_empty() {
        return Err(format!("invalid duration {orig:?}"));
    }
    let mut total = 0f64;
    while !rest.is_empty() {
        let num_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_end == 0 {
            return Err(format!("invalid duration {orig:?}"));
        }
        let value: f64 = rest[..num_end]
            .parse()
            .map_err(|_| format!("invalid duration {orig:?}"))?;
        rest = &rest[num_end..];
        let unit_end = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        if unit_end == 0 {
            return Err(format!("missing unit in duration {orig:?}"));
        }
        let scale = match &rest[..unit_end] {
            "ns" => 1e-9,
            "us" | "\u{b5}s" | "\u{3bc}s" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            u => return Err(format!("unknown unit {u:?} in duration {orig:?}")),
        };
        total += value * scale;
        rest = &rest[unit_end..];
    }
    Ok(Duration::from_secs_f64(total))
}

/// The kind of value a [`FlagSpec`] takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlagKind {
    /// Free-form string.
    Str,
    /// Integer.
    Int,
    /// Go-style duration.
    Duration,
    /// Boolean (no value needed).
    Bool,
}

/// One declared flag (name without dashes, kind, default text, usage).
#[derive(Clone, Debug)]
pub struct FlagSpec {
    /// Flag name without dashes.
    pub name: &'static str,
    /// Value kind.
    pub kind: FlagKind,
    /// Default value as text.
    pub default: &'static str,
    /// Usage line.
    pub usage: &'static str,
}

/// Result of parsing: values by flag name plus remaining positional args.
#[derive(Debug, Default)]
pub struct Parsed {
    values: Vec<(&'static str, String)>,
    /// Non-flag arguments, in order.
    pub positional: Vec<String>,
    specs: Vec<FlagSpec>,
}

impl Parsed {
    fn raw(&self, name: &str) -> String {
        if let Some((_, v)) = self.values.iter().rev().find(|(n, _)| *n == name) {
            return v.clone();
        }
        self.specs
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.default.to_string())
            .unwrap_or_default()
    }
    /// String value of a flag (or its default).
    pub fn string(&self, name: &str) -> String {
        self.raw(name)
    }
    /// Integer value of a flag (validated during parsing).
    pub fn int(&self, name: &str) -> i64 {
        self.raw(name).parse().unwrap_or(0)
    }
    /// Bool value of a flag.
    pub fn bool(&self, name: &str) -> bool {
        self.raw(name) == "true"
    }
    /// Duration value of a flag.
    pub fn duration(&self, name: &str) -> Duration {
        parse_duration(&self.raw(name)).unwrap_or_default()
    }
}

/// Renders Go `flag.PrintDefaults`-style help text.
pub fn usage_text(prog: &str, specs: &[FlagSpec]) -> String {
    let mut out = format!("Usage of {prog}:\n");
    for s in specs {
        let ty = match s.kind {
            FlagKind::Str => " string",
            FlagKind::Int => " int",
            FlagKind::Duration => " duration",
            FlagKind::Bool => "",
        };
        out.push_str(&format!("  -{}{}\n    \t{} (default {})\n", s.name, ty, s.usage, s.default));
    }
    out
}

/// Parses `args` (without the program name) against `specs`. Parsing stops at
/// the first non-flag argument or `--`, like Go's `flag`; the rest is positional.
/// `-h`/`-help` yields `Err` with the usage text.
pub fn parse_flags(prog: &str, specs: &[FlagSpec], args: &[String]) -> Result<Parsed, String> {
    let mut parsed = Parsed { specs: specs.to_vec(), ..Default::default() };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            break;
        }
        let body = a.trim_start_matches('-');
        if body == "h" || body == "help" {
            return Err(usage_text(prog, specs));
        }
        let (name, inline) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (body, None),
        };
        let spec = specs
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("flag provided but not defined: -{name}\n{}", usage_text(prog, specs)))?;
        i += 1;
        let value = match (spec.kind, inline) {
            (FlagKind::Bool, None) => "true".to_string(),
            (_, Some(v)) => v,
            (_, None) => {
                let v = args
                    .get(i)
                    .ok_or_else(|| format!("flag needs an argument: -{name}\n{}", usage_text(prog, specs)))?
                    .clone();
                i += 1;
                v
            }
        };
        match spec.kind {
            FlagKind::Int => {
                value.parse::<i64>().map_err(|_| format!("invalid value {value:?} for flag -{name}: parse error"))?;
            }
            FlagKind::Duration => {
                parse_duration(&value).map_err(|e| format!("invalid value {value:?} for flag -{name}: {e}"))?;
            }
            FlagKind::Bool => {
                if value != "true" && value != "false" {
                    return Err(format!("invalid boolean value {value:?} for -{name}: parse error"));
                }
            }
            FlagKind::Str => {}
        }
        parsed.values.push((spec.name, value));
    }
    parsed.positional = args[i..].to_vec();
    Ok(parsed)
}

/// Go `flag.Parse` convention: print the error and exit (status 2; 0 for help).
pub fn parse_or_exit(prog: &str, specs: &[FlagSpec], args: &[String]) -> Parsed {
    match parse_flags(prog, specs, args) {
        Ok(p) => p,
        Err(msg) => {
            let help = msg.starts_with("Usage of");
            eprint!("{msg}");
            if !msg.ends_with('\n') {
                eprintln!();
            }
            std::process::exit(if help { 0 } else { 2 });
        }
    }
}

/// ModbusArgs holds common command-line arguments for Modbus clients.
#[derive(Clone, Debug)]
pub struct ModbusArgs {
    /// Modbus server IP address (`-ip`).
    pub ip: String,
    /// Modbus server port (`-port`).
    pub port: i64,
    /// Modbus unit ID / slave ID (`-unit`). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (Unit Identifier is one byte on MBAP).
    pub unit_id: i64,
    /// Timeout for Modbus operations (`-timeout`).
    pub timeout: Duration,
    /// Log level name as typed.
    pub log_level: String,
    /// Log level mapped from `log_level`.
    pub log_level_id: LogLevel,
}

/// Flag declarations for the common client arguments (Go `ParseArgs`).
pub fn common_specs() -> Vec<FlagSpec> {
    vec![
        FlagSpec { name: "ip", kind: FlagKind::Str, default: "127.0.0.1", usage: "Modbus server IP address" },
        FlagSpec { name: "port", kind: FlagKind::Int, default: "502", usage: "Modbus server port" },
        FlagSpec { name: "unit", kind: FlagKind::Int, default: "1", usage: "Modbus unit ID (slave ID)" },
        FlagSpec { name: "timeout", kind: FlagKind::Duration, default: "5s", usage: "Timeout for Modbus operations" },
        FlagSpec { name: "log", kind: FlagKind::Str, default: "info", usage: "Log level (debug, info, warn, error)" },
    ]
}

/// Maps a log level name to a [`LogLevel`]; unknown names print a notice and
/// fall back to info (Go behaviour).
pub fn map_log_level(name: &str) -> LogLevel {
    match name {
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => {
            println!("Invalid log level: {name}, using 'info'");
            LogLevel::Info
        }
    }
}

impl ModbusArgs {
    /// Builds from already parsed common flags.
    pub fn from_parsed(p: &Parsed) -> ModbusArgs {
        let log_level = p.string("log");
        ModbusArgs {
            ip: p.string("ip"),
            port: p.int("port"),
            unit_id: p.int("unit"),
            timeout: p.duration("timeout"),
            log_level_id: map_log_level(&log_level),
            log_level,
        }
    }

    /// Parses `args` (without the program name) or exits like Go's `flag.Parse`.
    pub fn parse(prog: &str, args: &[String]) -> ModbusArgs {
        ModbusArgs::from_parsed(&parse_or_exit(prog, &common_specs(), args))
    }

    /// Parses the process arguments (Go `ParseArgs`).
    pub fn parse_args() -> ModbusArgs {
        let argv: Vec<String> = std::env::args().collect();
        ModbusArgs::parse(&argv[0], &argv[1..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("5s").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("1.5h").unwrap(), Duration::from_secs(5400));
        assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
        assert!(parse_duration("5").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("-1s").is_err());
        assert!(parse_duration("3x").is_err());
    }

    #[test]
    fn defaults() {
        let a = ModbusArgs::from_parsed(&parse_flags("t", &common_specs(), &[]).unwrap());
        assert_eq!(a.ip, "127.0.0.1");
        assert_eq!(a.port, 502);
        assert_eq!(a.unit_id, 1);
        assert_eq!(a.timeout, Duration::from_secs(5));
        assert_eq!(a.log_level_id, LogLevel::Info);
    }

    #[test]
    fn forms_and_positional() {
        let p = parse_flags(
            "t",
            &common_specs(),
            &s(&["-ip", "10.0.0.1", "--port=1502", "-unit=7", "--timeout", "2s", "-log", "debug", "rest", "-x"]),
        )
        .unwrap();
        let a = ModbusArgs::from_parsed(&p);
        assert_eq!((a.ip.as_str(), a.port, a.unit_id), ("10.0.0.1", 1502, 7));
        assert_eq!(a.timeout, Duration::from_secs(2));
        assert_eq!(a.log_level_id, LogLevel::Debug);
        assert_eq!(p.positional, s(&["rest", "-x"]));
    }

    #[test]
    fn errors() {
        assert!(parse_flags("t", &common_specs(), &s(&["-nope"])).is_err());
        assert!(parse_flags("t", &common_specs(), &s(&["-port"])).is_err());
        assert!(parse_flags("t", &common_specs(), &s(&["-port", "abc"])).is_err());
        assert!(parse_flags("t", &common_specs(), &s(&["-h"])).unwrap_err().starts_with("Usage of"));
    }

    #[test]
    fn bool_flags() {
        let specs = [FlagSpec { name: "debug", kind: FlagKind::Bool, default: "false", usage: "d" }];
        assert!(parse_flags("t", &specs, &s(&["-debug"])).unwrap().bool("debug"));
        assert!(!parse_flags("t", &specs, &s(&["-debug=false"])).unwrap().bool("debug"));
        assert!(!parse_flags("t", &specs, &[]).unwrap().bool("debug"));
    }
}
