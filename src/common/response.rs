//! Response interface. Port of `gomodbus/common/response.go`.

use super::errors::{Error, Result};
use super::pdu::PDU;
use super::types::{ExceptionCode, TransactionId, UnitId};

/// Response represents a Modbus response (Go `Response` interface).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP header + PDU = ADU)
/// and Section 7 (Exception Responses).
pub trait Response: Send + Sync {
    /// Returns the transaction ID.
    fn get_transaction_id(&self) -> TransactionId;
    /// Returns the unit ID.
    fn get_unit_id(&self) -> UnitId;
    /// Returns the PDU.
    fn get_pdu(&self) -> &PDU;
    /// Checks if the response is an exception (function code has bit 7 set).
    /// Ref: Section 7 (Exception Responses)
    fn is_exception(&self) -> bool;
    /// Returns the exception code if the response is an exception.
    fn get_exception(&self) -> ExceptionCode;
    /// Converts an exception response to an error (`None` when not an exception;
    /// Go returns a nil `error`).
    fn to_error(&self) -> Option<Error>;
    /// Encodes the response into bytes (MBAP header + PDU).
    fn encode(&self) -> Result<Vec<u8>>;
}
