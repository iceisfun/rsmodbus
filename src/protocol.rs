//! Modbus PDU encoding/decoding. Port of Go package `protocol` (`protocol.go`).
//!
//! [`ProtocolHandler`] implements [`common::Protocol`](crate::common::Protocol):
//! it builds request PDU *data* (everything after the function code byte) and
//! parses response PDU data. All multi-byte values are big-endian.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.2 (Data Encoding):
//! "MODBUS uses a 'big-Endian' representation for addresses and data items."

use std::sync::Arc;

use crate::common::util::format_bytes;
use crate::common::*;
use crate::logging;

/// ProtocolHandler implements the [`Protocol`] trait for the Modbus protocol.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (Protocol description) and Section 6
pub struct ProtocolHandler {
    // Implementation detail (not spec): sink for debug/error diagnostics.
    logger: Arc<dyn LoggerInterface>,
}

impl Default for ProtocolHandler {
    fn default() -> Self {
        ProtocolHandler::new()
    }
}

impl ProtocolHandler {
    /// Creates a handler with the default logger (Go `NewProtocolHandler()`).
    pub fn new() -> ProtocolHandler {
        ProtocolHandler { logger: logging::default_logger() }
    }

    /// Creates a handler with the given logger (Go `NewProtocolHandler(WithLogger(l))`).
    pub fn new_with_logger(logger: Arc<dyn LoggerInterface>) -> ProtocolHandler {
        ProtocolHandler { logger }
    }

    /// Helper for generating read requests that share one layout (read coils,
    /// discrete inputs, holding registers, input registers).
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Sections 6.1-6.4 (request PDU:
    /// Starting Address 2 bytes, Quantity 2 bytes).
    fn generate_read_request(&self, item_type: &str, address: Address, quantity: Quantity, max_quantity: u16) -> Result<Vec<u8>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Generating read {} request: address={}, quantity={}", item_type, address, quantity));

        // Quantity constraints: 1 to max (2000 for bits, 125 for registers)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Sections 6.1-6.4
        if quantity.0 == 0 || quantity.0 > max_quantity {
            self.logger.error(&ctx, format_args!("Invalid quantity for read {} request: {} (max {})", item_type, quantity, max_quantity));
            return Err(Error::InvalidQuantity);
        }

        let mut data = Vec::with_capacity(4);
        data.extend_from_slice(&address.0.to_be_bytes()); // Starting Address
        data.extend_from_slice(&quantity.0.to_be_bytes()); // Quantity

        self.logger.debug(&ctx, format_args!("Generated read {} request data: {}", item_type, format_bytes(&data)));
        Ok(data)
    }

    /// Helper for parsing responses that contain packed bit values (coils and
    /// discrete inputs).
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Sections 6.1/6.2 (response PDU:
    /// Byte Count 1 byte, status N bytes, "one coil per bit", LSB of first byte =
    /// lowest address; N = (quantity + 7) / 8).
    fn parse_bit_response(&self, item_type: &str, data: &[u8], quantity: Quantity) -> Result<Vec<bool>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Parsing read {} response: data={}, quantity={}", item_type, format_bytes(data), quantity));

        if data.is_empty() {
            self.logger.error(&ctx, format_args!("Empty response for read {}", item_type));
            return Err(Error::EmptyResponse);
        }

        // First byte is the byte count
        let byte_count = data[0] as usize;
        if data.len() != byte_count + 1 {
            self.logger.error(&ctx, format_args!("Invalid response length for read {}: expected {}, got {}", item_type, byte_count + 1, data.len()));
            return Err(Error::InvalidResponseLength);
        }

        // Calculate the expected byte count: ceil(quantity / 8)
        let expected_byte_count = (quantity.0 as usize).div_ceil(8);
        if byte_count != expected_byte_count {
            self.logger.error(&ctx, format_args!("Invalid byte count for read {}: expected {}, got {}", item_type, expected_byte_count, byte_count));
            return Err(Error::InvalidResponseLength);
        }

        // Unpack the values (bit 0 of byte 0 is the first item)
        let values: Vec<bool> = (0..quantity.0 as usize)
            .map(|i| ((data[1 + i / 8] >> (i % 8)) & 0x01) == 1)
            .collect();

        self.logger.debug(&ctx, format_args!("Parsed {} {} values", values.len(), item_type));
        Ok(values)
    }

    /// Helper for parsing responses that contain register values (holding and
    /// input registers).
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Sections 6.3/6.4 (response PDU:
    /// Byte Count = 2 x N, then N registers, each "in two bytes", high byte first).
    fn parse_register_response(&self, item_type: &str, data: &[u8], quantity: Quantity) -> Result<Vec<u16>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Parsing read {} response: data={}, quantity={}", item_type, format_bytes(data), quantity));

        if data.is_empty() {
            self.logger.error(&ctx, format_args!("Empty response for read {}", item_type));
            return Err(Error::EmptyResponse);
        }

        // First byte is the byte count
        let byte_count = data[0] as usize;
        if data.len() != byte_count + 1 {
            self.logger.error(&ctx, format_args!("Invalid response length for read {}: expected {}, got {}", item_type, byte_count + 1, data.len()));
            return Err(Error::InvalidResponseLength);
        }

        // Calculate the expected byte count: 2 bytes per register
        let expected_byte_count = quantity.0 as usize * BYTES_PER_REGISTER;
        if byte_count != expected_byte_count {
            self.logger.error(&ctx, format_args!("Invalid byte count for read {}: expected {}, got {}", item_type, expected_byte_count, byte_count));
            return Err(Error::InvalidResponseLength);
        }

        let values: Vec<u16> = (0..quantity.0 as usize)
            .map(|i| u16::from_be_bytes([data[1 + i * 2], data[2 + i * 2]]))
            .collect();

        self.logger.debug(&ctx, format_args!("Parsed {} {} values", values.len(), item_type));
        Ok(values)
    }
}

impl Protocol for ProtocolHandler {
    /// Returns a new handler with the given logger (Go `WithLogger` creates a new instance).
    fn with_logger(&self, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Protocol> {
        Arc::new(ProtocolHandler::new_with_logger(logger))
    }

    /// Generates a request to read coils.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Read Coils)
    ///
    /// PDU Data:
    /// Starting Address (2 bytes) - Ref: Section 6.1
    /// Quantity of Coils (2 bytes) - Ref: Section 6.1
    /// Quantity constraints: 1 to 2000 (0x07D0) - Ref: Section 6.1
    fn generate_read_coils_request(&self, address: Address, quantity: Quantity) -> Result<Vec<u8>> {
        self.generate_read_request("coils", address, quantity, MAX_COIL_COUNT)
    }

    /// Parses a response to a read coils request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Read Coils)
    ///
    /// PDU Data:
    /// Byte Count (1 byte) - number of data bytes to follow (N) - Ref: Section 6.1
    /// Coil Status (Byte Count bytes, packed bits, LSB of first byte = lowest address) - Ref: Section 6.1
    fn parse_read_coils_response(&self, data: &[u8], quantity: Quantity) -> Result<Vec<CoilValue>> {
        self.parse_bit_response("coils", data, quantity)
    }

    /// Generates a request to read discrete inputs.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (Read Discrete Inputs)
    /// Quantity of Inputs: 1 to 2000 (0x07D0).
    fn generate_read_discrete_inputs_request(&self, address: Address, quantity: Quantity) -> Result<Vec<u8>> {
        self.generate_read_request("discrete inputs", address, quantity, MAX_COIL_COUNT)
    }

    /// Parses a response to a read discrete inputs request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (Read Discrete Inputs)
    /// Same packed-bit response layout as Read Coils.
    fn parse_read_discrete_inputs_response(&self, data: &[u8], quantity: Quantity) -> Result<Vec<DiscreteInputValue>> {
        self.parse_bit_response("discrete inputs", data, quantity)
    }

    /// Generates a request to read holding registers.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Read Holding Registers)
    /// Quantity of Registers: 1 to 125 (0x007D).
    fn generate_read_holding_registers_request(&self, address: Address, quantity: Quantity) -> Result<Vec<u8>> {
        self.generate_read_request("holding registers", address, quantity, MAX_REGISTER_COUNT)
    }

    /// Parses a response to a read holding registers request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Read Holding Registers)
    fn parse_read_holding_registers_response(&self, data: &[u8], quantity: Quantity) -> Result<Vec<RegisterValue>> {
        self.parse_register_response("holding registers", data, quantity)
    }

    /// Generates a request to read input registers.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Read Input Registers)
    /// Quantity of Input Registers: 1 to 125 (0x007D).
    fn generate_read_input_registers_request(&self, address: Address, quantity: Quantity) -> Result<Vec<u8>> {
        self.generate_read_request("input registers", address, quantity, MAX_REGISTER_COUNT)
    }

    /// Parses a response to a read input registers request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Read Input Registers)
    fn parse_read_input_registers_response(&self, data: &[u8], quantity: Quantity) -> Result<Vec<InputRegisterValue>> {
        self.parse_register_response("input registers", data, quantity)
    }

    /// Generates a request to write a single coil.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5 (Write Single Coil)
    ///
    /// PDU Data:
    /// Output Address (2 bytes) - Ref: Section 6.5
    /// Output Value (2 bytes: 0xFF00 for ON, 0x0000 for OFF) - Ref: Section 6.5
    fn generate_write_single_coil_request(&self, address: Address, value: CoilValue) -> Result<Vec<u8>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Generating write single coil request: address={}, value={}", address, value));

        let mut data = Vec::with_capacity(4);
        // Address in big-endian format (most significant byte first)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.2 (Data Encoding)
        data.extend_from_slice(&address.0.to_be_bytes());

        // Encode boolean as 0xFF00 (on) or 0x0000 (off)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5
        // "A value of 0xFF00 requests the coil to be ON. A value of 0x0000 requests the coil to be OFF."
        let raw = if value { COIL_ON_U16 } else { COIL_OFF_U16 };
        data.extend_from_slice(&raw.to_be_bytes());

        self.logger.debug(&ctx, format_args!("Generated write single coil request data: {}", format_bytes(&data)));
        Ok(data)
    }

    /// Parses a response to a write single coil request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5 (Write Single Coil)
    ///
    /// PDU Data (echo of request):
    /// Output Address (2 bytes) - Ref: Section 6.5
    /// Output Value (2 bytes) - Ref: Section 6.5
    fn parse_write_single_coil_response(&self, data: &[u8]) -> Result<(Address, CoilValue)> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Parsing write single coil response: data={}", format_bytes(data)));

        if data.len() != 4 {
            self.logger.error(&ctx, format_args!("Invalid response length for write single coil: expected 4, got {}", data.len()));
            return Err(Error::InvalidResponseLength);
        }

        // Parse address from big-endian format
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.2 (Data Encoding)
        let address = Address(u16::from_be_bytes([data[0], data[1]]));
        let value = u16::from_be_bytes([data[2], data[3]]);

        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5
        // The normal response is an echo of the request; only 0xFF00 / 0x0000 are legal.
        match value {
            COIL_ON_U16 => {
                self.logger.debug(&ctx, format_args!("Parsed write single coil response: address={}, value=true", address));
                Ok((address, true))
            }
            COIL_OFF_U16 => {
                self.logger.debug(&ctx, format_args!("Parsed write single coil response: address={}, value=false", address));
                Ok((address, false))
            }
            _ => {
                self.logger.error(&ctx, format_args!("Invalid coil value in response: {}", value));
                Err(Error::msg(format!("invalid coil value: {}", value)))
            }
        }
    }

    /// Generates a request to write a single register.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.6 (Write Single Register)
    ///
    /// PDU Data:
    /// Register Address (2 bytes) - Ref: Section 6.6
    /// Register Value (2 bytes, 0x0000 to 0xFFFF) - Ref: Section 6.6
    fn generate_write_single_register_request(&self, address: Address, value: RegisterValue) -> Result<Vec<u8>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Generating write single register request: address={}, value={}", address, value));

        let mut data = Vec::with_capacity(4);
        data.extend_from_slice(&address.0.to_be_bytes());
        data.extend_from_slice(&value.to_be_bytes());

        self.logger.debug(&ctx, format_args!("Generated write single register request data: {}", format_bytes(&data)));
        Ok(data)
    }

    /// Parses a response to a write single register request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.6 (Write Single Register)
    /// The normal response is an echo of the request (address 2 bytes, value 2 bytes).
    fn parse_write_single_register_response(&self, data: &[u8]) -> Result<(Address, RegisterValue)> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Parsing write single register response: data={}", format_bytes(data)));

        if data.len() != 4 {
            self.logger.error(&ctx, format_args!("Invalid response length for write single register: expected 4, got {}", data.len()));
            return Err(Error::InvalidResponseLength);
        }

        let address = Address(u16::from_be_bytes([data[0], data[1]]));
        let value: RegisterValue = u16::from_be_bytes([data[2], data[3]]);

        self.logger.debug(&ctx, format_args!("Parsed write single register response: address={}, value={}", address, value));
        Ok((address, value))
    }

    /// Generates a request to write multiple coils.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (Write Multiple Coils)
    ///
    /// PDU Data:
    /// Starting Address (2 bytes) - Ref: Section 6.11
    /// Quantity of Outputs (2 bytes) - Ref: Section 6.11
    /// Byte Count (1 byte) - number of data bytes to follow - Ref: Section 6.11
    /// Outputs Value (Byte Count bytes, packed bits) - Ref: Section 6.11
    /// Quantity constraints: 1 to 1968 (0x07B0) - Ref: Section 6.11
    fn generate_write_multiple_coils_request(&self, address: Address, values: &[CoilValue]) -> Result<Vec<u8>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Generating write multiple coils request: address={}, count={}", address, values.len()));

        if values.is_empty() || values.len() > MAX_WRITE_COIL_COUNT as usize {
            self.logger.error(&ctx, format_args!("Invalid quantity for write multiple coils request: {}", values.len()));
            return Err(Error::InvalidQuantity);
        }

        // Byte count = ceil(quantity / 8)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (N = Quantity of Outputs / 8, +1 if remainder)
        let byte_count = values.len().div_ceil(8);
        let mut data = vec![0u8; 5 + byte_count];

        // Address - big-endian. Ref: Section 4.2 (Data Encoding)
        data[0..2].copy_from_slice(&address.0.to_be_bytes());
        // Quantity - big-endian
        data[2..4].copy_from_slice(&(values.len() as u16).to_be_bytes());
        // Byte count
        data[4] = byte_count as u8;

        // Pack coil values - LSB of first byte is the lowest coil address
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11
        for (i, value) in values.iter().enumerate() {
            if *value {
                data[5 + i / 8] |= 1 << (i % 8);
            }
        }

        self.logger.debug(&ctx, format_args!("Generated write multiple coils request data: {}", format_bytes(&data)));
        Ok(data)
    }

    /// Parses a response to a write multiple coils request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (Write Multiple Coils)
    ///
    /// PDU Data:
    /// Starting Address (2 bytes) - Ref: Section 6.11
    /// Quantity of Outputs (2 bytes) - Ref: Section 6.11
    fn parse_write_multiple_coils_response(&self, data: &[u8]) -> Result<(Address, Quantity)> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Parsing write multiple coils response: data={}", format_bytes(data)));

        if data.len() != 4 {
            self.logger.error(&ctx, format_args!("Invalid response length for write multiple coils: expected 4, got {}", data.len()));
            return Err(Error::InvalidResponseLength);
        }

        // Address and quantity, big-endian. Ref: Section 4.2 (Data Encoding)
        let address = Address(u16::from_be_bytes([data[0], data[1]]));
        let quantity = Quantity(u16::from_be_bytes([data[2], data[3]]));

        self.logger.debug(&ctx, format_args!("Parsed write multiple coils response: address={}, quantity={}", address, quantity));
        Ok((address, quantity))
    }

    /// Generates a request to write multiple registers.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (Write Multiple Registers)
    ///
    /// PDU Data:
    /// Starting Address (2 bytes), Quantity of Registers (2 bytes), Byte Count (1 byte = 2 x N),
    /// Registers Value (N x 2 bytes) - Ref: Section 6.12
    /// Quantity constraints: 1 to 123 (0x007B) - Ref: Section 6.12
    fn generate_write_multiple_registers_request(&self, address: Address, values: &[RegisterValue]) -> Result<Vec<u8>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Generating write multiple registers request: address={}, count={}", address, values.len()));

        if values.is_empty() || values.len() > MAX_WRITE_REGISTER_COUNT as usize {
            self.logger.error(&ctx, format_args!("Invalid quantity for write multiple registers request: {}", values.len()));
            return Err(Error::InvalidQuantity);
        }

        // Byte count: 2 bytes per register. Ref: Section 6.12
        let byte_count = values.len() * BYTES_PER_REGISTER;
        let mut data = Vec::with_capacity(5 + byte_count);

        data.extend_from_slice(&address.0.to_be_bytes()); // Starting Address
        data.extend_from_slice(&(values.len() as u16).to_be_bytes()); // Quantity of Registers
        data.push(byte_count as u8); // Byte Count

        // Register values, big-endian. Ref: Section 4.2 (Data Encoding)
        for value in values {
            data.extend_from_slice(&value.to_be_bytes());
        }

        self.logger.debug(&ctx, format_args!("Generated write multiple registers request data: {}", format_bytes(&data)));
        Ok(data)
    }

    /// Parses a response to a write multiple registers request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (Write Multiple Registers)
    /// Response: Starting Address (2 bytes), Quantity of Registers (2 bytes).
    fn parse_write_multiple_registers_response(&self, data: &[u8]) -> Result<(Address, Quantity)> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Parsing write multiple registers response: data={}", format_bytes(data)));

        if data.len() != 4 {
            self.logger.error(&ctx, format_args!("Invalid response length for write multiple registers: expected 4, got {}", data.len()));
            return Err(Error::InvalidResponseLength);
        }

        let address = Address(u16::from_be_bytes([data[0], data[1]]));
        let quantity = Quantity(u16::from_be_bytes([data[2], data[3]]));

        self.logger.debug(&ctx, format_args!("Parsed write multiple registers response: address={}, quantity={}", address, quantity));
        Ok((address, quantity))
    }

    /// Generates a request to read and write multiple registers.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.17 (Read/Write Multiple Registers)
    ///
    /// PDU Data:
    /// Read Starting Address (2 bytes) - Ref: Section 6.17
    /// Quantity to Read (2 bytes) - Ref: Section 6.17
    /// Write Starting Address (2 bytes) - Ref: Section 6.17
    /// Quantity to Write (2 bytes) - Ref: Section 6.17
    /// Write Byte Count (1 byte) - Ref: Section 6.17
    /// Write Registers Value (N x 2 bytes) - Ref: Section 6.17
    /// Read Quantity constraints: 1 to 125 (0x007D) - Ref: Section 6.17
    /// Write Quantity constraints: 1 to 121 (0x0079) - Ref: Section 6.17
    fn generate_read_write_multiple_registers_request(
        &self,
        read_address: Address,
        read_quantity: Quantity,
        write_address: Address,
        write_values: &[RegisterValue],
    ) -> Result<Vec<u8>> {
        let ctx = Context::background();
        self.logger.debug(
            &ctx,
            format_args!(
                "Generating read/write multiple registers request: readAddress={}, readQuantity={}, writeAddress={}, writeCount={}",
                read_address, read_quantity, write_address, write_values.len()
            ),
        );

        if read_quantity.0 == 0 || read_quantity.0 > MAX_READ_WRITE_READ_COUNT {
            self.logger.error(&ctx, format_args!("Invalid read quantity for read/write multiple registers request: {}", read_quantity));
            return Err(Error::InvalidQuantity);
        }
        if write_values.is_empty() || write_values.len() > MAX_READ_WRITE_WRITE_COUNT as usize {
            self.logger.error(&ctx, format_args!("Invalid write quantity for read/write multiple registers request: {}", write_values.len()));
            return Err(Error::InvalidQuantity);
        }

        // Byte count (2 bytes per register)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.2 (Data Encoding)
        let byte_count = write_values.len() * BYTES_PER_REGISTER;
        let mut data = Vec::with_capacity(9 + byte_count);

        // All fields big-endian. Ref: Section 4.2 (Data Encoding)
        data.extend_from_slice(&read_address.0.to_be_bytes()); // Read Starting Address
        data.extend_from_slice(&read_quantity.0.to_be_bytes()); // Quantity to Read
        data.extend_from_slice(&write_address.0.to_be_bytes()); // Write Starting Address
        data.extend_from_slice(&(write_values.len() as u16).to_be_bytes()); // Quantity to Write
        data.push(byte_count as u8); // Write Byte Count

        // Write Registers Value - each value is 2 bytes big-endian
        for value in write_values {
            data.extend_from_slice(&value.to_be_bytes());
        }

        self.logger.debug(&ctx, format_args!("Generated read/write multiple registers request data: {}", format_bytes(&data)));
        Ok(data)
    }

    /// Parses a response to a read/write multiple registers request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.17 (Read/Write Multiple Registers)
    ///
    /// PDU Data:
    /// Byte Count (1 byte) - N x 2 bytes of read data - Ref: Section 6.17
    /// Read Registers Value (N x 2 bytes) - Ref: Section 6.17
    fn parse_read_write_multiple_registers_response(&self, data: &[u8], read_quantity: Quantity) -> Result<Vec<RegisterValue>> {
        // Same implementation as parse_read_holding_registers_response: reading holding
        // registers and the read part of ReadWriteMultipleRegisters share one response format.
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.17
        self.parse_read_holding_registers_response(data, read_quantity)
    }

    /// Generates a request to read the exception status.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.7 (Read Exception Status)
    /// The request PDU carries only the function code (no data).
    fn generate_read_exception_status_request(&self) -> Result<Vec<u8>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Generating read exception status request"));

        // No data for this request
        Ok(Vec::new())
    }

    /// Parses a response to a read exception status request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.7 (Read Exception Status)
    /// Response: Output Data (1 byte, eight exception status "coils").
    fn parse_read_exception_status_response(&self, data: &[u8]) -> Result<ExceptionStatus> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Parsing read exception status response: data={}", format_bytes(data)));

        if data.len() != 1 {
            self.logger.error(&ctx, format_args!("Invalid response length for read exception status: expected 1, got {}", data.len()));
            return Err(Error::InvalidResponseLength);
        }

        let status = ExceptionStatus(data[0]);
        self.logger.debug(&ctx, format_args!("Parsed read exception status response: status={}", status));
        Ok(status)
    }

    /// Generates a request to read device identification.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Read Device Identification, Request PDU)
    ///
    /// PDU Data (after function code 0x2B):
    /// MEI Type (1 byte, 0x0E), Read Device ID code (1 byte, 01-04), Object Id (1 byte).
    fn generate_read_device_identification_request(&self, read_device_id_code: ReadDeviceIdCode, object_id: DeviceIdObjectCode) -> Result<Vec<u8>> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Generating read device identification request: code={}, objectID={}", read_device_id_code.0, object_id.0));

        // Validate read device ID code (01..04). Ref: Section 6.21, Table 73
        if read_device_id_code < READ_DEVICE_ID_BASIC || read_device_id_code > READ_DEVICE_ID_SPECIFIC {
            self.logger.error(&ctx, format_args!("Invalid read device ID code: {}", read_device_id_code.0));
            return Err(Error::InvalidValue);
        }

        // Data format:
        // Byte 0: MEI Type (0x0E for ReadDeviceID)
        // Byte 1: ReadDeviceID code (0x01-0x04)
        // Byte 2: Object ID
        let data = vec![MEI_READ_DEVICE_ID.0, read_device_id_code.0, object_id.0];

        self.logger.debug(&ctx, format_args!("Generated read device identification request data: {}", format_bytes(&data)));
        Ok(data)
    }

    /// Parses a response from a read device identification request.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Read Device Identification, Response PDU)
    ///
    /// PDU Data (after function code 0x2B):
    /// MEI Type (1), Read Device ID code (1), Conformity Level (1), More Follows (1),
    /// Next Object Id (1), Number of Objects (1), then per object: Object Id (1),
    /// Object Length (1), Object Value (Length bytes).
    fn parse_read_device_identification_response(&self, data: &[u8]) -> Result<DeviceIdentification> {
        let ctx = Context::background();
        self.logger.debug(&ctx, format_args!("Parsing read device identification response: {}", format_bytes(data)));

        // Check data length - minimum is 6 bytes (for a response with no objects):
        // MEI Type (1) + ReadDeviceID code (1) + Conformity level (1) + More Follows (1) +
        // Next Object ID (1) + Number of Objects (1)
        if data.len() < 6 {
            self.logger.error(&ctx, format_args!("Invalid response length for read device identification: {}", data.len()));
            return Err(Error::InvalidResponseLength);
        }

        // Check MEI Type. Ref: Section 6.21 (MEI Type 0x0E)
        if MeiType(data[0]) != MEI_READ_DEVICE_ID {
            self.logger.error(&ctx, format_args!("Invalid MEI type: 0x{:02X}, expected 0x{:02X}", data[0], MEI_READ_DEVICE_ID.0));
            return Err(Error::InvalidValue);
        }

        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response PDU Format)
        let mut result = DeviceIdentification {
            read_device_id_code: ReadDeviceIdCode(data[1]), // Echoes the request's code
            conformity_level: ConformityLevel(data[2]),     // Conformity level of the device
            more_follows: MoreFollows(data[3]),             // More objects follow in subsequent requests
            next_object_id: DeviceIdObjectCode(data[4]),    // Object ID to request next if more_follows
            number_of_objects: data[5],                     // Number of objects in this response
            objects: Vec::with_capacity(data[5] as usize),
        };

        // Parse objects
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response Objects Format)
        // Each object: Object ID (1 byte), Object Length (1 byte), Object Value (n bytes)
        let mut offset = 6usize;
        for _ in 0..data[5] {
            // Check if we have enough data for the object header
            if offset + 2 > data.len() {
                self.logger.error(&ctx, format_args!("Invalid response format for read device identification: not enough data for object header"));
                return Err(Error::InvalidResponseFormat);
            }

            let object_id = DeviceIdObjectCode(data[offset]);
            let object_length = data[offset + 1];
            offset += 2;

            // Check if we have enough data for the object value
            if offset + object_length as usize > data.len() {
                self.logger.error(&ctx, format_args!("Invalid response format for read device identification: not enough data for object value"));
                return Err(Error::InvalidResponseFormat);
            }

            // Object value (bytes -> string; Go's string(bytes) is byte-preserving, lossy UTF-8 here)
            let object_value = String::from_utf8_lossy(&data[offset..offset + object_length as usize]).into_owned();
            offset += object_length as usize;

            // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21, Table 72
            result.objects.push(DeviceIdObject { id: object_id, length: object_length, value: object_value });
        }

        self.logger.debug(&ctx, format_args!("Parsed read device identification response: {} objects", result.objects.len()));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::NoopLogger;

    // Quiet handler for tests (the Go tests use the default stdout logger).
    fn handler() -> ProtocolHandler {
        ProtocolHandler::new_with_logger(Arc::new(NoopLogger))
    }

    // ---- Port of protocol/protocol_test.go ----

    #[test]
    fn test_generate_read_coils_request() {
        let handler = handler();

        // Test valid request
        let address = Address(100);
        let quantity = Quantity(10);

        let data = handler.generate_read_coils_request(address, quantity).expect("returned error");
        assert_eq!(data.len(), 4, "expected data length 4");

        // Check address field
        let addr = u16::from_be_bytes([data[0], data[1]]);
        assert_eq!(addr, address.0);

        // Check quantity field
        let quant = u16::from_be_bytes([data[2], data[3]]);
        assert_eq!(quant, quantity.0);

        // Test invalid quantity
        assert!(handler.generate_read_coils_request(address, Quantity(0)).is_err(), "quantity=0 should return error");
        assert!(
            handler.generate_read_coils_request(address, Quantity(MAX_COIL_COUNT + 1)).is_err(),
            "quantity > MaxCoilCount should return error"
        );
    }

    #[test]
    fn test_generate_read_discrete_inputs_request() {
        let handler = handler();
        let address = Address(100);
        let quantity = Quantity(10);

        let data = handler.generate_read_discrete_inputs_request(address, quantity).expect("returned error");
        assert_eq!(data.len(), 4);
        assert_eq!(u16::from_be_bytes([data[0], data[1]]), address.0);
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), quantity.0);
    }

    #[test]
    fn test_generate_read_holding_registers_request() {
        let handler = handler();
        let address = Address(100);
        let quantity = Quantity(10);

        let data = handler.generate_read_holding_registers_request(address, quantity).expect("returned error");
        assert_eq!(data.len(), 4);
        assert_eq!(u16::from_be_bytes([data[0], data[1]]), address.0);
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), quantity.0);

        // Test invalid quantity
        assert!(handler.generate_read_holding_registers_request(address, Quantity(0)).is_err());
        assert!(
            handler.generate_read_holding_registers_request(address, Quantity(MAX_REGISTER_COUNT + 1)).is_err(),
            "quantity > MaxRegisterCount should return error"
        );
    }

    #[test]
    fn test_parse_read_coils_response() {
        let handler = handler();

        // Test valid response
        let quantity = Quantity(10);
        let byte_count = 2u8; // Ceiling of 10/8

        // Create a response with some coils on and some off
        let response_data = [byte_count, 0b10101010, 0b00000011];

        let values = handler.parse_read_coils_response(&response_data, quantity).expect("returned error");
        assert_eq!(values.len(), quantity.0 as usize);

        // Check values (alternating true/false, then two on)
        let expected_values = [false, true, false, true, false, true, false, true, true, true];
        for (i, expected) in expected_values.iter().enumerate() {
            assert_eq!(values[i], *expected, "value at index {}", i);
        }

        // Test invalid responses
        // Empty response
        assert!(handler.parse_read_coils_response(&[], quantity).is_err(), "empty data should return error");

        // Wrong byte count
        assert!(handler.parse_read_coils_response(&[3, 0, 0, 0], quantity).is_err(), "incorrect byte count should return error");

        // Data too short
        assert!(handler.parse_read_coils_response(&[2, 0], quantity).is_err(), "data too short should return error");
    }

    #[test]
    fn test_generate_write_single_coil_request() {
        let handler = handler();

        // Test ON value
        let address = Address(100);
        let data = handler.generate_write_single_coil_request(address, true).expect("returned error");
        assert_eq!(data.len(), 4);
        assert_eq!(u16::from_be_bytes([data[0], data[1]]), address.0);
        // Check value field (ON = 0xFF00)
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), COIL_ON_U16, "expected value 0xFF00");

        // Test OFF value
        let data = handler.generate_write_single_coil_request(address, false).expect("returned error");
        // Check value field (OFF = 0x0000)
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), COIL_OFF_U16, "expected value 0x0000");
    }

    #[test]
    fn test_parse_write_single_coil_response() {
        let handler = handler();

        // Test valid ON response - should echo the request
        let address = Address(100);
        let mut response_data = [0u8; 4];
        response_data[0..2].copy_from_slice(&address.0.to_be_bytes());
        response_data[2..4].copy_from_slice(&COIL_ON_U16.to_be_bytes());

        let (resp_address, resp_value) = handler.parse_write_single_coil_response(&response_data).expect("returned error");
        assert_eq!(resp_address, address);
        assert!(resp_value);

        // Test valid OFF response
        response_data[2..4].copy_from_slice(&COIL_OFF_U16.to_be_bytes());
        let (_, resp_value) = handler.parse_write_single_coil_response(&response_data).expect("returned error");
        assert!(!resp_value);

        // Test invalid responses
        // Data too short
        assert!(handler.parse_write_single_coil_response(&[0, 0]).is_err(), "data too short should return error");

        // Invalid value (neither ON nor OFF)
        response_data[2..4].copy_from_slice(&0x1234u16.to_be_bytes());
        let err = handler.parse_write_single_coil_response(&response_data).unwrap_err();
        assert_eq!(err.to_string(), "invalid coil value: 4660");
    }

    #[test]
    fn test_generate_write_single_register_request() {
        let handler = handler();

        let address = Address(100);
        let value: RegisterValue = 12345;

        let data = handler.generate_write_single_register_request(address, value).expect("returned error");
        assert_eq!(data.len(), 4);
        assert_eq!(u16::from_be_bytes([data[0], data[1]]), address.0);
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), value);
    }

    #[test]
    fn test_protocol_handler_with_logger() {
        // Create a protocol handler with a custom logger
        let logger: Arc<dyn LoggerInterface> = Arc::new(NoopLogger);
        let handler: Arc<dyn Protocol> = Arc::new(ProtocolHandler::new_with_logger(logger));

        // Create a new handler with a different logger
        let new_logger: Arc<dyn LoggerInterface> = Arc::new(NoopLogger);
        let new_handler = handler.with_logger(new_logger);

        // WithLogger must return a new instance (compare data pointers)
        assert!(
            !std::ptr::eq(Arc::as_ptr(&new_handler) as *const (), Arc::as_ptr(&handler) as *const ()),
            "WithLogger returned the same instance - expected a new instance"
        );

        // Test that the new handler works correctly
        let address = Address(100);
        let data = new_handler.generate_read_coils_request(address, Quantity(10)).expect("returned error");
        assert_eq!(u16::from_be_bytes([data[0], data[1]]), address.0);
    }

    // ---- Extra coverage for cases the Go tests do not exercise ----

    #[test]
    fn test_read_limits_boundaries() {
        let h = handler();
        assert!(h.generate_read_coils_request(Address(0), Quantity(2000)).is_ok());
        assert!(h.generate_read_discrete_inputs_request(Address(0), Quantity(2001)).is_err());
        assert!(h.generate_read_input_registers_request(Address(0), Quantity(125)).is_ok());
        assert!(h.generate_read_input_registers_request(Address(0), Quantity(126)).is_err());
    }

    #[test]
    fn test_parse_register_responses() {
        let h = handler();
        let data = [4, 0x12, 0x34, 0xAB, 0xCD];
        assert_eq!(h.parse_read_holding_registers_response(&data, Quantity(2)).unwrap(), vec![0x1234, 0xABCD]);
        assert_eq!(h.parse_read_input_registers_response(&data, Quantity(2)).unwrap(), vec![0x1234, 0xABCD]);
        assert_eq!(h.parse_read_write_multiple_registers_response(&data, Quantity(2)).unwrap(), vec![0x1234, 0xABCD]);
        assert_eq!(h.parse_read_holding_registers_response(&[], Quantity(2)), Err(Error::EmptyResponse));
        assert_eq!(h.parse_read_holding_registers_response(&data, Quantity(3)), Err(Error::InvalidResponseLength));
        assert_eq!(h.parse_read_holding_registers_response(&[4, 0, 0], Quantity(2)), Err(Error::InvalidResponseLength));
    }

    #[test]
    fn test_write_multiple_coils_roundtrip() {
        let h = handler();
        let values = [true, false, true, true, false, false, false, false, true, true];
        let data = h.generate_write_multiple_coils_request(Address(19), &values).unwrap();
        // Layout per Section 6.11: start, quantity, byte count, packed bits (LSB = lowest address)
        assert_eq!(data, vec![0x00, 0x13, 0x00, 0x0A, 0x02, 0b0000_1101, 0b0000_0011]);
        assert_eq!(h.parse_write_multiple_coils_response(&[0x00, 0x13, 0x00, 0x0A]).unwrap(), (Address(19), Quantity(10)));
        assert_eq!(h.parse_write_multiple_coils_response(&[0, 1]), Err(Error::InvalidResponseLength));
        assert_eq!(h.generate_write_multiple_coils_request(Address(0), &[]), Err(Error::InvalidQuantity));
        let too_many = vec![true; MAX_WRITE_COIL_COUNT as usize + 1];
        assert_eq!(h.generate_write_multiple_coils_request(Address(0), &too_many), Err(Error::InvalidQuantity));
        let max = vec![true; MAX_WRITE_COIL_COUNT as usize];
        assert_eq!(h.generate_write_multiple_coils_request(Address(0), &max).unwrap().len(), 5 + 246);
    }

    #[test]
    fn test_write_multiple_registers() {
        let h = handler();
        // Spec example (6.12): write 0x000A and 0x0102 starting at 1
        let data = h.generate_write_multiple_registers_request(Address(1), &[0x000A, 0x0102]).unwrap();
        assert_eq!(data, vec![0x00, 0x01, 0x00, 0x02, 0x04, 0x00, 0x0A, 0x01, 0x02]);
        assert_eq!(h.parse_write_multiple_registers_response(&[0, 1, 0, 2]).unwrap(), (Address(1), Quantity(2)));
        assert_eq!(h.parse_write_multiple_registers_response(&[0, 1, 0]), Err(Error::InvalidResponseLength));
        assert_eq!(h.generate_write_multiple_registers_request(Address(0), &[]), Err(Error::InvalidQuantity));
        let max = vec![0u16; MAX_WRITE_REGISTER_COUNT as usize];
        assert!(h.generate_write_multiple_registers_request(Address(0), &max).is_ok());
        let too_many = vec![0u16; MAX_WRITE_REGISTER_COUNT as usize + 1];
        assert_eq!(h.generate_write_multiple_registers_request(Address(0), &too_many), Err(Error::InvalidQuantity));
    }

    #[test]
    fn test_write_single_register_response() {
        let h = handler();
        assert_eq!(h.parse_write_single_register_response(&[0, 1, 0, 3]).unwrap(), (Address(1), 3));
        assert_eq!(h.parse_write_single_register_response(&[0, 1, 0]), Err(Error::InvalidResponseLength));
    }

    #[test]
    fn test_read_write_multiple_registers_request() {
        let h = handler();
        // Spec example (6.17): read 6 from 3, write 3 registers at 14
        let data = h
            .generate_read_write_multiple_registers_request(Address(3), Quantity(6), Address(14), &[0x00FF, 0x00FF, 0x00FF])
            .unwrap();
        assert_eq!(
            data,
            vec![0x00, 0x03, 0x00, 0x06, 0x00, 0x0E, 0x00, 0x03, 0x06, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF]
        );
        assert_eq!(
            h.generate_read_write_multiple_registers_request(Address(0), Quantity(0), Address(0), &[1]),
            Err(Error::InvalidQuantity)
        );
        assert_eq!(
            h.generate_read_write_multiple_registers_request(Address(0), Quantity(126), Address(0), &[1]),
            Err(Error::InvalidQuantity)
        );
        assert_eq!(
            h.generate_read_write_multiple_registers_request(Address(0), Quantity(1), Address(0), &[]),
            Err(Error::InvalidQuantity)
        );
        let too_many = vec![0u16; MAX_READ_WRITE_WRITE_COUNT as usize + 1];
        assert_eq!(
            h.generate_read_write_multiple_registers_request(Address(0), Quantity(1), Address(0), &too_many),
            Err(Error::InvalidQuantity)
        );
        let max = vec![0u16; MAX_READ_WRITE_WRITE_COUNT as usize];
        assert!(h.generate_read_write_multiple_registers_request(Address(0), Quantity(125), Address(0), &max).is_ok());
    }

    #[test]
    fn test_exception_status() {
        let h = handler();
        assert_eq!(h.generate_read_exception_status_request().unwrap(), Vec::<u8>::new());
        assert_eq!(h.parse_read_exception_status_response(&[0x6D]).unwrap(), ExceptionStatus(0x6D));
        assert_eq!(h.parse_read_exception_status_response(&[]), Err(Error::InvalidResponseLength));
        assert_eq!(h.parse_read_exception_status_response(&[1, 2]), Err(Error::InvalidResponseLength));
    }

    // ---- Port of protocol/device_id_test.go ----

    #[test]
    fn test_generate_read_device_identification_request() {
        let handler = handler();

        // Test with valid parameters for basic identification
        let data = handler.generate_read_device_identification_request(READ_DEVICE_ID_BASIC, DeviceIdObjectCode(0)).expect("unexpected error");
        let expected = vec![MEI_READ_DEVICE_ID.0, READ_DEVICE_ID_BASIC.0, 0];
        assert_eq!(data, expected);

        // Test with valid parameters for specific object
        let data = handler
            .generate_read_device_identification_request(READ_DEVICE_ID_SPECIFIC, DEVICE_ID_VENDOR_NAME)
            .expect("unexpected error");
        let expected = vec![MEI_READ_DEVICE_ID.0, READ_DEVICE_ID_SPECIFIC.0, DEVICE_ID_VENDOR_NAME.0];
        assert_eq!(data, expected);

        // Test with invalid parameters: invalid code
        assert!(
            handler.generate_read_device_identification_request(ReadDeviceIdCode(0), DeviceIdObjectCode(0)).is_err(),
            "expected error for invalid read device ID code"
        );
        assert!(handler.generate_read_device_identification_request(ReadDeviceIdCode(5), DeviceIdObjectCode(0)).is_err());
    }

    #[test]
    fn test_parse_read_device_identification_response() {
        let handler = handler();

        // Test with valid response data for basic identification
        let mut response_data: Vec<u8> = vec![
            MEI_READ_DEVICE_ID.0,   // MEI type
            READ_DEVICE_ID_BASIC.0, // Read device ID code
            0x01,                   // Conformity level
            0x00,                   // More follows
            0x00,                   // Next object ID
            0x03,                   // Number of objects
            // Object 1: Vendor name
            DEVICE_ID_VENDOR_NAME.0, // ID
            0x0B,                    // Length
        ];
        response_data.extend_from_slice(b"Acme Device"); // Value
        // Object 2: Product code
        response_data.extend_from_slice(&[DEVICE_ID_PRODUCT_CODE.0, 0x05]);
        response_data.extend_from_slice(b"ABC12");
        // Object 3: Revision
        response_data.extend_from_slice(&[DEVICE_ID_MAJOR_MINOR_REVISION.0, 0x05]);
        response_data.extend_from_slice(b"V1.00");

        let device_id = handler.parse_read_device_identification_response(&response_data).expect("unexpected error");

        assert_eq!(device_id.read_device_id_code, READ_DEVICE_ID_BASIC);
        assert_eq!(device_id.conformity_level, CONFORMITY_LEVEL_BASIC);
        assert_eq!(device_id.more_follows, MORE_FOLLOWS_NO);
        assert_eq!(device_id.number_of_objects, 3);

        // Check object 1
        let obj = device_id.get_object(DEVICE_ID_VENDOR_NAME).expect("Vendor name object not found");
        assert_eq!(obj.value, "Acme Device");

        // Check object 2
        let obj = device_id.get_object(DEVICE_ID_PRODUCT_CODE).expect("Product code object not found");
        assert_eq!(obj.value, "ABC12");

        // Check object 3
        let obj = device_id.get_object(DEVICE_ID_MAJOR_MINOR_REVISION).expect("Revision object not found");
        assert_eq!(obj.value, "V1.00");

        // Check helper methods
        assert_eq!(device_id.get_vendor_name(), "Acme Device");
        assert_eq!(device_id.get_product_code(), "ABC12");
        assert_eq!(device_id.get_revision(), "V1.00");

        // Test with invalid response data: too short
        assert!(handler.parse_read_device_identification_response(&[0x0E, 0x01]).is_err(), "expected error for too short response");

        // Test with invalid response data: wrong MEI type
        let mut bad_mei = response_data.clone();
        bad_mei[0] = 0x0F; // Wrong MEI type
        assert!(handler.parse_read_device_identification_response(&bad_mei).is_err(), "expected error for wrong MEI type");

        // Test with invalid response data: invalid object data (remove last byte)
        let bad_object = &response_data[..response_data.len() - 1];
        assert!(handler.parse_read_device_identification_response(bad_object).is_err(), "expected error for invalid object data");

        // Missing object header
        let truncated = &response_data[..7];
        assert_eq!(handler.parse_read_device_identification_response(truncated), Err(Error::InvalidResponseFormat));
    }
}
