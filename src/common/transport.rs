//! Transport interface. Port of `gomodbus/common/transport.go`.

use std::sync::Arc;

use super::context::Context;
use super::errors::Result;
use super::logger::LoggerInterface;
use super::request::Request;
use super::response::Response;

/// Transport is an abstraction for the underlying transport mechanism.
/// It provides a common interface for TCP, RTU, etc.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction)
/// (request/response exchange); the connection handling itself is an
/// implementation detail.
pub trait Transport: Send + Sync {
    /// Establishes a connection.
    fn connect(&self, ctx: &Context) -> Result<()>;
    /// Closes the connection.
    fn disconnect(&self, ctx: &Context) -> Result<()>;
    /// Returns true if connected.
    fn is_connected(&self) -> bool;
    /// Sends a request and returns the response. `request` is `&mut` because the
    /// transport assigns the transaction ID (`set_transaction_id`) before encoding.
    fn send(&self, ctx: &Context, request: &mut dyn Request) -> Result<Box<dyn Response>>;
    /// Sets the logger for the transport (Go `WithLogger`). Takes `Arc<Self>` so an
    /// implementation can either configure and return itself (`self`) or return a
    /// modified copy, matching the Go implementations.
    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Transport>;
}
