//! Modbus TCP response (MBAP header + PDU). Port of `gomodbus/transport/response.go`.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP header + PDU = ADU)
//! and Section 7 (MODBUS Exception Responses).

use crate::common::{
    self, Error, ExceptionCode, FunctionCode, PDU, ProtocolId, Result, TCP_PROTOCOL_IDENTIFIER,
    TransactionId, UnitId, is_function_exception, new_modbus_error,
};

use super::request::{decode_adu, encode_adu};

/// Response implements the [`common::Response`] trait for Modbus TCP.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header + PDU)
#[derive(Debug, Clone)]
pub struct Response {
    /// MBAP Transaction Identifier (echoed from the request). Ref: Section 4.1
    pub transaction_id: TransactionId,
    /// MBAP Protocol Identifier, 0 for MODBUS. Ref: Section 4.1
    pub protocol_id: ProtocolId,
    /// MBAP Unit Identifier. Ref: Section 4.1
    pub unit_id: UnitId,
    /// Protocol Data Unit. Ref: Section 4.1
    pub pdu: PDU,
}

impl Response {
    /// Creates a new Response.
    ///
    /// Ref: Section 4.1 (Protocol Identifier is 0 for MODBUS protocol)
    pub fn new(
        transaction_id: TransactionId,
        unit_id: UnitId,
        function_code: FunctionCode,
        data: Vec<u8>,
    ) -> Response {
        Response {
            transaction_id,
            protocol_id: TCP_PROTOCOL_IDENTIFIER,
            unit_id,
            pdu: PDU { function_code, data },
        }
    }

    /// Decodes a Response from bytes (MBAP header + PDU). A Length field below 2 is
    /// rejected with `Error::InvalidResponseLength`.
    ///
    /// Ref: Section 4.1 (MBAP Header) and Section 6 (PDU format)
    pub fn decode(&mut self, data: &[u8]) -> Result<()> {
        let adu = decode_adu(data, true)?;
        self.transaction_id = adu.transaction_id;
        self.protocol_id = adu.protocol_id;
        self.unit_id = adu.unit_id;
        self.pdu = adu.pdu;
        Ok(())
    }
}

impl common::Response for Response {
    fn get_transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    fn get_unit_id(&self) -> UnitId {
        self.unit_id
    }

    fn get_pdu(&self) -> &PDU {
        &self.pdu
    }

    /// Checks if the response is an exception (function code | 0x80).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
    fn is_exception(&self) -> bool {
        is_function_exception(self.pdu.function_code)
    }

    /// Returns the exception code if the response is an exception, otherwise 0.
    /// Ref: Section 7: the exception response data field is a single exception code byte.
    fn get_exception(&self) -> ExceptionCode {
        if self.is_exception() && !self.pdu.data.is_empty() {
            return ExceptionCode(self.pdu.data[0]);
        }
        ExceptionCode(0)
    }

    /// Converts an exception response to an error. Ref: Section 7 (Exception Responses)
    fn to_error(&self) -> Option<Error> {
        if self.is_exception() {
            return Some(new_modbus_error(self.pdu.function_code, self.get_exception()));
        }
        None
    }

    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header format)
    fn encode(&self) -> Result<Vec<u8>> {
        Ok(encode_adu(self.transaction_id, self.protocol_id, self.unit_id, &self.pdu))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Response as _;

    #[test]
    fn encode_decode_roundtrip() {
        let r = Response::new(TransactionId(5), UnitId(1), FunctionCode(0x03), vec![0x02, 0x00, 0x2A]);
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0, 5, 0, 0, 0, 5, 1, 3, 2, 0, 0x2A]);
        let mut d = Response::new(TransactionId(0), UnitId(0), FunctionCode(0), vec![]);
        d.decode(&bytes).unwrap();
        assert_eq!(d.pdu, r.pdu);
        assert_eq!(d.transaction_id, TransactionId(5));
        assert!(!d.is_exception());
        assert!(d.to_error().is_none());
        assert_eq!(d.get_exception(), ExceptionCode(0));
    }

    #[test]
    fn decode_errors() {
        let mut d = Response::new(TransactionId(0), UnitId(0), FunctionCode(0), vec![]);
        assert_eq!(d.decode(&[0; 3]), Err(Error::InvalidResponseLength));
        assert_eq!(d.decode(&[0, 1, 0, 0, 0, 1, 1, 3]), Err(Error::InvalidResponseLength));
        assert!(d.decode(&[0, 1, 0, 0, 0, 5, 1, 3, 0]).is_err());
        assert!(d.decode(&[0, 1, 0, 0, 0, 2, 1]).is_err());
    }

    #[test]
    fn exception_response() {
        // Ref: Section 7 example: function 0x01 -> 0x81, exception code 0x02
        let r = Response::new(TransactionId(1), UnitId(1), FunctionCode(0x81), vec![0x02]);
        assert!(r.is_exception());
        assert_eq!(r.get_exception(), ExceptionCode(2));
        let e = r.to_error().unwrap();
        assert!(common::is_modbus_error(&e));
        // exception function code without data byte
        let r = Response::new(TransactionId(1), UnitId(1), FunctionCode(0x83), vec![]);
        assert_eq!(r.get_exception(), ExceptionCode(0));
        assert!(r.to_error().is_some());
    }
}
