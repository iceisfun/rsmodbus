//! PDU structure. Port of `gomodbus/common/pdu.go`.

use super::types::FunctionCode;

/// PDU (Protocol Data Unit) is the core Modbus message structure.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (Protocol description)
/// A PDU consists of a Function Code followed by Function Code specific data;
/// its maximum size is 253 bytes ([`super::types::MAX_PDU_LENGTH`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PDU {
    /// 1 byte, Ref: Section 6 (MODBUS Function Codes)
    pub function_code: FunctionCode,
    /// Function-specific data (everything after the function code byte).
    pub data: Vec<u8>,
}

impl PDU {
    /// Creates a PDU from a function code and its data.
    pub fn new(function_code: FunctionCode, data: Vec<u8>) -> PDU {
        PDU { function_code, data }
    }
}
