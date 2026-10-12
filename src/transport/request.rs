//! Modbus TCP request (MBAP header + PDU). Port of `gomodbus/transport/request.go`.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP header + PDU = ADU)
//! Note on spec refs: Section 4.1 of Modbus_Application_Protocol_V1_1b3.pdf only gives the ADU size
//! (253-byte PDU + 7-byte MBAP = 260 bytes). The MBAP field layout (Transaction Id, Protocol Id,
//! Length, Unit Id) is defined in the MODBUS Messaging on TCP/IP Implementation Guide, which is not
//! bundled; "Section 4.1 (MBAP ...)" refs below point at the ADU size paragraph plus that guide.

use std::io;
use std::time::{Duration, Instant};

use crate::common::{
    self, Error, FunctionCode, MAX_PDU_LENGTH, PDU, ProtocolId, Result, TCP_HEADER_LENGTH, TCP_PROTOCOL_IDENTIFIER,
    TransactionId, UnitId,
};

/// Builds the "EOF" / "unexpected EOF" error Go's `io.ReadFull`/`binary.Read` return when
/// a buffer is too short (implementation detail, not spec).
pub(crate) fn eof_error(read_any: bool) -> Error {
    if read_any {
        Error::from(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF"))
    } else {
        Error::from(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"))
    }
}

/// Encodes an ADU: MBAP header followed by the PDU.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header format).
pub(crate) fn encode_adu(
    transaction_id: TransactionId,
    protocol_id: ProtocolId,
    unit_id: UnitId,
    pdu: &PDU,
) -> Result<Vec<u8>> {
    // Ref: Section 4.1 (max PDU 253 bytes / TCP ADU 260 bytes): function code (1) + data <= 253.
    if 1 + pdu.data.len() > MAX_PDU_LENGTH {
        return Err(Error::RequestTooLarge);
    }
    // Ref: Section 4.1: Length field = Unit ID (1 byte) + Function Code (1 byte) + Data (N bytes)
    // (a u16 on the wire; cannot overflow after the size check above).
    let length = (1 + 1 + pdu.data.len()) as u16;

    let mut buffer = Vec::with_capacity(TCP_HEADER_LENGTH + 1 + pdu.data.len());
    // MBAP header - all multi-byte values use big-endian byte order.
    // Ref: Section 4.1, MBAP Header (Transaction Identifier, Protocol Identifier, Length, Unit Identifier)
    // Ref: Section 4.2 (Data Encoding): "MODBUS uses a 'big-Endian' representation for addresses
    // and data items ... the most significant byte is sent first."
    buffer.extend_from_slice(&transaction_id.0.to_be_bytes()); // Transaction Identifier, 2 bytes
    buffer.extend_from_slice(&protocol_id.0.to_be_bytes()); // Protocol Identifier, 2 bytes (0 = MODBUS)
    buffer.extend_from_slice(&length.to_be_bytes()); // Length, 2 bytes
    buffer.push(unit_id.0); // Unit Identifier, 1 byte
    // PDU. Ref: Section 4.1 (Function Code + Data)
    buffer.push(pdu.function_code.0);
    buffer.extend_from_slice(&pdu.data);
    Ok(buffer)
}

/// Decoded MBAP header + PDU of an ADU (shared by request and response decoding).
pub(crate) struct DecodedAdu {
    pub transaction_id: TransactionId,
    pub protocol_id: ProtocolId,
    pub unit_id: UnitId,
    pub pdu: PDU,
}

/// Decodes an ADU. `strict_length` mirrors Go's `Response.Decode`, which rejects a Length
/// field below 2 with `ErrInvalidResponseLength`; `Request.Decode` instead lets the
/// subtraction wrap and fails on the short read.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header) and Section 6 (PDU format)
pub(crate) fn decode_adu(data: &[u8], strict_length: bool) -> Result<DecodedAdu> {
    if data.len() < TCP_HEADER_LENGTH {
        return Err(Error::InvalidResponseLength);
    }

    // Ref: Section 4.1 MBAP Header, field 1: Transaction Identifier (2 bytes)
    let transaction_id = TransactionId(u16::from_be_bytes([data[0], data[1]]));
    // Field 2: Protocol Identifier (2 bytes)
    let protocol_id = ProtocolId(u16::from_be_bytes([data[2], data[3]]));
    // Field 3: Length (2 bytes)
    let length = u16::from_be_bytes([data[4], data[5]]);
    // Field 4: Unit Identifier (1 byte)
    let unit_id = UnitId(data[6]);

    // PDU - Function Code (1 byte). Ref: Section 6 (MODBUS Function Codes)
    let Some(&function_code) = data.get(TCP_HEADER_LENGTH) else {
        return Err(eof_error(false));
    };

    // PDU - Data (variable). The Length field counts Unit ID (1) and Function Code (1).
    let pdu_data_length = if strict_length {
        let n = length as i32 - 2;
        if n < 0 {
            return Err(Error::InvalidResponseLength);
        }
        n as usize
    } else {
        length.wrapping_sub(2) as usize
    };

    let rest = &data[TCP_HEADER_LENGTH + 1..];
    if rest.len() < pdu_data_length {
        return Err(eof_error(!rest.is_empty()));
    }

    Ok(DecodedAdu {
        transaction_id,
        protocol_id,
        unit_id,
        pdu: PDU { function_code: FunctionCode(function_code), data: rest[..pdu_data_length].to_vec() },
    })
}

/// Request implements the [`common::Request`] trait for Modbus TCP.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header + PDU)
#[derive(Debug, Clone)]
pub struct Request {
    /// MBAP Transaction Identifier. Ref: Section 4.1
    pub transaction_id: TransactionId,
    /// MBAP Protocol Identifier, 0 for MODBUS. Ref: Section 4.1
    pub protocol_id: ProtocolId,
    /// MBAP Unit Identifier. Ref: Section 4.1
    pub unit_id: UnitId,
    /// Protocol Data Unit. Ref: Section 4.1
    pub pdu: PDU,
    /// Creation time, used for lifetime/timeout detection (implementation detail, not spec).
    pub create: Instant,
}

impl Request {
    /// Creates a new Request (transaction ID 0 until a pool assigns one).
    ///
    /// Ref: Section 4.1 (Protocol Identifier is 0 for MODBUS protocol)
    pub fn new(unit_id: UnitId, function_code: FunctionCode, data: Vec<u8>) -> Request {
        Request {
            transaction_id: TransactionId(0),
            protocol_id: TCP_PROTOCOL_IDENTIFIER,
            unit_id,
            pdu: PDU { function_code, data },
            create: Instant::now(),
        }
    }

    /// Decodes a Request from bytes (MBAP header + PDU).
    ///
    /// Ref: Section 4.1 (MBAP Header) and Section 6 (PDU format)
    pub fn decode(&mut self, data: &[u8]) -> Result<()> {
        let adu = decode_adu(data, false)?;
        self.transaction_id = adu.transaction_id;
        self.protocol_id = adu.protocol_id;
        self.unit_id = adu.unit_id;
        self.pdu = adu.pdu;
        Ok(())
    }

    /// Returns how long ago the request was created (implementation detail, not spec).
    pub fn get_lifetime(&self) -> Duration {
        self.create.elapsed()
    }

    /// Called when a transaction is cancelled; a hook for cleanup, a no-op here
    /// (implementation detail, not spec).
    pub fn cancel(&self, _err: &Error) {}
}

impl common::Request for Request {
    fn get_transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    fn set_transaction_id(&mut self, id: TransactionId) {
        self.transaction_id = id;
    }

    fn get_unit_id(&self) -> UnitId {
        self.unit_id
    }

    fn get_pdu(&self) -> &PDU {
        &self.pdu
    }

    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header format)
    fn encode(&self) -> Result<Vec<u8>> {
        encode_adu(self.transaction_id, self.protocol_id, self.unit_id, &self.pdu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Request as _;

    #[test]
    fn encode_read_holding_registers_example() {
        // Ref: Section 6.3 example, wrapped in an MBAP header (Section 4.1)
        let mut r = Request::new(UnitId(0x11), FunctionCode(0x03), vec![0x00, 0x6B, 0x00, 0x03]);
        r.set_transaction_id(TransactionId(0x0102));
        assert_eq!(
            r.encode().unwrap(),
            vec![0x01, 0x02, 0x00, 0x00, 0x00, 0x06, 0x11, 0x03, 0x00, 0x6B, 0x00, 0x03]
        );
    }

    #[test]
    fn decode_roundtrip() {
        let mut r = Request::new(UnitId(7), FunctionCode(0x10), vec![1, 2, 3]);
        r.set_transaction_id(TransactionId(0xABCD));
        let bytes = r.encode().unwrap();
        let mut d = Request::new(UnitId(0), FunctionCode(0), vec![]);
        d.decode(&bytes).unwrap();
        assert_eq!(d.transaction_id, TransactionId(0xABCD));
        assert_eq!(d.protocol_id, TCP_PROTOCOL_IDENTIFIER);
        assert_eq!(d.unit_id, UnitId(7));
        assert_eq!(d.pdu, r.pdu);
    }

    #[test]
    fn decode_errors() {
        let mut d = Request::new(UnitId(0), FunctionCode(0), vec![]);
        assert_eq!(d.decode(&[0; 6]), Err(Error::InvalidResponseLength));
        // header only, no function code
        assert!(d.decode(&[0, 1, 0, 0, 0, 2, 1]).is_err());
        // length says 4 data bytes but only 1 present
        assert!(d.decode(&[0, 1, 0, 0, 0, 6, 1, 3, 0]).is_err());
        // length 1 wraps (Go behaviour) and fails on the short read
        assert!(d.decode(&[0, 1, 0, 0, 0, 1, 1, 3]).is_err());
    }

    #[test]
    fn lifetime_is_monotonic() {
        let r = Request::new(UnitId(1), FunctionCode(3), vec![]);
        std::thread::sleep(Duration::from_millis(5));
        assert!(r.get_lifetime() >= Duration::from_millis(5));
    }
}
