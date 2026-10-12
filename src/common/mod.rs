//! Shared types, errors, traits and utilities (port of Go package `common`).
//!
//! Everything is re-exported flat, so `common::Address`, `common::Error`,
//! `common::FUNC_READ_COILS`, `common::Client` etc. all work as `common.X` did in Go.
//!
//! Go -> Rust naming: types keep their name (`UnitID` -> `UnitId`), methods and
//! functions are snake_case (`GetPDU` -> `get_pdu`), constants SCREAMING_SNAKE
//! (`FuncReadCoils` -> `FUNC_READ_COILS`, `ErrTimeout` -> `Error::Timeout`).
pub mod types;
pub mod errors;
pub mod device;
pub mod util;
pub mod logger;
pub mod context;
pub mod pdu;
pub mod request;
pub mod response;
pub mod transport;
pub mod client;
pub mod server;
pub mod test_support;

pub use client::{Client, Protocol};
pub use context::Context;
pub use device::{DeviceIdObject, DeviceIdentification};
pub use errors::*;
pub use logger::*;
pub use pdu::PDU;
pub use request::Request;
pub use response::Response;
pub use server::*;
pub use transport::Transport;
pub use types::*;
pub use util::{find_free_port_tcp, format_bytes};
