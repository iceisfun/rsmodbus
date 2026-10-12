//! Error types. Port of `gomodbus/common/errors.go`.
//!
//! Go uses sentinel errors (`ErrTimeout`, ...), `fmt.Errorf("...: %w", err)`
//! wrapping and a `*ModbusError` for exception responses. Rust maps these to a
//! single [`Error`] enum:
//!
//! * every Go sentinel (`ErrFoo`) is an [`Error`] unit variant (`Error::Foo`),
//! * `*ModbusError` is [`Error::Modbus`],
//! * `fmt.Errorf("msg")` is [`Error::Other`] ([`Error::msg`]),
//! * `fmt.Errorf("ctx: %w", err)` is [`Error::Wrapped`] ([`Error::wrap`]),
//! * `errors.Is(err, ErrFoo)` is [`Error::is`] (walks `Wrapped` chains).

use std::fmt;

use super::types::*;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Crate error type (all Go `Err*` sentinels plus wrapping).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Codes) and
/// Section 7 (Exception Responses) for the protocol-level variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    // Client state errors (implementation detail, not spec)
    /// "client not connected"
    NotConnected,
    /// "client already connected"
    AlreadyConnected,

    // Protocol constraint errors (related to Modbus specification)
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Codes) - Various constraints
    /// "invalid quantity" - quantity constraints from spec.
    InvalidQuantity,
    /// "invalid address" - address range constraints from spec.
    InvalidAddress,

    // Protocol format errors
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4 (MODBUS Data Model)
    /// "invalid response length" - packet length issues.
    InvalidResponseLength,
    /// "invalid CRC" - for RTU mode.
    InvalidCrc,

    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (MODBUS Function Codes)
    /// "invalid function code" - unsupported function code.
    InvalidFunction,
    /// "invalid value"
    InvalidValue,
    /// "invalid response format"
    InvalidResponseFormat,

    // Communication errors (implementation detail, not spec)
    /// "timeout"
    Timeout,
    /// "context canceled"
    ContextCanceled,
    /// "context deadline exceeded" (Go's `context.DeadlineExceeded`).
    DeadlineExceeded,

    // Protocol header errors
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header)
    /// "invalid protocol header"
    InvalidProtocolHeader,

    // Request constraint errors
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (Write Multiple Registers)
    /// "too many registers requested"
    TooManyRegisters,
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (Write Multiple Coils)
    /// "too many coils requested"
    TooManyCoils,

    // Response errors
    /// "empty response"
    EmptyResponse,
    /// "response too large"
    ResponseTooLarge,
    /// "request too large"
    RequestTooLarge,

    // Transaction errors (implementation detail, not spec)
    /// "transaction timeout"
    TransactionTimeout,
    /// "transport closing"
    TransportClosing,

    // Server errors
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
    /// "server device failure" - related to exception code 0x04.
    ServerDeviceFailure,
    /// "no response from server"
    NoResponse,

    /// An exception response from the server (Go `*ModbusError`).
    Modbus(ModbusError),

    /// An I/O error (kind + message, kept `Clone`/`Eq`-friendly).
    Io {
        /// The `std::io::ErrorKind` of the original error.
        kind: std::io::ErrorKind,
        /// The original error's display text.
        message: String,
    },

    /// Ad-hoc error message (Go `fmt.Errorf` / `errors.New` without a sentinel).
    Other(String),

    /// Error with added context (Go `fmt.Errorf("context: %w", err)`).
    Wrapped {
        /// Context text printed before the source error.
        context: String,
        /// The wrapped error.
        source: Box<Error>,
    },
}

impl Error {
    /// Creates an ad-hoc error (Go `fmt.Errorf("...")`).
    pub fn msg(message: impl Into<String>) -> Error {
        Error::Other(message.into())
    }

    /// Wraps `source` with context text (Go `fmt.Errorf("context: %w", err)`).
    pub fn wrap(context: impl Into<String>, source: Error) -> Error {
        Error::Wrapped { context: context.into(), source: Box::new(source) }
    }

    /// Equivalent of Go's `errors.Is(self, target)`: true if `self` or any error
    /// in its `Wrapped` chain equals `target`.
    pub fn is(&self, target: &Error) -> bool {
        if self == target {
            return true;
        }
        match self {
            Error::Wrapped { source, .. } => source.is(target),
            _ => false,
        }
    }

    /// True for any timeout flavour (`Timeout`, `TransactionTimeout`,
    /// `DeadlineExceeded`, or an `Io` error of kind `TimedOut`/`WouldBlock`),
    /// looking through `Wrapped` chains.
    pub fn is_timeout(&self) -> bool {
        match self {
            Error::Timeout | Error::TransactionTimeout | Error::DeadlineExceeded => true,
            Error::Io { kind, .. } => {
                matches!(kind, std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)
            }
            Error::Wrapped { source, .. } => source.is_timeout(),
            _ => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotConnected => f.write_str("client not connected"),
            Error::AlreadyConnected => f.write_str("client already connected"),
            Error::InvalidQuantity => f.write_str("invalid quantity"),
            Error::InvalidAddress => f.write_str("invalid address"),
            Error::InvalidResponseLength => f.write_str("invalid response length"),
            Error::InvalidCrc => f.write_str("invalid CRC"),
            Error::InvalidFunction => f.write_str("invalid function code"),
            Error::InvalidValue => f.write_str("invalid value"),
            Error::InvalidResponseFormat => f.write_str("invalid response format"),
            Error::Timeout => f.write_str("timeout"),
            Error::ContextCanceled => f.write_str("context canceled"),
            Error::DeadlineExceeded => f.write_str("context deadline exceeded"),
            Error::InvalidProtocolHeader => f.write_str("invalid protocol header"),
            Error::TooManyRegisters => f.write_str("too many registers requested"),
            Error::TooManyCoils => f.write_str("too many coils requested"),
            Error::EmptyResponse => f.write_str("empty response"),
            Error::ResponseTooLarge => f.write_str("response too large"),
            Error::RequestTooLarge => f.write_str("request too large"),
            Error::TransactionTimeout => f.write_str("transaction timeout"),
            Error::TransportClosing => f.write_str("transport closing"),
            Error::ServerDeviceFailure => f.write_str("server device failure"),
            Error::NoResponse => f.write_str("no response from server"),
            Error::Modbus(e) => fmt::Display::fmt(e, f),
            Error::Io { message, .. } => f.write_str(message),
            Error::Other(m) => f.write_str(m),
            Error::Wrapped { context, source } => write!(f, "{}: {}", context, source),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Modbus(e) => Some(e),
            Error::Wrapped { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io { kind: e.kind(), message: e.to_string() }
    }
}

impl From<ModbusError> for Error {
    fn from(e: ModbusError) -> Error {
        Error::Modbus(e)
    }
}

/// ModbusError represents an error from a Modbus exception response.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
/// "When a Client sends a request to a Server device, it expects a normal response. ..."
/// "If the Server returns an Exception Response, the Exception Code field contains
/// the reason why the Server is unable to process the requested function."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModbusError {
    /// Function code from the request (Go keeps whatever the caller passed; the
    /// exception bit may or may not be set).
    pub function_code: FunctionCode,
    /// Exception code indicating the error reason.
    pub exception_code: ExceptionCode,
}

impl ModbusError {
    /// Creates a new ModbusError (Go `NewModbusError`).
    pub fn new(function_code: FunctionCode, exception_code: ExceptionCode) -> ModbusError {
        ModbusError { function_code, exception_code }
    }
}

impl fmt::Display for ModbusError {
    // Message format identical to Go, except the exception code is printed as
    // its numeric value (Go's `%#x` on a Stringer printed hex of the name).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "modbus: exception response: function: {}, exception code: {:#x} ({})",
            self.function_code,
            self.exception_code.0,
            get_exception_string(self.exception_code)
        )
    }
}

impl std::error::Error for ModbusError {}

/// Creates a new [`ModbusError`] wrapped as an [`Error`] (Go `NewModbusError`
/// returning `*ModbusError` used as `error`).
pub fn new_modbus_error(function_code: FunctionCode, exception_code: ExceptionCode) -> Error {
    Error::Modbus(ModbusError::new(function_code, exception_code))
}

/// Checks if an error is a ModbusError (Go `IsModbusError`; direct type check only).
pub fn is_modbus_error(err: &Error) -> bool {
    matches!(err, Error::Modbus(_))
}

/// Checks if an error is a specific Modbus exception (Go `IsExceptionError`).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
pub fn is_exception_error(err: &Error, exception_code: ExceptionCode) -> bool {
    match err {
        Error::Modbus(e) => e.exception_code == exception_code,
        _ => false,
    }
}

/// Checks if an error is due to a function not being supported (exception 0x01).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception code 01, Illegal Function)
pub fn is_function_not_supported_error(err: &Error) -> bool {
    is_exception_error(err, EXCEPTION_FUNCTION_CODE_NOT_SUPPORTED)
}

/// Returns a human-readable description of an exception code.
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
pub fn get_exception_string(exception_code: ExceptionCode) -> String {
    match exception_code {
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.1
        EXCEPTION_FUNCTION_CODE_NOT_SUPPORTED => "function code not supported".to_string(),
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.2
        EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE => "data address not available".to_string(),
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.3
        EXCEPTION_INVALID_DATA_VALUE => "invalid data value".to_string(),
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.4
        EXCEPTION_SERVER_DEVICE_FAILURE => "server device failure".to_string(),
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.5
        EXCEPTION_ACKNOWLEDGE => "acknowledge".to_string(),
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.6
        EXCEPTION_SERVER_DEVICE_BUSY => "server device busy".to_string(),
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.8
        EXCEPTION_MEMORY_PARITY_ERROR => "memory parity error".to_string(),
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.9
        EXCEPTION_GATEWAY_PATH_UNAVAILABLE => "gateway path unavailable".to_string(),
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7.10
        EXCEPTION_GATEWAY_TARGET_NO_RESPONSE => "gateway target no response".to_string(),
        other => format!("unknown exception code: {:#x}", other.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modbus_error_message() {
        let e = ModbusError::new(FunctionCode(0x83), EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE);
        assert_eq!(
            e.to_string(),
            "modbus: exception response: function: Exception(ReadHoldingRegisters), exception code: 0x2 (data address not available)"
        );
    }

    #[test]
    fn exception_helpers() {
        let err = new_modbus_error(FUNC_READ_COILS, EXCEPTION_FUNCTION_CODE_NOT_SUPPORTED);
        assert!(is_modbus_error(&err));
        assert!(is_function_not_supported_error(&err));
        assert!(!is_exception_error(&err, EXCEPTION_ACKNOWLEDGE));
        assert!(!is_modbus_error(&Error::Timeout));
        assert!(!is_function_not_supported_error(&Error::Timeout));
        assert_eq!(get_exception_string(ExceptionCode(0xFF)), "unknown exception code: 0xff");
    }

    #[test]
    fn wrapping_and_is() {
        let e = Error::wrap("failed to bind", Error::NotConnected);
        assert_eq!(e.to_string(), "failed to bind: client not connected");
        assert!(e.is(&Error::NotConnected));
        assert!(!e.is(&Error::Timeout));
        assert!(Error::wrap("x", Error::TransactionTimeout).is_timeout());
    }

    #[test]
    fn sentinel_messages() {
        assert_eq!(Error::NotConnected.to_string(), "client not connected");
        assert_eq!(Error::NoResponse.to_string(), "no response from server");
        assert_eq!(Error::TooManyRegisters.to_string(), "too many registers requested");
        assert_eq!(Error::DeadlineExceeded.to_string(), "context deadline exceeded");
    }
}
