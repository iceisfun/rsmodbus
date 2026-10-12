//! Modbus TCP transport (port of Go package `transport`).
//!
//! - [`tcp_transport::TCPTransport`]: client transport implementing `common::Transport`.
//! - [`transaction_pool::TransactionPool`] / [`transaction::Transaction`]: Transaction Identifier
//!   allocation and request/response pairing (Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1).
//! - [`request::Request`] / [`response::Response`]: MBAP-framed ADUs implementing
//!   `common::Request` / `common::Response`.
pub mod tcp_transport;
pub mod transaction;
pub mod transaction_pool;
pub mod request;
pub mod response;

pub use request::Request;
pub use response::Response;
pub use tcp_transport::{TCPTransport, TCPTransportOption};
pub use transaction::Transaction;
pub use transaction_pool::{TransactionPool, TransactionPoolOption};
