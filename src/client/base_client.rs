//! BaseClient: common functionality for all Modbus clients. Port of
//! `gomodbus/client/base_client.go`.
//!
//! The client is the "MODBUS client" role of the spec: it builds a request PDU,
//! hands it to a transport (which adds the MBAP header), waits for the response
//! and decodes it. Exception responses are turned into errors.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction, state diagram,
//! client side), Section 4.5 (Define MODBUS Transaction), Section 7 (MODBUS Exception Responses)

use std::sync::Arc;
use std::time::Duration;

use crate::common::{
    self, Address, Client, Context, DeviceIdObjectCode, DeviceIdentification, Error,
    ExceptionStatus, FunctionCode, InputRegisterValue, LoggerInterface, Protocol, Quantity,
    ReadDeviceIdCode, RegisterValue, Response, Result, Transport, UnitId, format_bytes,
};
use crate::logging;
use crate::protocol::ProtocolHandler;
use crate::transport;

/// Timeout applied by [`BaseClient::send`] when the caller's context has no deadline.
/// (Implementation detail, not spec. Ref for the concept: Section 7, client timeout condition
/// on the client side.)
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// BaseClient provides common functionality for all Modbus clients.
/// It uses a [`Transport`] for low-level communication.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction) (client side of a transaction)
pub struct BaseClient {
    logger: Arc<dyn LoggerInterface>,
    transport: Arc<dyn Transport>,
    protocol: Arc<dyn Protocol>,
    // Ref: Section 4.1 (ADU = MBAP 7 bytes + PDU); the MBAP Unit Identifier is defined in the MODBUS Messaging on TCP-IP Implementation Guide, not in this document: addresses a device behind a gateway.
    unit_id: UnitId,
}

/// A function that configures a [`BaseClient`] (Go `Option`).
pub type BaseClientOption = Box<dyn FnOnce(&mut BaseClient)>;

/// Sets the logger for the client. The logger is propagated to the transport and the
/// protocol handler (Go `WithLogger`).
///
/// Implementation detail, not spec.
pub fn with_logger(logger: Arc<dyn LoggerInterface>) -> BaseClientOption {
    Box::new(move |c| {
        c.logger = logger.clone();

        // Propagate logger to transport and protocol
        c.transport = c.transport.clone().with_logger(logger.clone());
        c.protocol = c.protocol.with_logger(logger);
    })
}

/// Sets the unit ID for the client (Go `WithUnitID`).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (ADU = MBAP 7 bytes + PDU); the MBAP Unit Identifier is defined in the MODBUS Messaging on TCP-IP Implementation Guide, not in this document
pub fn with_unit_id(unit_id: UnitId) -> BaseClientOption {
    Box::new(move |c| c.unit_id = unit_id)
}

/// Sets the protocol handler for the client (Go `WithProtocol`).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function codes descriptions)
pub fn with_protocol(protocol: Arc<dyn Protocol>) -> BaseClientOption {
    Box::new(move |c| c.protocol = protocol)
}

impl BaseClient {
    /// Creates a new BaseClient (Go `NewBaseClient`). Defaults: default (stdout, Info)
    /// logger, the standard [`ProtocolHandler`], unit ID 0. Options are applied in order.
    pub fn new(transport: Arc<dyn Transport>, options: Vec<BaseClientOption>) -> BaseClient {
        let mut client = BaseClient {
            logger: logging::default_logger(),
            transport,
            protocol: Arc::new(ProtocolHandler::new()),
            unit_id: UnitId(0), // Default unit ID
        };

        // Apply options
        for option in options {
            option(&mut client);
        }

        client
    }

    /// The client's logger.
    pub fn logger(&self) -> Arc<dyn LoggerInterface> {
        self.logger.clone()
    }

    /// The client's transport.
    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.clone()
    }

    /// The client's protocol handler.
    pub fn protocol(&self) -> Arc<dyn Protocol> {
        self.protocol.clone()
    }

    /// The unit ID placed in every request's MBAP header.
    pub fn unit_id(&self) -> UnitId {
        self.unit_id
    }

    /// Enqueues the request to the transport layer and awaits the response (Go `Send`).
    /// Fails with `Error::NotConnected` if the transport is not connected. A context without
    /// a deadline gets a 30s timeout. Exception responses are returned as `Error::Modbus`.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction) (client sends request PDU,
    /// waits for response), Section 7 (an exception response has function code | 0x80)
    pub fn send(&self, ctx: &Context, function_code: FunctionCode, data: Vec<u8>) -> Result<Box<dyn Response>> {
        if !self.is_connected() {
            return Err(Error::NotConnected);
        }

        // Create the request
        let mut request = transport::Request::new(self.unit_id, function_code, data);

        // Use the context or derive a new one with timeout
        let derived;
        let ctx = if ctx.deadline().is_none() {
            // Apply a default timeout if no deadline specified
            derived = ctx.with_timeout(DEFAULT_REQUEST_TIMEOUT);
            &derived
        } else {
            ctx
        };

        self.logger.debug(
            ctx,
            format_args!(
                "Sending request: function={}, data={}",
                function_code,
                format_bytes(&request.pdu.data)
            ),
        );

        // Send the request and get the response
        let response = match self.transport.send(ctx, &mut request) {
            Ok(r) => r,
            Err(err) => {
                self.logger.error(ctx, format_args!("Error sending request: {}", err));
                return Err(err);
            }
        };

        // Check for Modbus exception
        // Ref: Section 7 (Exception Responses)
        if response.is_exception() {
            self.logger.warn(
                ctx,
                format_args!(
                    "Received exception response: function={}, exception={}",
                    response.get_pdu().function_code,
                    response.get_exception().0
                ),
            );
            return Err(response.to_error().unwrap_or_else(|| Error::msg("exception response")));
        }

        self.logger.debug(
            ctx,
            format_args!("Received successful response: function={}", response.get_pdu().function_code),
        );
        Ok(response)
    }

    // Implementation detail, not spec: logs and passes through a request generation error.
    fn generated<T>(&self, ctx: &Context, what: &str, r: Result<T>) -> Result<T> {
        r.inspect_err(|e| self.logger.error(ctx, format_args!("Error generating {} request: {}", what, e)))
    }

    // Implementation detail, not spec: logs and passes through a response parsing error.
    fn parsed<T>(&self, ctx: &Context, what: &str, r: Result<T>) -> Result<T> {
        r.inspect_err(|e| self.logger.error(ctx, format_args!("Error parsing {} response: {}", what, e)))
    }
}

impl Client for BaseClient {
    /// Establishes a connection to the Modbus server.
    fn connect(&self, ctx: &Context) -> Result<()> {
        self.logger.info(ctx, format_args!("Connecting to Modbus server with unit ID {}", self.unit_id));
        self.transport.connect(ctx)
    }

    /// Closes the connection to the Modbus server.
    fn disconnect(&self, ctx: &Context) -> Result<()> {
        self.logger.info(ctx, format_args!("Disconnecting from Modbus server"));
        self.transport.disconnect(ctx)
    }

    /// Returns true if the client is connected to the server.
    fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    /// Reads coils from the server (FC 0x01).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Read Coils)
    fn read_coils(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<common::CoilValue>> {
        self.logger.debug(ctx, format_args!("Reading {} coils from address {}", quantity, address));

        let data = self.generated(ctx, "read coils", self.protocol.generate_read_coils_request(address, quantity))?;
        let response = self.send(ctx, common::FUNC_READ_COILS, data)?;
        let values = self.parsed(
            ctx,
            "read coils",
            self.protocol.parse_read_coils_response(&response.get_pdu().data, quantity),
        )?;

        self.logger.debug(ctx, format_args!("Read {} coils successfully", values.len()));
        Ok(values)
    }

    /// Reads discrete inputs from the server (FC 0x02).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (Read Discrete Inputs)
    fn read_discrete_inputs(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<common::DiscreteInputValue>> {
        self.logger.debug(ctx, format_args!("Reading {} discrete inputs from address {}", quantity, address));

        let data = self.generated(
            ctx,
            "read discrete inputs",
            self.protocol.generate_read_discrete_inputs_request(address, quantity),
        )?;
        let response = self.send(ctx, common::FUNC_READ_DISCRETE_INPUTS, data)?;
        let values = self.parsed(
            ctx,
            "read discrete inputs",
            self.protocol.parse_read_discrete_inputs_response(&response.get_pdu().data, quantity),
        )?;

        self.logger.debug(ctx, format_args!("Read {} discrete inputs successfully", values.len()));
        Ok(values)
    }

    /// Reads holding registers from the server (FC 0x03).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Read Holding Registers)
    fn read_holding_registers(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<RegisterValue>> {
        self.logger.debug(ctx, format_args!("Reading {} holding registers from address {}", quantity, address));

        let data = self.generated(
            ctx,
            "read holding registers",
            self.protocol.generate_read_holding_registers_request(address, quantity),
        )?;
        let response = self.send(ctx, common::FUNC_READ_HOLDING_REGISTERS, data)?;
        let values = self.parsed(
            ctx,
            "read holding registers",
            self.protocol.parse_read_holding_registers_response(&response.get_pdu().data, quantity),
        )?;

        self.logger.debug(ctx, format_args!("Read {} holding registers successfully", values.len()));
        Ok(values)
    }

    /// Reads input registers from the server (FC 0x04).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Read Input Registers)
    fn read_input_registers(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<InputRegisterValue>> {
        self.logger.debug(ctx, format_args!("Reading {} input registers from address {}", quantity, address));

        let data = self.generated(
            ctx,
            "read input registers",
            self.protocol.generate_read_input_registers_request(address, quantity),
        )?;
        let response = self.send(ctx, common::FUNC_READ_INPUT_REGISTERS, data)?;
        let values = self.parsed(
            ctx,
            "read input registers",
            self.protocol.parse_read_input_registers_response(&response.get_pdu().data, quantity),
        )?;

        self.logger.debug(ctx, format_args!("Read {} input registers successfully", values.len()));
        Ok(values)
    }

    /// Writes a single coil to the server (FC 0x05).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5 (Write Single Coil)
    fn write_single_coil(&self, ctx: &Context, address: Address, value: common::CoilValue) -> Result<()> {
        self.logger.info(ctx, format_args!("Writing coil at address {} with value {}", address, value));

        let data = self.generated(
            ctx,
            "write single coil",
            self.protocol.generate_write_single_coil_request(address, value),
        )?;
        let response = self.send(ctx, common::FUNC_WRITE_SINGLE_COIL, data)?;
        self.parsed(
            ctx,
            "write single coil",
            self.protocol.parse_write_single_coil_response(&response.get_pdu().data),
        )?;

        self.logger.debug(ctx, format_args!("Wrote coil {}={} successfully", address, value));
        Ok(())
    }

    /// Writes a single register to the server (FC 0x06).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.6 (Write Single Register)
    fn write_single_register(&self, ctx: &Context, address: Address, value: RegisterValue) -> Result<()> {
        self.logger.info(ctx, format_args!("Writing register at address {} with value {}", address, value));

        let data = self.generated(
            ctx,
            "write single register",
            self.protocol.generate_write_single_register_request(address, value),
        )?;
        let response = self.send(ctx, common::FUNC_WRITE_SINGLE_REGISTER, data)?;
        self.parsed(
            ctx,
            "write single register",
            self.protocol.parse_write_single_register_response(&response.get_pdu().data),
        )?;

        self.logger.debug(ctx, format_args!("Wrote register {}={} successfully", address, value));
        Ok(())
    }

    /// Writes multiple coils to the server (FC 0x0F).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (Write Multiple Coils)
    fn write_multiple_coils(&self, ctx: &Context, address: Address, values: &[common::CoilValue]) -> Result<()> {
        self.logger.info(ctx, format_args!("Writing {} coils starting at address {}", values.len(), address));

        let data = self.generated(
            ctx,
            "write multiple coils",
            self.protocol.generate_write_multiple_coils_request(address, values),
        )?;
        let response = self.send(ctx, common::FUNC_WRITE_MULTIPLE_COILS, data)?;
        self.parsed(
            ctx,
            "write multiple coils",
            self.protocol.parse_write_multiple_coils_response(&response.get_pdu().data),
        )?;

        self.logger.debug(ctx, format_args!("Wrote {} coils successfully", values.len()));
        Ok(())
    }

    /// Writes multiple registers to the server (FC 0x10).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (Write Multiple registers)
    fn write_multiple_registers(&self, ctx: &Context, address: Address, values: &[RegisterValue]) -> Result<()> {
        self.logger.info(ctx, format_args!("Writing {} registers starting at address {}", values.len(), address));

        let data = self.generated(
            ctx,
            "write multiple registers",
            self.protocol.generate_write_multiple_registers_request(address, values),
        )?;
        let response = self.send(ctx, common::FUNC_WRITE_MULTIPLE_REGISTERS, data)?;
        self.parsed(
            ctx,
            "write multiple registers",
            self.protocol.parse_write_multiple_registers_response(&response.get_pdu().data),
        )?;

        self.logger.debug(ctx, format_args!("Wrote {} registers successfully", values.len()));
        Ok(())
    }

    /// Reads and writes multiple registers in one transaction (FC 0x17).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.17 (Read/Write Multiple registers)
    fn read_write_multiple_registers(
        &self,
        ctx: &Context,
        read_address: Address,
        read_quantity: Quantity,
        write_address: Address,
        write_values: &[RegisterValue],
    ) -> Result<Vec<RegisterValue>> {
        self.logger.debug(
            ctx,
            format_args!(
                "Reading {} registers from {} and writing {} registers to {}",
                read_quantity,
                read_address,
                write_values.len(),
                write_address
            ),
        );

        let data = self.generated(
            ctx,
            "read/write multiple registers",
            self.protocol.generate_read_write_multiple_registers_request(
                read_address,
                read_quantity,
                write_address,
                write_values,
            ),
        )?;
        let response = self.send(ctx, common::FUNC_READ_WRITE_MULTIPLE_REGISTERS, data)?;
        let values = self.parsed(
            ctx,
            "read/write multiple registers",
            self.protocol
                .parse_read_write_multiple_registers_response(&response.get_pdu().data, read_quantity),
        )?;

        self.logger.debug(
            ctx,
            format_args!("Read/write operation completed successfully, read {} registers", values.len()),
        );
        Ok(values)
    }

    /// Reads the exception status from the server (FC 0x07).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.7 (Read Exception Status)
    fn read_exception_status(&self, ctx: &Context) -> Result<ExceptionStatus> {
        self.logger.info(ctx, format_args!("Reading exception status"));

        let data = self.generated(ctx, "read exception status", self.protocol.generate_read_exception_status_request())?;
        let response = self.send(ctx, common::FUNC_READ_EXCEPTION_STATUS, data)?;
        let status = self.parsed(
            ctx,
            "read exception status",
            self.protocol.parse_read_exception_status_response(&response.get_pdu().data),
        )?;

        self.logger.debug(ctx, format_args!("Read exception status successfully: {}", status.0));
        Ok(status)
    }

    /// Reads device identification data from the server (FC 0x2B, MEI type 0x0E).
    ///
    /// `read_device_id_code` selects the access type:
    ///   - `READ_DEVICE_ID_BASIC`: basic device identification (stream access)
    ///   - `READ_DEVICE_ID_REGULAR`: regular device identification (stream access)
    ///   - `READ_DEVICE_ID_EXTENDED`: extended device identification (stream access)
    ///   - `READ_DEVICE_ID_SPECIFIC`: one specific identification object (individual access)
    ///
    /// With `READ_DEVICE_ID_SPECIFIC`, `object_id` selects the object; otherwise it should be
    /// `DeviceIdObjectCode(0)`.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Encapsulated Interface
    /// Transport / Read Device Identification, MEI type 0x0E)
    fn read_device_identification(
        &self,
        ctx: &Context,
        read_device_id_code: ReadDeviceIdCode,
        object_id: DeviceIdObjectCode,
    ) -> Result<DeviceIdentification> {
        self.logger.debug(
            ctx,
            format_args!("Reading device identification: code={}, objectID={}", read_device_id_code.0, object_id.0),
        );

        let data = self.generated(
            ctx,
            "read device identification",
            self.protocol.generate_read_device_identification_request(read_device_id_code, object_id),
        )?;
        let response = self.send(ctx, common::FUNC_READ_DEVICE_IDENTIFICATION, data)?;
        let device_id = self.parsed(
            ctx,
            "read device identification",
            self.protocol.parse_read_device_identification_response(&response.get_pdu().data),
        )?;

        self.logger.debug(
            ctx,
            format_args!("Read device identification successfully: {} objects", device_id.objects.len()),
        );
        Ok(device_id)
    }

    /// Returns a new client with the given logger (Go `WithLogger`). The copy shares the
    /// transport (which receives the logger) and keeps the unit ID and protocol handler.
    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Client> {
        // Create a copy of the client with the new logger
        Arc::new(BaseClient::new(
            self.transport.clone(),
            vec![with_logger(logger), with_unit_id(self.unit_id), with_protocol(self.protocol.clone())],
        ))
    }
}

// ---------------------------------------------------------------------------------------------
// Tests (port of base_client_test.go and device_id_test.go, plus extra coverage of every
// function code, exception handling and option propagation)
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_support::{
        MockResponse, MockTransport, new_mock_device_identification_response,
    };
    use crate::common::*;

    fn ctx() -> Context {
        Context::background()
    }

    fn quiet() -> BaseClientOption {
        with_logger(logging::noop())
    }

    fn mock_response(fc: FunctionCode, data: Vec<u8>) -> MockResponse {
        MockResponse::new(TransactionId(1), UnitId(1), fc, data)
    }

    fn connected_client() -> (Arc<MockTransport>, BaseClient) {
        let transport = Arc::new(MockTransport::new());
        let client = BaseClient::new(transport.clone(), vec![quiet()]);
        client.connect(&ctx()).unwrap();
        (transport, client)
    }

    // Go: TestBaseClient_Connect
    #[test]
    fn connect() {
        let transport = Arc::new(MockTransport::new());
        let client = BaseClient::new(transport.clone(), vec![]);

        client.connect(&ctx()).unwrap();
        assert!(transport.is_connected(), "Transport should be connected but isn't");

        client.disconnect(&ctx()).unwrap();
        assert!(!transport.is_connected(), "Transport should be disconnected but isn't");
    }

    // Go: TestBaseClient_WithLogger
    #[test]
    fn with_logger_returns_new_instance() {
        let transport = Arc::new(MockTransport::new());
        let client = Arc::new(BaseClient::new(transport.clone(), vec![]));

        let new_client = client.clone().with_logger(logging::default_logger());

        // Verify that the new client is a different instance
        assert!(!std::ptr::addr_eq(Arc::as_ptr(&client), Arc::as_ptr(&new_client)));

        // Connect to the mock transport directly, then connect the client
        transport.connect(&ctx()).unwrap();
        new_client.connect(&ctx()).unwrap();
        assert!(transport.is_connected());
    }

    #[test]
    fn with_logger_keeps_unit_id_and_protocol() {
        let transport = Arc::new(MockTransport::new());
        let client = Arc::new(BaseClient::new(transport.clone(), vec![quiet(), with_unit_id(UnitId(7))]));
        let copy = client.with_logger(logging::noop());
        copy.connect(&ctx()).unwrap();
        transport.queue_response(mock_response(FUNC_READ_EXCEPTION_STATUS, vec![0x42]));
        assert_eq!(copy.read_exception_status(&ctx()).unwrap(), ExceptionStatus(0x42));
        // The request that reached the shared transport carries the unit ID
        assert_eq!(transport.get_requests()[0].unit_id, UnitId(7));
    }

    #[test]
    fn defaults_and_accessors() {
        let transport = Arc::new(MockTransport::new());
        let client = BaseClient::new(transport, vec![with_unit_id(UnitId(9))]);
        assert_eq!(client.unit_id(), UnitId(9));
        assert!(!client.is_connected());
        let default = BaseClient::new(Arc::new(MockTransport::new()), vec![]);
        assert_eq!(default.unit_id(), UnitId(0));
    }

    #[test]
    fn send_requires_connection() {
        let client = BaseClient::new(Arc::new(MockTransport::new()), vec![quiet()]);
        let err = client.read_coils(&ctx(), Address(0), Quantity(1)).unwrap_err();
        assert_eq!(err, Error::NotConnected);
    }

    // Go: TestBaseClient_ReadCoils
    #[test]
    fn read_coils() {
        let (transport, client) = connected_client();
        let address = Address(100);
        let quantity = Quantity(10);

        // 10 coils, alternating pattern then two true
        let response_data = vec![2, 0b10101010, 0b00000011];
        transport.queue_response(mock_response(FUNC_READ_COILS, response_data));

        let values = client.read_coils(&ctx(), address, quantity).unwrap();
        assert_eq!(values.len(), quantity.0 as usize);

        let expected = [false, true, false, true, false, true, false, true, true, true];
        for (i, e) in expected.iter().enumerate() {
            assert_eq!(values[i], *e, "Value at index {}", i);
        }

        // Verify the request that was sent
        let requests = transport.get_requests();
        assert_eq!(requests.len(), 1);
        let req = &requests[0];
        assert_eq!(req.get_pdu().function_code, FUNC_READ_COILS);

        let req_data = &req.get_pdu().data;
        assert_eq!(req_data.len(), 4);
        assert_eq!(u16::from_be_bytes([req_data[0], req_data[1]]), address.0);
        assert_eq!(u16::from_be_bytes([req_data[2], req_data[3]]), quantity.0);

        // Test with an error from the transport
        transport.clear();
        transport.queue_error(Error::msg("test error"));
        assert!(client.read_coils(&ctx(), address, quantity).is_err());
    }

    // Go: TestBaseClient_ReadHoldingRegisters
    #[test]
    fn read_holding_registers() {
        let (transport, client) = connected_client();
        let address = Address(100);
        let quantity = Quantity(2);

        // Two registers: 0x1234, 0x5678
        transport.queue_response(mock_response(FUNC_READ_HOLDING_REGISTERS, vec![4, 0x12, 0x34, 0x56, 0x78]));

        let values = client.read_holding_registers(&ctx(), address, quantity).unwrap();
        assert_eq!(values, vec![0x1234, 0x5678]);

        let requests = transport.get_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].get_pdu().function_code, FUNC_READ_HOLDING_REGISTERS);
    }

    // Go: TestBaseClient_WriteSingleCoil
    #[test]
    fn write_single_coil() {
        let (transport, client) = connected_client();
        let address = Address(100);

        // Mock response: echo of the request
        let mut response_data = vec![0u8; 4];
        response_data[0..2].copy_from_slice(&address.0.to_be_bytes());
        response_data[2..4].copy_from_slice(&COIL_ON_U16.to_be_bytes());
        transport.queue_response(mock_response(FUNC_WRITE_SINGLE_COIL, response_data));

        client.write_single_coil(&ctx(), address, true).unwrap();

        let requests = transport.get_requests();
        assert_eq!(requests.len(), 1);
        let req = &requests[0];
        assert_eq!(req.get_pdu().function_code, FUNC_WRITE_SINGLE_COIL);

        let req_data = &req.get_pdu().data;
        assert_eq!(req_data.len(), 4);
        assert_eq!(u16::from_be_bytes([req_data[0], req_data[1]]), address.0);
        // ON = 0xFF00
        assert_eq!(u16::from_be_bytes([req_data[2], req_data[3]]), COIL_ON_U16);

        // Test with a false value
        transport.clear();
        let mut response_data = vec![0u8; 4];
        response_data[0..2].copy_from_slice(&address.0.to_be_bytes());
        response_data[2..4].copy_from_slice(&COIL_OFF_U16.to_be_bytes());
        transport.queue_response(MockResponse::new(TransactionId(2), UnitId(1), FUNC_WRITE_SINGLE_COIL, response_data));

        client.write_single_coil(&ctx(), address, false).unwrap();

        // (Go left the false-value request check commented out; it is cheap here.)
        let requests = transport.get_requests();
        let req_data = &requests[0].get_pdu().data;
        assert_eq!(u16::from_be_bytes([req_data[2], req_data[3]]), COIL_OFF_U16);
    }

    // Go: TestBaseClient_ReadDeviceIdentification (device_id_test.go)
    #[test]
    fn read_device_identification() {
        let transport = Arc::new(MockTransport::new());
        let client = BaseClient::new(
            transport.clone(),
            vec![quiet(), with_protocol(Arc::new(ProtocolHandler::new_with_logger(logging::noop())))],
        );

        transport.queue_response(new_mock_device_identification_response(READ_DEVICE_ID_BASIC));
        client.connect(&ctx()).unwrap();

        let device_id = client
            .read_device_identification(&ctx(), READ_DEVICE_ID_BASIC, DeviceIdObjectCode(0))
            .unwrap();

        assert_eq!(device_id.read_device_id_code, READ_DEVICE_ID_BASIC);
        assert_eq!(device_id.get_vendor_name(), "Acme Inc.");
        assert_eq!(device_id.get_product_code(), "ABC123");
        assert_eq!(device_id.get_revision(), "V1.0");

        // Error case: transport error
        transport.queue_error(Error::Timeout);
        assert!(client.read_device_identification(&ctx(), READ_DEVICE_ID_BASIC, DeviceIdObjectCode(0)).is_err());

        // Error case: protocol error in request generation
        transport.clear();
        assert!(
            client
                .read_device_identification(&ctx(), ReadDeviceIdCode(0xFF), DeviceIdObjectCode(0))
                .is_err()
        );
    }

    // Ref: Section 6.21 - individual access (0x04) of one object
    #[test]
    fn read_device_identification_specific_object_request() {
        let (transport, client) = connected_client();
        transport.queue_response(new_mock_device_identification_response(READ_DEVICE_ID_SPECIFIC));
        let _ = client.read_device_identification(&ctx(), READ_DEVICE_ID_SPECIFIC, DEVICE_ID_PRODUCT_CODE);
        let req = &transport.get_requests()[0];
        assert_eq!(req.get_pdu().function_code, FUNC_READ_DEVICE_IDENTIFICATION);
        assert_eq!(
            req.get_pdu().data,
            vec![MEI_READ_DEVICE_ID.0, READ_DEVICE_ID_SPECIFIC.0, DEVICE_ID_PRODUCT_CODE.0]
        );
    }

    // Ref: Section 6.21 - stream access codes (0x01 basic, 0x02 regular, 0x03 extended)
    #[test]
    fn read_device_identification_stream_codes() {
        for code in [READ_DEVICE_ID_BASIC, READ_DEVICE_ID_REGULAR, READ_DEVICE_ID_EXTENDED] {
            let (transport, client) = connected_client();
            transport.queue_response(new_mock_device_identification_response(code));
            let id = client.read_device_identification(&ctx(), code, DeviceIdObjectCode(0)).unwrap();
            assert_eq!(id.read_device_id_code, code);
            assert_eq!(id.objects.len(), 3);
            assert_eq!(transport.get_requests()[0].get_pdu().data, vec![MEI_READ_DEVICE_ID.0, code.0, 0]);
        }
    }

    // Ref: Section 6.2
    #[test]
    fn read_discrete_inputs() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_READ_DISCRETE_INPUTS, vec![1, 0b0000_0101]));
        let v = client.read_discrete_inputs(&ctx(), Address(5), Quantity(3)).unwrap();
        assert_eq!(v, vec![true, false, true]);
        let req = &transport.get_requests()[0];
        assert_eq!(req.get_pdu().function_code, FUNC_READ_DISCRETE_INPUTS);
        assert_eq!(req.get_pdu().data, vec![0, 5, 0, 3]);
    }

    // Ref: Section 6.4
    #[test]
    fn read_input_registers() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_READ_INPUT_REGISTERS, vec![2, 0xAB, 0xCD]));
        let v = client.read_input_registers(&ctx(), Address(8), Quantity(1)).unwrap();
        assert_eq!(v, vec![0xABCD]);
        assert_eq!(transport.get_requests()[0].get_pdu().data, vec![0, 8, 0, 1]);
    }

    // Ref: Section 6.6
    #[test]
    fn write_single_register() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_WRITE_SINGLE_REGISTER, vec![0, 1, 0x12, 0x34]));
        client.write_single_register(&ctx(), Address(1), 0x1234).unwrap();
        let req = &transport.get_requests()[0];
        assert_eq!(req.get_pdu().function_code, FUNC_WRITE_SINGLE_REGISTER);
        assert_eq!(req.get_pdu().data, vec![0, 1, 0x12, 0x34]);
    }

    // Ref: Section 6.11 (Example: 10 coils from 20 -> CD 01)
    #[test]
    fn write_multiple_coils() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_WRITE_MULTIPLE_COILS, vec![0, 19, 0, 10]));
        let values = [true, false, true, true, false, false, true, true, true, false];
        client.write_multiple_coils(&ctx(), Address(19), &values).unwrap();
        let req = &transport.get_requests()[0];
        assert_eq!(req.get_pdu().function_code, FUNC_WRITE_MULTIPLE_COILS);
        assert_eq!(req.get_pdu().data, vec![0, 19, 0, 10, 2, 0xCD, 0x01]);
    }

    // Ref: Section 6.12
    #[test]
    fn write_multiple_registers() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_WRITE_MULTIPLE_REGISTERS, vec![0, 1, 0, 2]));
        client.write_multiple_registers(&ctx(), Address(1), &[0x000A, 0x0102]).unwrap();
        assert_eq!(
            transport.get_requests()[0].get_pdu().data,
            vec![0, 1, 0, 2, 4, 0x00, 0x0A, 0x01, 0x02]
        );
    }

    // Ref: Section 6.17
    #[test]
    fn read_write_multiple_registers() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_READ_WRITE_MULTIPLE_REGISTERS, vec![4, 0, 1, 0, 2]));
        let v = client
            .read_write_multiple_registers(&ctx(), Address(3), Quantity(2), Address(14), &[0x00FF])
            .unwrap();
        assert_eq!(v, vec![1, 2]);
        assert_eq!(
            transport.get_requests()[0].get_pdu().data,
            vec![0, 3, 0, 2, 0, 14, 0, 1, 2, 0x00, 0xFF]
        );
    }

    // Ref: Section 6.7
    #[test]
    fn read_exception_status() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_READ_EXCEPTION_STATUS, vec![0x6D]));
        assert_eq!(client.read_exception_status(&ctx()).unwrap(), ExceptionStatus(0x6D));
        let req = &transport.get_requests()[0];
        assert_eq!(req.get_pdu().function_code, FUNC_READ_EXCEPTION_STATUS);
        assert!(req.get_pdu().data.is_empty());
    }

    // Ref: Section 7 (exception function code = request fc | 0x80, then exception code)
    #[test]
    fn exception_response_becomes_modbus_error() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(
            FunctionCode(FUNC_READ_HOLDING_REGISTERS.0 | EXCEPTION_BIT),
            vec![EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE.0],
        ));
        let err = client.read_holding_registers(&ctx(), Address(0), Quantity(1)).unwrap_err();
        assert!(is_modbus_error(&err));
        assert!(is_exception_error(&err, EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE));
    }

    // Validation errors surface before anything is sent (Ref: Section 6.3, max 125 registers)
    #[test]
    fn invalid_request_is_not_sent() {
        let (transport, client) = connected_client();
        assert!(client.read_holding_registers(&ctx(), Address(0), Quantity(126)).is_err());
        assert!(client.read_coils(&ctx(), Address(0), Quantity(0)).is_err());
        assert!(transport.get_requests().is_empty());
    }

    #[test]
    fn malformed_response_is_parse_error() {
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_READ_HOLDING_REGISTERS, vec![4, 0x12]));
        assert!(client.read_holding_registers(&ctx(), Address(0), Quantity(2)).is_err());
    }

    #[test]
    fn send_applies_default_timeout_only_without_deadline() {
        // The mock ignores the context, so just make sure both paths work.
        let (transport, client) = connected_client();
        transport.queue_response(mock_response(FUNC_READ_EXCEPTION_STATUS, vec![1]));
        let with_deadline = ctx().with_timeout(Duration::from_secs(1));
        assert!(client.read_exception_status(&with_deadline).is_ok());
        transport.queue_response(mock_response(FUNC_READ_EXCEPTION_STATUS, vec![1]));
        let r = client.send(&ctx(), FUNC_READ_EXCEPTION_STATUS, vec![]).unwrap();
        assert_eq!(r.get_pdu().data, vec![1]);
    }
}
