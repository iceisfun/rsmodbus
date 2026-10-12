//! Server and DataStore interfaces. Port of `gomodbus/common/server.go`.

use std::sync::Arc;

use super::context::Context;
use super::errors::{Error, ModbusError, Result};
use super::logger::LoggerInterface;
use super::request::Request;
use super::response::Response;
use super::types::*;

/// HandlerFunc processes a Modbus request and returns a response (Go `HandlerFunc`).
/// Handlers are shared between connection threads, hence `Arc` + `Send + Sync`.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction)
pub type HandlerFunc = Arc<dyn Fn(&Context, &dyn Request) -> Result<Box<dyn Response>> + Send + Sync>;

/// Returns a "not implemented" error for any request (Go `DefaultHandlerFunc`).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (exception code 01, Illegal Function).
/// Note (as in Go): the function code in the error is the request's code, without the exception bit.
pub fn default_handler_func(_ctx: &Context, request: &dyn Request) -> Result<Box<dyn Response>> {
    Err(Error::Modbus(ModbusError::new(
        request.get_pdu().function_code,
        EXCEPTION_FUNCTION_CODE_NOT_SUPPORTED,
    )))
}

/// DataStore represents a Modbus data store with read/write capabilities.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (MODBUS Data model: coils,
/// discrete inputs, holding registers, input registers).
pub trait DataStore: Send + Sync {
    /// Reads coil values from the data store. Ref: Section 6.1
    fn read_coils(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<CoilValue>>;

    /// Reads discrete input values from the data store. Ref: Section 6.2
    fn read_discrete_inputs(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<DiscreteInputValue>>;

    /// Reads holding register values from the data store. Ref: Section 6.3
    fn read_holding_registers(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<RegisterValue>>;

    /// Reads input register values from the data store. Ref: Section 6.4
    fn read_input_registers(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<InputRegisterValue>>;

    /// Writes a single coil value to the data store. Ref: Section 6.5
    fn write_single_coil(&self, ctx: &Context, address: Address, value: CoilValue) -> Result<()>;

    /// Writes a single register value to the data store. Ref: Section 6.6
    fn write_single_register(&self, ctx: &Context, address: Address, value: RegisterValue) -> Result<()>;

    /// Writes multiple coil values to the data store. Ref: Section 6.11
    fn write_multiple_coils(&self, ctx: &Context, address: Address, values: &[CoilValue]) -> Result<()>;

    /// Writes multiple register values to the data store. Ref: Section 6.12
    fn write_multiple_registers(&self, ctx: &Context, address: Address, values: &[RegisterValue]) -> Result<()>;
}

/// Server is the interface that all Modbus servers must implement.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction)
pub trait Server: Send + Sync {
    /// Starts the server.
    fn start(&self, ctx: &Context) -> Result<()>;

    /// Stops the server.
    fn stop(&self, ctx: &Context) -> Result<()>;

    /// Returns true if the server is running.
    fn is_running(&self) -> bool;

    /// Sets the handler for a specific Modbus function code.
    /// Ref: Section 6 (Function Codes)
    fn set_handler(&self, function_code: FunctionCode, handler: HandlerFunc);

    /// Sets the logger for the server (Go `WithLogger`; implementations normally
    /// configure and return `self`).
    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Server>;

    /// Sets the data store for the server (Go `WithDataStore`).
    fn with_data_store(self: Arc<Self>, data_store: Arc<dyn DataStore>) -> Arc<dyn Server>;
}

/// ServerOption configures a server (Go `ServerOption func(Server)`). It receives
/// the server handle and returns the (possibly new) handle.
pub type ServerOption = Box<dyn FnOnce(Arc<dyn Server>) -> Arc<dyn Server>>;

/// Sets the logger for the server (Go `WithServerLogger`).
pub fn with_server_logger(logger: Arc<dyn LoggerInterface>) -> ServerOption {
    Box::new(move |s| s.with_logger(logger))
}

/// Sets the data store for the server (Go `WithServerDataStore`).
pub fn with_server_data_store(data_store: Arc<dyn DataStore>) -> ServerOption {
    Box::new(move |s| s.with_data_store(data_store))
}
