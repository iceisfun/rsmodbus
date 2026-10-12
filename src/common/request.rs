//! Request interface. Port of `gomodbus/common/request.go`.

use super::errors::Result;
use super::pdu::PDU;
use super::types::{TransactionId, UnitId};

/// Request represents a Modbus request (Go `Request` interface).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP header + PDU = ADU)
pub trait Request: Send + Sync {
    /// Returns the transaction ID. Ref: Section 4.1 (MBAP Header, Transaction Identifier)
    fn get_transaction_id(&self) -> TransactionId;
    /// Sets the transaction ID (done by the transport/transaction pool just before sending).
    fn set_transaction_id(&mut self, id: TransactionId);
    /// Returns the unit ID. Ref: Section 4.1 (MBAP Header, Unit Identifier)
    fn get_unit_id(&self) -> UnitId;
    /// Returns the PDU.
    fn get_pdu(&self) -> &PDU;
    /// Encodes the request into bytes (MBAP header + PDU).
    fn encode(&self) -> Result<Vec<u8>>;
}
