//! Server-side request processing. Port of `gomodbus/server/protocol_handler.go`.
//!
//! Each `handle_*` method parses a request PDU, validates it, calls the
//! [`DataStore`] and builds the response PDU. Validation failures map to
//! Modbus exception codes (returned as `Error::Modbus`) which the TCP server
//! turns into exception responses.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (MODBUS Function Codes)
//! and Section 7 (MODBUS Exception Responses)

use std::time::Instant;

use crate::common::*;

/// Request implementation used by the TCP server for decoded MBAP frames
/// (Go uses `transport.NewRequest`; kept local so `server` does not depend on
/// the transport module).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP header + PDU = ADU)
#[derive(Debug, Clone)]
pub struct ServerRequest {
    /// MBAP Transaction Identifier. Ref: Section 4.1 (MBAP Header)
    pub transaction_id: TransactionId,
    /// MBAP Protocol Identifier (0 = MODBUS). Ref: Section 4.1 (MBAP Header)
    pub protocol_id: ProtocolId,
    /// MBAP Unit Identifier. Ref: Section 4.1 (MBAP Header)
    pub unit_id: UnitId,
    /// Function code + data. Ref: Section 4.1 (PDU)
    pub pdu: PDU,
    /// Time the request object was created (implementation detail, not spec).
    pub create: Instant,
}

impl ServerRequest {
    /// Creates a new request (Go `transport.NewRequest`).
    pub fn new(unit_id: UnitId, function_code: FunctionCode, data: Vec<u8>) -> ServerRequest {
        ServerRequest {
            transaction_id: TransactionId(0),
            protocol_id: TCP_PROTOCOL_IDENTIFIER,
            unit_id,
            pdu: PDU::new(function_code, data),
            create: Instant::now(),
        }
    }
}

impl Request for ServerRequest {
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
        Ok(encode_adu(self.transaction_id, self.protocol_id, self.unit_id, &self.pdu))
    }
}

/// Response implementation produced by the server handlers (Go `transport.NewResponse`).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP header + PDU = ADU)
#[derive(Debug, Clone)]
pub struct ServerResponse {
    /// MBAP Transaction Identifier (echoed from the request). Ref: Section 4.1
    pub transaction_id: TransactionId,
    /// MBAP Protocol Identifier (0 = MODBUS). Ref: Section 4.1
    pub protocol_id: ProtocolId,
    /// MBAP Unit Identifier (echoed from the request). Ref: Section 4.1
    pub unit_id: UnitId,
    /// Function code + data. Ref: Section 4.1 (PDU)
    pub pdu: PDU,
}

impl ServerResponse {
    /// Creates a new response (Go `transport.NewResponse`).
    pub fn new(transaction_id: TransactionId, unit_id: UnitId, function_code: FunctionCode, data: Vec<u8>) -> ServerResponse {
        ServerResponse {
            transaction_id,
            protocol_id: TCP_PROTOCOL_IDENTIFIER,
            unit_id,
            pdu: PDU::new(function_code, data),
        }
    }
}

impl Response for ServerResponse {
    fn get_transaction_id(&self) -> TransactionId {
        self.transaction_id
    }
    fn get_unit_id(&self) -> UnitId {
        self.unit_id
    }
    fn get_pdu(&self) -> &PDU {
        &self.pdu
    }
    /// Exception when the function code has bit 7 set.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (MODBUS Exception Responses)
    fn is_exception(&self) -> bool {
        is_function_exception(self.pdu.function_code)
    }
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception code field)
    fn get_exception(&self) -> ExceptionCode {
        if self.is_exception() && !self.pdu.data.is_empty() {
            return ExceptionCode(self.pdu.data[0]);
        }
        ExceptionCode(0)
    }
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

// Encodes MBAP header + PDU, big-endian.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header, Table 3) and Section 4.2 (Data Encoding)
// Length field = Unit ID (1) + Function Code (1) + Data (N).
fn encode_adu(tid: TransactionId, pid: ProtocolId, unit: UnitId, pdu: &PDU) -> Vec<u8> {
    let length = (1 + 1 + pdu.data.len()) as u16;
    let mut out = Vec::with_capacity(TCP_HEADER_LENGTH + 1 + pdu.data.len());
    out.extend_from_slice(&tid.0.to_be_bytes());
    out.extend_from_slice(&pid.0.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    out.push(unit.0);
    out.push(pdu.function_code.0);
    out.extend_from_slice(&pdu.data);
    out
}

/// ServerProtocolHandler processes Modbus requests and generates responses
/// (Go `serverProtocolHandler`).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (MODBUS Function Codes)
#[derive(Debug, Clone, Copy, Default)]
pub struct ServerProtocolHandler;

// Builds the exception error for a request (Ref: Section 7, Exception Responses).
fn exc(req: &dyn Request, code: ExceptionCode) -> Error {
    new_modbus_error(req.get_pdu().function_code, code)
}

// Maps a data store error to an exception: ErrInvalidQuantity -> Illegal Data Value (03),
// anything else -> Server Device Failure (04).
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception codes 03 and 04)
fn store_exc(req: &dyn Request, err: &Error) -> Error {
    if *err == Error::InvalidQuantity {
        return exc(req, EXCEPTION_INVALID_DATA_VALUE);
    }
    exc(req, EXCEPTION_SERVER_DEVICE_FAILURE)
}

// Builds a response echoing the request's transaction / unit / function code.
fn respond(req: &dyn Request, data: Vec<u8>) -> Result<Box<dyn Response>> {
    Ok(Box::new(ServerResponse::new(
        req.get_transaction_id(),
        req.get_unit_id(),
        req.get_pdu().function_code,
        data,
    )))
}

// Big-endian u16 at `off`. Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.2 (Data Encoding)
fn be16(data: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([data[off], data[off + 1]])
}

// Packs a list of u16 as big-endian bytes.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.2 (Data Encoding)
// "MODBUS uses a 'big-Endian' representation for addresses and data items."
fn pack_registers(values: &[u16]) -> Vec<u8> {
    let byte_count = values.len() * 2;
    let mut out = Vec::with_capacity(1 + byte_count);
    out.push(byte_count as u8); // First byte is the byte count
    for v in values {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out
}

// Decodes `quantity` big-endian registers starting at `data[off..]`.
fn unpack_registers(data: &[u8], off: usize, quantity: u16) -> Vec<RegisterValue> {
    (0..quantity as usize).map(|i| be16(data, off + i * 2)).collect()
}

impl ServerProtocolHandler {
    /// Creates a new protocol handler for the server (Go `newServerProtocolHandler`).
    pub fn new() -> ServerProtocolHandler {
        ServerProtocolHandler
    }

    // Shared by Read Coils (0x01) and Read Discrete Inputs (0x02).
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Sections 6.1 and 6.2 (Read Coils/Discrete Inputs)
    fn handle_read_bit_values(
        &self,
        ctx: &Context,
        req: &dyn Request,
        max_quantity: u16,
        read_func: impl FnOnce(&Context, Address, Quantity) -> Result<Vec<bool>>,
    ) -> Result<Box<dyn Response>> {
        // Parse request PDU data
        // Ref: Section 6.1/6.2 (Request PDU): Starting Address (2 bytes), Quantity (2 bytes)
        let data = &req.get_pdu().data;
        if data.len() != 4 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - big-endian
        let address = Address(be16(data, 0));
        let quantity = Quantity(be16(data, 2));

        // Validate quantity (between 1 and max_quantity)
        // Ref: Section 6.1/6.2 (Quantity of coils/inputs 1 to 2000 (0x7D0))
        if quantity.0 == 0 || quantity.0 > max_quantity {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Read values from data store
        let values = read_func(ctx, address, quantity).map_err(|e| store_exc(req, &e))?;

        // Response PDU: Byte Count (1), Coil/Input Status (N bytes, N = ceil(quantity / 8))
        // Ref: Section 6.1/6.2 (Response PDU)
        let byte_count = (quantity.0 as usize).div_ceil(8);
        let mut response_data = vec![0u8; 1 + byte_count];
        response_data[0] = byte_count as u8;

        // "The coils in the response message are packed as one coil per bit of the data field.
        // Status is indicated as 1= ON and 0= OFF. The LSB of the first data byte contains the
        // output addressed in the query."
        // Ref: Section 6.1/6.2 (Response)
        for (i, value) in values.iter().enumerate() {
            if *value {
                response_data[1 + i / 8] |= 1 << (i % 8);
            }
        }

        respond(req, response_data)
    }

    // Shared by Read Holding Registers (0x03) and Read Input Registers (0x04).
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Sections 6.3 and 6.4 (Read Holding/Input Registers)
    fn handle_read_register_values(
        &self,
        ctx: &Context,
        req: &dyn Request,
        max_quantity: u16,
        read_func: impl FnOnce(&Context, Address, Quantity) -> Result<Vec<u16>>,
    ) -> Result<Box<dyn Response>> {
        // Ref: Section 6.3/6.4 (Request PDU): Starting Address (2 bytes), Quantity of Registers (2 bytes)
        let data = &req.get_pdu().data;
        if data.len() != 4 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - big-endian
        let address = Address(be16(data, 0));
        let quantity = Quantity(be16(data, 2));

        // Ref: Section 6.3/6.4 (Quantity of Registers 1 to 125 (0x7D))
        if quantity.0 == 0 || quantity.0 > max_quantity {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        let values = read_func(ctx, address, quantity).map_err(|e| store_exc(req, &e))?;

        // Response PDU: Byte Count (1) = 2 x N, Register Values (N x 2 bytes, big-endian)
        // Ref: Section 6.3/6.4 (Response PDU); Section 4.2 (Data Encoding)
        respond(req, pack_registers(&values))
    }

    /// Processes a Read Coils request (FC 0x01).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Read Coils)
    pub fn handle_read_coils(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        self.handle_read_bit_values(ctx, req, MAX_COIL_COUNT, |c, a, q| store.read_coils(c, a, q))
    }

    /// Processes a Read Discrete Inputs request (FC 0x02).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (Read Discrete Inputs)
    pub fn handle_read_discrete_inputs(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        self.handle_read_bit_values(ctx, req, MAX_COIL_COUNT, |c, a, q| store.read_discrete_inputs(c, a, q))
    }

    /// Processes a Read Holding Registers request (FC 0x03).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Read Holding Registers)
    pub fn handle_read_holding_registers(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        self.handle_read_register_values(ctx, req, MAX_REGISTER_COUNT, |c, a, q| store.read_holding_registers(c, a, q))
    }

    /// Processes a Read Input Registers request (FC 0x04).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Read Input Registers)
    pub fn handle_read_input_registers(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        self.handle_read_register_values(ctx, req, MAX_REGISTER_COUNT, |c, a, q| store.read_input_registers(c, a, q))
    }

    /// Processes a Write Single Coil request (FC 0x05).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5 (Write Single Coil)
    pub fn handle_write_single_coil(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        // Request PDU: Output Address (2 bytes), Output Value (2 bytes: 0xFF00 ON, 0x0000 OFF)
        // Ref: Section 6.5 (Request)
        let data = &req.get_pdu().data;
        if data.len() != 4 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - big-endian
        let address = Address(be16(data, 0));
        let value = be16(data, 2);

        // "A value of FF00 hex requests the output to be ON. A value of 0000 requests it to be OFF.
        // All other values are illegal and will not affect the output."
        // Ref: Section 6.5 (Request)
        let coil_value = if value == COIL_ON_U16 {
            true
        } else if value == COIL_OFF_U16 {
            false
        } else {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        };

        // Any store failure -> Server Device Failure (04). Ref: Section 7
        store
            .write_single_coil(ctx, address, coil_value)
            .map_err(|_| exc(req, EXCEPTION_SERVER_DEVICE_FAILURE))?;

        // "The normal response is an echo of the request, returned after the coil state has been written."
        // Ref: Section 6.5 (Response)
        respond(req, data.clone())
    }

    /// Processes a Write Single Register request (FC 0x06).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.6 (Write Single Register)
    pub fn handle_write_single_register(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        // Request PDU: Register Address (2 bytes), Register Value (2 bytes). Ref: Section 6.6 (Request)
        let data = &req.get_pdu().data;
        if data.len() != 4 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - big-endian
        let address = Address(be16(data, 0));
        let value: RegisterValue = be16(data, 2);

        store
            .write_single_register(ctx, address, value)
            .map_err(|_| exc(req, EXCEPTION_SERVER_DEVICE_FAILURE))?;

        // "The normal response is an echo of the request, returned after the register contents have been written."
        // Ref: Section 6.6 (Response)
        respond(req, data.clone())
    }

    /// Processes a Write Multiple Coils request (FC 0x0F).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (Write Multiple Coils)
    pub fn handle_write_multiple_coils(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        // Request PDU: Starting Address (2), Quantity of Outputs (2), Byte Count (1), Outputs Value (N bytes)
        // Ref: Section 6.11 (Request PDU)
        let data = &req.get_pdu().data;
        if data.len() < 5 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - big-endian
        let address = Address(be16(data, 0));
        let quantity = Quantity(be16(data, 2));
        let byte_count = data[4] as usize;

        // Validate data length against the Byte Count field. Ref: Section 6.11 (Request)
        if data.len() != 5 + byte_count {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // "Quantity of Outputs: 1 to 1968 (0x07B0)". Ref: Section 6.11 (Request)
        if quantity.0 == 0 || quantity.0 > MAX_WRITE_COIL_COUNT {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // "Byte Count = N, N = Quantity of Outputs / 8, if the remainder is different of 0 then N = N+1"
        // Ref: Section 6.11 (Request)
        let expected_byte_count = (quantity.0 as usize).div_ceil(8);
        if byte_count != expected_byte_count {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // "The outputs are packed one per bit of the data field. 1 = ON, 0 = OFF. The LSB of the
        // first data byte contains the output addressed in the request."
        // Ref: Section 6.11 (Request data encoding)
        let values: Vec<CoilValue> = (0..quantity.0 as usize)
            .map(|i| (data[5 + i / 8] >> (i % 8)) & 0x01 != 0)
            .collect();

        store
            .write_multiple_coils(ctx, address, &values)
            .map_err(|e| store_exc(req, &e))?;

        // Response PDU: Starting Address (2), Quantity of Outputs (2). Ref: Section 6.11 (Response)
        let mut response_data = Vec::with_capacity(4);
        response_data.extend_from_slice(&address.0.to_be_bytes());
        response_data.extend_from_slice(&quantity.0.to_be_bytes());
        respond(req, response_data)
    }

    /// Processes a Write Multiple Registers request (FC 0x10).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (Write Multiple Registers)
    pub fn handle_write_multiple_registers(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        // Request PDU: Starting Address (2), Quantity of Registers (2), Byte Count (1),
        // Registers Value (N x 2 bytes). Ref: Section 6.12 (Request PDU)
        let data = &req.get_pdu().data;
        if data.len() < 5 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - big-endian
        let address = Address(be16(data, 0));
        let quantity = Quantity(be16(data, 2));
        let byte_count = data[4] as usize;

        // Ref: Section 6.12 (Request) - data must match the Byte Count field
        if data.len() != 5 + byte_count {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // "Quantity of Registers: 1 to 123 (0x7B)". Ref: Section 6.12 (Request)
        if quantity.0 == 0 || quantity.0 > MAX_WRITE_REGISTER_COUNT {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // "Byte Count = 2 x N" (N = Quantity of Registers). Ref: Section 6.12 (Request)
        if byte_count != quantity.0 as usize * 2 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - registers are big-endian, high byte first
        let values = unpack_registers(data, 5, quantity.0);

        store
            .write_multiple_registers(ctx, address, &values)
            .map_err(|e| store_exc(req, &e))?;

        // Response PDU: Starting Address (2), Quantity of Registers (2). Ref: Section 6.12 (Response)
        let mut response_data = Vec::with_capacity(4);
        response_data.extend_from_slice(&address.0.to_be_bytes());
        response_data.extend_from_slice(&quantity.0.to_be_bytes());
        respond(req, response_data)
    }

    /// Processes a Read/Write Multiple Registers request (FC 0x17).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.17 (Read/Write Multiple registers)
    pub fn handle_read_write_multiple_registers(&self, ctx: &Context, req: &dyn Request, store: &dyn DataStore) -> Result<Box<dyn Response>> {
        // Request PDU: Read Starting Address (2), Quantity to Read (2), Write Starting Address (2),
        // Quantity to Write (2), Write Byte Count (1), Write Registers Value (N x 2 bytes)
        // Ref: Section 6.17 (Request PDU)
        let data = &req.get_pdu().data;
        if data.len() < 9 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - big-endian
        let read_address = Address(be16(data, 0));
        let read_quantity = Quantity(be16(data, 2));
        let write_address = Address(be16(data, 4));
        let write_quantity = Quantity(be16(data, 6));
        let byte_count = data[8] as usize;

        // Ref: Section 6.17 (Request) - data must match the Write Byte Count field
        if data.len() != 9 + byte_count {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // "Quantity to Read: 1 to 125 (0x7D)", "Quantity to Write: 1 to 121 (0x79)"
        // Ref: Section 6.17 (Request)
        if read_quantity.0 == 0
            || read_quantity.0 > MAX_READ_WRITE_READ_COUNT
            || write_quantity.0 == 0
            || write_quantity.0 > MAX_READ_WRITE_WRITE_COUNT
        {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // "Write Byte Count = 2 x N" (N = Quantity to Write). Ref: Section 6.17 (Request)
        if byte_count != write_quantity.0 as usize * 2 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        // Ref: Section 4.2 (Data Encoding) - registers are big-endian
        let write_values = unpack_registers(data, 9, write_quantity.0);

        store
            .write_multiple_registers(ctx, write_address, &write_values)
            .map_err(|e| store_exc(req, &e))?;

        // "The write operation is performed before the read." Ref: Section 6.17 (Description)
        let read_values = store
            .read_holding_registers(ctx, read_address, read_quantity)
            .map_err(|e| store_exc(req, &e))?;

        // Response PDU: Byte Count (1) = 2 x N, Read Registers Value (N x 2 bytes)
        // Ref: Section 6.17 (Response PDU)
        respond(req, pack_registers(&read_values))
    }

    /// Processes a Read Device Identification request (FC 0x2B / MEI type 0x0E).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Encapsulated Interface Transport,
    /// Read Device Identification)
    pub fn handle_read_device_identification(&self, _ctx: &Context, req: &dyn Request, _store: &dyn DataStore) -> Result<Box<dyn Response>> {
        // Request PDU: MEI Type (0x0E), Read Device ID code (0x01-0x04), Object Id
        // Ref: Section 6.21 (Request PDU)
        let data = &req.get_pdu().data;
        if data.len() < 3 {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        if MeiType(data[0]) != MEI_READ_DEVICE_ID {
            return Err(exc(req, EXCEPTION_INVALID_DATA_VALUE));
        }

        let read_device_id_code = ReadDeviceIdCode(data[1]);
        let object_id = DeviceIdObjectCode(data[2]);

        // Fixed identification for this server.
        // Ref: Section 6.21 (Conformity level; More Follows 0x00 = no; Next Object Id)
        let mut objects_to_include: Vec<DeviceIdObjectCode> = vec![
            DEVICE_ID_VENDOR_NAME,
            DEVICE_ID_PRODUCT_CODE,
            DEVICE_ID_MAJOR_MINOR_REVISION,
        ];

        // Ref: Section 6.21 (Read Device ID code: 01 basic stream, 02 regular stream,
        // 03 extended stream, 04 individual access)
        if read_device_id_code == READ_DEVICE_ID_SPECIFIC_OBJECT {
            objects_to_include = vec![object_id];
        } else if read_device_id_code == READ_DEVICE_ID_REGULAR_STREAM {
            // Regular objects too (0x03-0x06)
            objects_to_include.extend([
                DEVICE_ID_VENDOR_URL,
                DEVICE_ID_PRODUCT_NAME,
                DEVICE_ID_MODEL_NAME,
                DEVICE_ID_USER_APP_NAME,
            ]);
        } else if read_device_id_code == READ_DEVICE_ID_EXTENDED_STREAM {
            // All objects (basic + regular + extended 0x80-0xFF)
            objects_to_include.extend([
                DEVICE_ID_VENDOR_URL,
                DEVICE_ID_PRODUCT_NAME,
                DEVICE_ID_MODEL_NAME,
                DEVICE_ID_USER_APP_NAME,
                DeviceIdObjectCode(0x80), // Example extended object
            ]);
        }

        // Object values, Ref: Section 6.21 (Object Id table: 0x00-0x02 basic/mandatory,
        // 0x03-0x06 regular, 0x80-0xFF private extended)
        let object_value = |id: DeviceIdObjectCode| -> Option<&'static str> {
            match id {
                DEVICE_ID_VENDOR_NAME => Some("gomodbus"),
                DEVICE_ID_PRODUCT_CODE => Some("GM-001"),
                DEVICE_ID_MAJOR_MINOR_REVISION => Some("1.0"),
                DEVICE_ID_VENDOR_URL => Some("https://github.com/Moonlight-Companies/gomodbus"),
                DEVICE_ID_PRODUCT_NAME => Some("gomodbus Server"),
                DEVICE_ID_MODEL_NAME => Some("Modbus TCP Server"),
                DEVICE_ID_USER_APP_NAME => Some("Example Server"),
                DeviceIdObjectCode(0x80) => Some("Extended Object Example"),
                _ => None,
            }
        };

        let objects: Vec<DeviceIdObject> = objects_to_include
            .iter()
            .filter_map(|id| {
                object_value(*id).map(|v| DeviceIdObject { id: *id, length: v.len() as u8, value: v.to_string() })
            })
            .collect();

        // Response PDU (Ref: Section 6.21):
        //   MEI Type (0x0E), Read Device ID code, Conformity level, More Follows (0x00 / 0xFF),
        //   Next Object Id, Number of objects, then per object: Object Id, Object Length, Object Value
        let mut response_data = vec![
            MEI_READ_DEVICE_ID.0,
            read_device_id_code.0,
            CONFORMITY_LEVEL_BASIC.0,
            MORE_FOLLOWS_NO.0,
            0x00, // Next Object Id
            objects.len() as u8,
        ];
        for obj in &objects {
            response_data.push(obj.id.0);
            response_data.push(obj.length);
            response_data.extend_from_slice(obj.value.as_bytes());
        }

        respond(req, response_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_support::{MockDataStore, MockRequest};

    fn ctx() -> Context {
        Context::background()
    }

    fn mock_req(fc: FunctionCode, data: Vec<u8>) -> MockRequest {
        MockRequest::new(TransactionId(1), UnitId(1), fc, data)
    }

    #[test]
    fn handle_read_coils() {
        let handler = ServerProtocolHandler::new();
        let store = MockDataStore::new();
        store.set_coil(Address(100), true);
        store.set_coil(Address(101), false);
        store.set_coil(Address(102), true);

        let address = Address(100);
        let quantity = Quantity(3);
        let mut req_data = Vec::new();
        req_data.extend_from_slice(&address.0.to_be_bytes());
        req_data.extend_from_slice(&quantity.0.to_be_bytes());
        let req = mock_req(FUNC_READ_COILS, req_data);

        let resp = handler.handle_read_coils(&ctx(), &req, &store).expect("HandleReadCoils returned error");
        let resp_data = &resp.get_pdu().data;
        // byte count 1 for 3 coils
        assert_eq!(resp_data[0], 1);
        // bits 0 and 2 set: 0b00000101 = 5
        assert_eq!(resp_data[1], 5);

        // invalid request (wrong data length)
        let invalid = mock_req(FUNC_READ_COILS, vec![0x00, 0x64]);
        assert!(handler.handle_read_coils(&ctx(), &invalid, &store).is_err());

        // invalid quantity (0)
        let invalid_q = mock_req(FUNC_READ_COILS, vec![0x00, 0x64, 0x00, 0x00]);
        assert!(handler.handle_read_coils(&ctx(), &invalid_q, &store).is_err());

        // datastore failure
        store.set_fail_on_address(address);
        assert!(handler.handle_read_coils(&ctx(), &req, &store).is_err());
        store.clear_fail_on_address();
    }

    #[test]
    fn handle_read_discrete_inputs() {
        let handler = ServerProtocolHandler::new();
        let store = MockDataStore::new();
        store.set_discrete_input(Address(100), true);
        store.set_discrete_input(Address(101), true);
        store.set_discrete_input(Address(102), false);

        let req = mock_req(FUNC_READ_DISCRETE_INPUTS, vec![0x00, 0x64, 0x00, 0x03]);
        let resp = handler.handle_read_discrete_inputs(&ctx(), &req, &store).expect("error");
        let resp_data = &resp.get_pdu().data;
        assert_eq!(resp_data[0], 1);
        // 0b00000011 = 3
        assert_eq!(resp_data[1], 3);
    }

    #[test]
    fn handle_read_holding_registers() {
        let handler = ServerProtocolHandler::new();
        let store = MockDataStore::new();
        store.set_holding_register(Address(100), 0x1234);
        store.set_holding_register(Address(101), 0x5678);

        let req = mock_req(FUNC_READ_HOLDING_REGISTERS, vec![0x00, 0x64, 0x00, 0x02]);
        let resp = handler.handle_read_holding_registers(&ctx(), &req, &store).expect("error");
        let resp_data = &resp.get_pdu().data;
        assert_eq!(resp_data[0], 4);
        assert_eq!(be16(resp_data, 1), 0x1234);
        assert_eq!(be16(resp_data, 3), 0x5678);
    }

    #[test]
    fn handle_write_single_coil() {
        let handler = ServerProtocolHandler::new();
        let store = MockDataStore::new();
        let address = Address(100);

        let mut req_data = Vec::new();
        req_data.extend_from_slice(&address.0.to_be_bytes());
        req_data.extend_from_slice(&COIL_ON_U16.to_be_bytes());
        let req = mock_req(FUNC_WRITE_SINGLE_COIL, req_data);

        let resp = handler.handle_write_single_coil(&ctx(), &req, &store).expect("error");
        let resp_data = &resp.get_pdu().data;
        // echo of the request
        assert_eq!(resp_data.len(), 4);
        assert_eq!(be16(resp_data, 0), address.0);
        assert_eq!(be16(resp_data, 2), COIL_ON_U16);

        // written to datastore
        assert_eq!(store.get_coil(address), Some(true));

        // invalid value
        let invalid = mock_req(FUNC_WRITE_SINGLE_COIL, vec![0x00, 0x64, 0x12, 0x34]);
        assert!(handler.handle_write_single_coil(&ctx(), &invalid, &store).is_err());
    }

    #[test]
    fn handle_write_multiple_registers() {
        let handler = ServerProtocolHandler::new();
        let store = MockDataStore::new();
        let address = Address(100);
        let quantity = Quantity(2);
        let values: [RegisterValue; 2] = [0x1234, 0x5678];

        let mut req_data = Vec::new();
        req_data.extend_from_slice(&address.0.to_be_bytes());
        req_data.extend_from_slice(&quantity.0.to_be_bytes());
        req_data.push(4);
        req_data.extend_from_slice(&values[0].to_be_bytes());
        req_data.extend_from_slice(&values[1].to_be_bytes());
        let req = mock_req(FUNC_WRITE_MULTIPLE_REGISTERS, req_data);

        let resp = handler.handle_write_multiple_registers(&ctx(), &req, &store).expect("error");
        let resp_data = &resp.get_pdu().data;
        assert_eq!(resp_data.len(), 4);
        assert_eq!(be16(resp_data, 0), address.0);
        assert_eq!(be16(resp_data, 2), quantity.0);

        for (i, expected) in values.iter().enumerate() {
            let addr = address + Address(i as u16);
            assert_eq!(store.get_holding_register(addr), Some(*expected), "address {}", addr.0);
        }

        // invalid quantity
        let invalid_q = mock_req(FUNC_WRITE_MULTIPLE_REGISTERS, vec![0x00, 0x64, 0x00, 0x00, 0x00]);
        assert!(handler.handle_write_multiple_registers(&ctx(), &invalid_q, &store).is_err());

        // mismatched byte count
        let mismatched = mock_req(
            FUNC_WRITE_MULTIPLE_REGISTERS,
            vec![0x00, 0x64, 0x00, 0x02, 0x03, 0x12, 0x34, 0x56],
        );
        assert!(handler.handle_write_multiple_registers(&ctx(), &mismatched, &store).is_err());
    }

    // Additional coverage beyond the Go tests: exception code mapping and remaining handlers.
    #[test]
    fn exception_codes_and_remaining_handlers() {
        let handler = ServerProtocolHandler::new();
        let store = MockDataStore::new();

        // bad quantity -> Illegal Data Value (03)
        let req = mock_req(FUNC_READ_HOLDING_REGISTERS, vec![0, 0, 0, 126]);
        let err = handler.handle_read_holding_registers(&ctx(), &req, &store).err().unwrap();
        assert!(is_exception_error(&err, EXCEPTION_INVALID_DATA_VALUE));

        // store failure -> Server Device Failure (04)
        store.set_fail(true);
        let req = mock_req(FUNC_WRITE_SINGLE_REGISTER, vec![0, 1, 0, 2]);
        let err = handler.handle_write_single_register(&ctx(), &req, &store).err().unwrap();
        assert!(is_exception_error(&err, EXCEPTION_SERVER_DEVICE_FAILURE));
        store.set_fail(false);

        // write single register echoes
        let resp = handler.handle_write_single_register(&ctx(), &req, &store).unwrap();
        assert_eq!(resp.get_pdu().data, vec![0, 1, 0, 2]);
        assert_eq!(store.get_holding_register(Address(1)), Some(2));

        // write multiple coils: 10 coils = 0x01 0x02 -> coils 0 and 9 set
        let req = mock_req(FUNC_WRITE_MULTIPLE_COILS, vec![0, 0, 0, 10, 2, 0x01, 0x02]);
        let resp = handler.handle_write_multiple_coils(&ctx(), &req, &store).unwrap();
        assert_eq!(resp.get_pdu().data, vec![0, 0, 0, 10]);
        assert_eq!(store.get_coil(Address(0)), Some(true));
        assert_eq!(store.get_coil(Address(1)), Some(false));
        assert_eq!(store.get_coil(Address(9)), Some(true));

        // read/write multiple registers: write happens before read
        let req = mock_req(
            FUNC_READ_WRITE_MULTIPLE_REGISTERS,
            vec![0, 10, 0, 2, 0, 10, 0, 1, 2, 0xAB, 0xCD],
        );
        let resp = handler.handle_read_write_multiple_registers(&ctx(), &req, &store).unwrap();
        assert_eq!(resp.get_pdu().data, vec![4, 0xAB, 0xCD, 0x00, 0x00]);

        // read/write limits: write quantity 122 > 121
        let mut d = vec![0, 0, 0, 1, 0, 0, 0, 122, 244];
        d.extend(vec![0u8; 244]);
        let req = mock_req(FUNC_READ_WRITE_MULTIPLE_REGISTERS, d);
        assert!(handler.handle_read_write_multiple_registers(&ctx(), &req, &store).is_err());
    }

    #[test]
    fn read_device_identification() {
        let handler = ServerProtocolHandler::new();
        let store = MockDataStore::new();

        let req = mock_req(FUNC_READ_DEVICE_IDENTIFICATION, vec![0x0E, 0x01, 0x00]);
        let resp = handler.handle_read_device_identification(&ctx(), &req, &store).unwrap();
        let d = &resp.get_pdu().data;
        assert_eq!(&d[0..6], &[0x0E, 0x01, 0x01, 0x00, 0x00, 3]);
        assert_eq!(&d[6..8], &[0x00, 8]);
        assert_eq!(&d[8..16], b"gomodbus");

        // specific object
        let req = mock_req(FUNC_READ_DEVICE_IDENTIFICATION, vec![0x0E, 0x04, 0x01]);
        let resp = handler.handle_read_device_identification(&ctx(), &req, &store).unwrap();
        assert_eq!(resp.get_pdu().data[5], 1);

        // wrong MEI type / short data
        let req = mock_req(FUNC_READ_DEVICE_IDENTIFICATION, vec![0x0D, 0x01, 0x00]);
        assert!(handler.handle_read_device_identification(&ctx(), &req, &store).is_err());
        let req = mock_req(FUNC_READ_DEVICE_IDENTIFICATION, vec![0x0E, 0x01]);
        assert!(handler.handle_read_device_identification(&ctx(), &req, &store).is_err());
    }

    #[test]
    fn server_response_encode() {
        let r = ServerResponse::new(TransactionId(0x0102), UnitId(7), FunctionCode(0x83), vec![2]);
        assert_eq!(r.encode().unwrap(), vec![1, 2, 0, 0, 0, 3, 7, 0x83, 2]);
        assert!(r.is_exception());
        assert_eq!(r.get_exception(), ExceptionCode(2));
    }
}
