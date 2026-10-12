//! Test doubles. Port of `gomodbus/common/test` (`mock_messages.go`,
//! `mock_transport.go`, `mock_datastore.go`, `device_mock.go`).
//!
//! Implementation detail (not spec), but intentionally a plain `pub mod` (not
//! `cfg(test)`) so integration tests under `tests/` can use the mocks too.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use super::context::Context;
use super::errors::{Error, ModbusError, Result};
use super::logger::LoggerInterface;
use super::pdu::PDU;
use super::request::Request;
use super::response::Response;
use super::server::DataStore;
use super::transport::Transport;
use super::types::*;

// Encodes MBAP header + PDU for the mocks.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP header: Transaction Id 2,
// Protocol Id 2, Length 2, Unit Id 1; then Function Code + data).
// Note: the Go mock allocated 7+len(data) bytes yet wrote the function code at index 7
// (out of range for empty data, truncating the last data byte otherwise); here the
// buffer is sized 8+len(data) so the frame is complete.
fn encode_mock_frame(transaction_id: TransactionId, unit_id: UnitId, pdu: &PDU) -> Vec<u8> {
    let mut result = Vec::with_capacity(TCP_HEADER_LENGTH + 1 + pdu.data.len());
    // Transaction ID (2 bytes)
    result.extend_from_slice(&transaction_id.0.to_be_bytes());
    // Protocol ID (2 bytes) - always 0 for Modbus TCP
    result.extend_from_slice(&TCP_PROTOCOL_IDENTIFIER.0.to_be_bytes());
    // Length (2 bytes) - unit ID + function code + data
    let length = (1 + 1 + pdu.data.len()) as u16;
    result.extend_from_slice(&length.to_be_bytes());
    // Unit ID (1 byte)
    result.push(unit_id.0);
    // Function code (1 byte)
    result.push(pdu.function_code.0);
    // Data
    result.extend_from_slice(&pdu.data);
    result
}

/// MockRequest implements [`Request`] for testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockRequest {
    /// MBAP transaction id.
    pub transaction_id: TransactionId,
    /// MBAP unit id.
    pub unit_id: UnitId,
    /// The request PDU.
    pub pdu: PDU,
}

impl MockRequest {
    /// Creates a mock request (Go `NewMockRequest`).
    pub fn new(transaction_id: TransactionId, unit_id: UnitId, function_code: FunctionCode, data: Vec<u8>) -> MockRequest {
        MockRequest { transaction_id, unit_id, pdu: PDU { function_code, data } }
    }
}

impl Request for MockRequest {
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

    fn encode(&self) -> Result<Vec<u8>> {
        // Simple encoding for testing
        Ok(encode_mock_frame(self.transaction_id, self.unit_id, &self.pdu))
    }
}

/// MockResponse implements [`Response`] for testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockResponse {
    /// MBAP transaction id.
    pub transaction_id: TransactionId,
    /// MBAP unit id.
    pub unit_id: UnitId,
    /// The response PDU.
    pub pdu: PDU,
}

impl MockResponse {
    /// Creates a mock response (Go `NewMockResponse`).
    pub fn new(transaction_id: TransactionId, unit_id: UnitId, function_code: FunctionCode, data: Vec<u8>) -> MockResponse {
        MockResponse { transaction_id, unit_id, pdu: PDU { function_code, data } }
    }
}

impl Response for MockResponse {
    fn get_transaction_id(&self) -> TransactionId {
        self.transaction_id
    }

    fn get_unit_id(&self) -> UnitId {
        self.unit_id
    }

    fn get_pdu(&self) -> &PDU {
        &self.pdu
    }

    // Ref: Section 7 (exception function code = fc | 0x80)
    fn is_exception(&self) -> bool {
        is_function_exception(self.pdu.function_code)
    }

    // Ref: Section 7 (the exception code is the first data byte)
    fn get_exception(&self) -> ExceptionCode {
        if !self.is_exception() {
            return ExceptionCode(0);
        }
        match self.pdu.data.first() {
            Some(b) => ExceptionCode(*b),
            None => ExceptionCode(0),
        }
    }

    fn to_error(&self) -> Option<Error> {
        if !self.is_exception() {
            return None;
        }
        Some(Error::Modbus(ModbusError::new(self.pdu.function_code, self.get_exception())))
    }

    fn encode(&self) -> Result<Vec<u8>> {
        // Simple encoding for testing
        Ok(encode_mock_frame(self.transaction_id, self.unit_id, &self.pdu))
    }
}

/// Creates a mock response for a read device identification request
/// (Go `NewMockDeviceIdentificationResponse`).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Read Device Identification, Response PDU)
pub fn new_mock_device_identification_response(read_device_id_code: ReadDeviceIdCode) -> MockResponse {
    // Sample data: MEI type, code, conformity level, more follows, next object id, 3 objects
    let mut response_data = vec![
        MEI_READ_DEVICE_ID.0,   // MEI type
        read_device_id_code.0,  // Read device ID code
        0x01,                   // Conformity level
        0x00,                   // More follows
        0x00,                   // Next object ID
        0x03,                   // Number of objects
        // Object 1: Vendor name
        DEVICE_ID_VENDOR_NAME.0, // ID
        0x09,                    // Length
    ];
    response_data.extend_from_slice(b"Acme Inc."); // Value
    // Object 2: Product code
    response_data.extend_from_slice(&[DEVICE_ID_PRODUCT_CODE.0, 0x06]);
    response_data.extend_from_slice(b"ABC123");
    // Object 3: Revision
    response_data.extend_from_slice(&[DEVICE_ID_MAJOR_MINOR_REVISION.0, 0x04]);
    response_data.extend_from_slice(b"V1.0");

    MockResponse::new(TransactionId(1), UnitId(1), FUNC_READ_DEVICE_IDENTIFICATION, response_data)
}

// Shared mutable state of a MockTransport.
#[derive(Default)]
struct MockTransportState {
    connected: bool,
    response_queue: Vec<Box<dyn Response>>,
    requests: Vec<MockRequest>,
    error_queue: Vec<Error>,
}

/// MockTransport implements [`Transport`] for testing: records requests and
/// answers from queued errors first, then queued responses.
///
/// `with_logger` returns a transport that shares this one's queues and request
/// log (Go copied the slice headers, so they initially aliased as well).
pub struct MockTransport {
    state: Arc<Mutex<MockTransportState>>,
    logger: Option<Arc<dyn LoggerInterface>>,
}

impl Default for MockTransport {
    fn default() -> Self {
        MockTransport::new()
    }
}

impl MockTransport {
    /// Creates a new, disconnected mock transport (Go `NewMockTransport`).
    pub fn new() -> MockTransport {
        MockTransport { state: Arc::new(Mutex::new(MockTransportState::default())), logger: None }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MockTransportState> {
        match self.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// Adds a response to the queue.
    pub fn queue_response(&self, response: impl Response + 'static) {
        self.lock().response_queue.push(Box::new(response));
    }

    /// Adds an error to the queue (errors are served before responses).
    pub fn queue_error(&self, err: Error) {
        self.lock().error_queue.push(err);
    }

    /// Returns a snapshot of the requests received so far.
    pub fn get_requests(&self) -> Vec<MockRequest> {
        self.lock().requests.clone()
    }

    /// Clears all queues and the request log.
    pub fn clear(&self) {
        let mut s = self.lock();
        s.response_queue.clear();
        s.requests.clear();
        s.error_queue.clear();
    }

    /// The logger set via `with_logger`, if any.
    pub fn logger(&self) -> Option<Arc<dyn LoggerInterface>> {
        self.logger.clone()
    }
}

impl Transport for MockTransport {
    fn connect(&self, _ctx: &Context) -> Result<()> {
        self.lock().connected = true;
        Ok(())
    }

    fn disconnect(&self, _ctx: &Context) -> Result<()> {
        self.lock().connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.lock().connected
    }

    fn send(&self, _ctx: &Context, request: &mut dyn Request) -> Result<Box<dyn Response>> {
        let mut s = self.lock();

        // Check if connected
        if !s.connected {
            return Err(Error::NotConnected);
        }

        // Record the request
        s.requests.push(MockRequest {
            transaction_id: request.get_transaction_id(),
            unit_id: request.get_unit_id(),
            pdu: request.get_pdu().clone(),
        });

        // Return the next queued error or response
        if !s.error_queue.is_empty() {
            return Err(s.error_queue.remove(0));
        }
        if !s.response_queue.is_empty() {
            return Ok(s.response_queue.remove(0));
        }

        Err(Error::NoResponse)
    }

    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Transport> {
        Arc::new(MockTransport { state: Arc::clone(&self.state), logger: Some(logger) })
    }
}

// Mutable contents of the MockDataStore.
#[derive(Default)]
struct MockDataStoreInner {
    coils: HashMap<Address, CoilValue>,
    discrete_inputs: HashMap<Address, DiscreteInputValue>,
    holding_registers: HashMap<Address, RegisterValue>,
    input_registers: HashMap<Address, InputRegisterValue>,
    fail_flag: bool,
    fail_on_address: Option<Address>,
    fail_on_quantity: Option<Quantity>,
}

impl MockDataStoreInner {
    // Shared failure injection for reads/writes (Go repeated this in every method).
    // `check_quantity` is None for single-item writes.
    fn check(&self, address: Address, quantity: Option<Quantity>) -> Result<()> {
        if self.fail_flag {
            return Err(Error::ServerDeviceFailure);
        }
        if self.fail_on_address == Some(address) {
            return Err(Error::ServerDeviceFailure);
        }
        if let Some(q) = quantity
            && self.fail_on_quantity == Some(q)
        {
            return Err(Error::InvalidQuantity);
        }
        Ok(())
    }
}

/// MockDataStore implements [`DataStore`] for testing, with failure injection.
///
/// Unset coils/inputs read as `false`, unset registers as `0`. Quantity limits
/// follow Go: coils/discrete inputs 1..=2000 (read) and 1..=2000 (write);
/// registers 1..=125 (read and write).
#[derive(Default)]
pub struct MockDataStore {
    inner: RwLock<MockDataStoreInner>,
}

impl MockDataStore {
    /// Creates a new mock data store (Go `NewMockDataStore`).
    pub fn new() -> MockDataStore {
        MockDataStore::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, MockDataStoreInner> {
        match self.inner.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, MockDataStoreInner> {
        match self.inner.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// Sets the fail flag: every operation returns `ServerDeviceFailure`.
    pub fn set_fail(&self, fail: bool) {
        self.write().fail_flag = fail;
    }

    /// Sets an address that will cause operations to fail with `ServerDeviceFailure`.
    pub fn set_fail_on_address(&self, address: Address) {
        self.write().fail_on_address = Some(address);
    }

    /// Clears the fail-on-address setting.
    pub fn clear_fail_on_address(&self) {
        self.write().fail_on_address = None;
    }

    /// Sets a quantity that will cause operations to fail with `InvalidQuantity`.
    pub fn set_fail_on_quantity(&self, quantity: Quantity) {
        self.write().fail_on_quantity = Some(quantity);
    }

    /// Clears the fail-on-quantity setting.
    pub fn clear_fail_on_quantity(&self) {
        self.write().fail_on_quantity = None;
    }

    /// Sets a coil value directly (for test setup).
    pub fn set_coil(&self, address: Address, value: CoilValue) {
        self.write().coils.insert(address, value);
    }

    /// Sets a discrete input value directly (for test setup).
    pub fn set_discrete_input(&self, address: Address, value: DiscreteInputValue) {
        self.write().discrete_inputs.insert(address, value);
    }

    /// Sets a holding register value directly (for test setup).
    pub fn set_holding_register(&self, address: Address, value: RegisterValue) {
        self.write().holding_registers.insert(address, value);
    }

    /// Sets an input register value directly (for test setup).
    pub fn set_input_register(&self, address: Address, value: InputRegisterValue) {
        self.write().input_registers.insert(address, value);
    }

    /// Gets a coil value directly (for verification); `None` if never set.
    pub fn get_coil(&self, address: Address) -> Option<CoilValue> {
        self.read().coils.get(&address).copied()
    }

    /// Gets a discrete input value directly; `None` if never set.
    pub fn get_discrete_input(&self, address: Address) -> Option<DiscreteInputValue> {
        self.read().discrete_inputs.get(&address).copied()
    }

    /// Gets a holding register value directly; `None` if never set.
    pub fn get_holding_register(&self, address: Address) -> Option<RegisterValue> {
        self.read().holding_registers.get(&address).copied()
    }

    /// Gets an input register value directly; `None` if never set.
    pub fn get_input_register(&self, address: Address) -> Option<InputRegisterValue> {
        self.read().input_registers.get(&address).copied()
    }
}

impl DataStore for MockDataStore {
    // Ref: Section 6.1 (quantity 1..=2000)
    fn read_coils(&self, _ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<CoilValue>> {
        let ds = self.read();
        ds.check(address, Some(quantity))?;
        if quantity.0 == 0 || quantity.0 > MAX_COIL_COUNT {
            return Err(Error::InvalidQuantity);
        }
        Ok((0..quantity.0).map(|i| ds.coils.get(&(address + i)).copied().unwrap_or(false)).collect())
    }

    // Ref: Section 6.2 (quantity 1..=2000)
    fn read_discrete_inputs(&self, _ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<DiscreteInputValue>> {
        let ds = self.read();
        ds.check(address, Some(quantity))?;
        if quantity.0 == 0 || quantity.0 > MAX_COIL_COUNT {
            return Err(Error::InvalidQuantity);
        }
        Ok((0..quantity.0).map(|i| ds.discrete_inputs.get(&(address + i)).copied().unwrap_or(false)).collect())
    }

    // Ref: Section 6.3 (quantity 1..=125)
    fn read_holding_registers(&self, _ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<RegisterValue>> {
        let ds = self.read();
        ds.check(address, Some(quantity))?;
        if quantity.0 == 0 || quantity.0 > MAX_REGISTER_COUNT {
            return Err(Error::InvalidQuantity);
        }
        Ok((0..quantity.0).map(|i| ds.holding_registers.get(&(address + i)).copied().unwrap_or(0)).collect())
    }

    // Ref: Section 6.4 (quantity 1..=125)
    fn read_input_registers(&self, _ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<InputRegisterValue>> {
        let ds = self.read();
        ds.check(address, Some(quantity))?;
        if quantity.0 == 0 || quantity.0 > MAX_REGISTER_COUNT {
            return Err(Error::InvalidQuantity);
        }
        Ok((0..quantity.0).map(|i| ds.input_registers.get(&(address + i)).copied().unwrap_or(0)).collect())
    }

    // Ref: Section 6.5
    fn write_single_coil(&self, _ctx: &Context, address: Address, value: CoilValue) -> Result<()> {
        let mut ds = self.write();
        ds.check(address, None)?;
        ds.coils.insert(address, value);
        Ok(())
    }

    // Ref: Section 6.6
    fn write_single_register(&self, _ctx: &Context, address: Address, value: RegisterValue) -> Result<()> {
        let mut ds = self.write();
        ds.check(address, None)?;
        ds.holding_registers.insert(address, value);
        Ok(())
    }

    // Ref: Section 6.11 (Go's mock validates against MaxCoilCount, not the 1968 write limit)
    fn write_multiple_coils(&self, _ctx: &Context, address: Address, values: &[CoilValue]) -> Result<()> {
        let mut ds = self.write();
        let quantity = Quantity(values.len().min(u16::MAX as usize) as u16);
        ds.check(address, Some(quantity))?;
        if values.is_empty() || values.len() > MAX_COIL_COUNT as usize {
            return Err(Error::InvalidQuantity);
        }
        for (i, v) in values.iter().enumerate() {
            ds.coils.insert(address + i as u16, *v);
        }
        Ok(())
    }

    // Ref: Section 6.12 (Go's mock validates against MaxRegisterCount, not the 123 write limit)
    fn write_multiple_registers(&self, _ctx: &Context, address: Address, values: &[RegisterValue]) -> Result<()> {
        let mut ds = self.write();
        let quantity = Quantity(values.len().min(u16::MAX as usize) as u16);
        ds.check(address, Some(quantity))?;
        if values.is_empty() || values.len() > MAX_REGISTER_COUNT as usize {
            return Err(Error::InvalidQuantity);
        }
        for (i, v) in values.iter().enumerate() {
            ds.holding_registers.insert(address + i as u16, *v);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_request_encode_and_set_id() {
        let mut r = MockRequest::new(TransactionId(0x0102), UnitId(7), FUNC_READ_COILS, vec![0, 1, 0, 2]);
        assert_eq!(r.encode().unwrap(), vec![1, 2, 0, 0, 0, 6, 7, 1, 0, 1, 0, 2]);
        r.set_transaction_id(TransactionId(5));
        assert_eq!(r.get_transaction_id(), TransactionId(5));
        // empty data no longer panics
        let e = MockRequest::new(TransactionId(1), UnitId(1), FUNC_READ_EXCEPTION_STATUS, vec![]);
        assert_eq!(e.encode().unwrap().len(), 8);
    }

    #[test]
    fn mock_response_exception() {
        let ok = MockResponse::new(TransactionId(1), UnitId(1), FUNC_READ_COILS, vec![1, 0]);
        assert!(!ok.is_exception());
        assert!(ok.to_error().is_none());
        assert_eq!(ok.get_exception(), ExceptionCode(0));

        let ex = MockResponse::new(TransactionId(1), UnitId(1), FunctionCode(0x81), vec![0x02]);
        assert!(ex.is_exception());
        assert_eq!(ex.get_exception(), EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE);
        match ex.to_error() {
            Some(Error::Modbus(e)) => {
                assert_eq!(e.function_code, FunctionCode(0x81));
                assert_eq!(e.exception_code, EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE);
            }
            other => panic!("unexpected {:?}", other),
        }
        let empty = MockResponse::new(TransactionId(1), UnitId(1), FunctionCode(0x81), vec![]);
        assert_eq!(empty.get_exception(), ExceptionCode(0));
    }

    #[test]
    fn mock_transport_flow() {
        let t = MockTransport::new();
        let ctx = Context::background();
        let mut req = MockRequest::new(TransactionId(1), UnitId(1), FUNC_READ_COILS, vec![0, 0, 0, 1]);

        // Not connected
        assert_eq!(t.send(&ctx, &mut req).err(), Some(Error::NotConnected));
        assert!(!t.is_connected());
        t.connect(&ctx).unwrap();
        assert!(t.is_connected());

        // Nothing queued
        assert_eq!(t.send(&ctx, &mut req).err(), Some(Error::NoResponse));

        // Errors are served before responses
        t.queue_response(MockResponse::new(TransactionId(1), UnitId(1), FUNC_READ_COILS, vec![1, 1]));
        t.queue_error(Error::Timeout);
        assert_eq!(t.send(&ctx, &mut req).err(), Some(Error::Timeout));
        let resp = t.send(&ctx, &mut req).unwrap();
        assert_eq!(resp.get_pdu().data, vec![1, 1]);
        assert_eq!(t.get_requests().len(), 3);
        assert_eq!(t.get_requests()[0].pdu.function_code, FUNC_READ_COILS);

        t.clear();
        assert!(t.get_requests().is_empty());
        t.disconnect(&ctx).unwrap();
        assert!(!t.is_connected());
    }

    #[test]
    fn mock_transport_with_logger_shares_state() {
        let t = Arc::new(MockTransport::new());
        let t2 = Arc::clone(&t).with_logger(crate::logging::noop());
        t2.connect(&Context::background()).unwrap();
        assert!(t.is_connected());
    }

    #[test]
    fn mock_device_identification_response_parses() {
        use crate::common::client::Protocol;
        let resp = new_mock_device_identification_response(READ_DEVICE_ID_BASIC);
        let h = crate::protocol::ProtocolHandler::new_with_logger(crate::logging::noop());
        let d = h.parse_read_device_identification_response(&resp.get_pdu().data).unwrap();
        assert_eq!(d.get_vendor_name(), "Acme Inc.");
        assert_eq!(d.get_product_code(), "ABC123");
        assert_eq!(d.get_revision(), "V1.0");
    }

    #[test]
    fn mock_datastore_reads_writes() {
        let ds = MockDataStore::new();
        let ctx = Context::background();
        ds.set_coil(Address(1), true);
        ds.set_discrete_input(Address(2), true);
        ds.set_holding_register(Address(3), 33);
        ds.set_input_register(Address(4), 44);
        assert_eq!(ds.read_coils(&ctx, Address(0), Quantity(3)).unwrap(), vec![false, true, false]);
        assert_eq!(ds.read_discrete_inputs(&ctx, Address(2), Quantity(2)).unwrap(), vec![true, false]);
        assert_eq!(ds.read_holding_registers(&ctx, Address(2), Quantity(2)).unwrap(), vec![0, 33]);
        assert_eq!(ds.read_input_registers(&ctx, Address(4), Quantity(1)).unwrap(), vec![44]);

        ds.write_single_coil(&ctx, Address(10), true).unwrap();
        assert_eq!(ds.get_coil(Address(10)), Some(true));
        ds.write_single_register(&ctx, Address(11), 7).unwrap();
        assert_eq!(ds.get_holding_register(Address(11)), Some(7));
        ds.write_multiple_coils(&ctx, Address(20), &[true, false, true]).unwrap();
        assert_eq!(ds.get_coil(Address(22)), Some(true));
        ds.write_multiple_registers(&ctx, Address(30), &[1, 2]).unwrap();
        assert_eq!(ds.get_holding_register(Address(31)), Some(2));
        assert_eq!(ds.get_discrete_input(Address(2)), Some(true));
        assert_eq!(ds.get_input_register(Address(99)), None);
    }

    #[test]
    fn mock_datastore_validation_and_failures() {
        let ds = MockDataStore::new();
        let ctx = Context::background();
        assert_eq!(ds.read_coils(&ctx, Address(0), Quantity(0)), Err(Error::InvalidQuantity));
        assert_eq!(ds.read_coils(&ctx, Address(0), Quantity(2001)), Err(Error::InvalidQuantity));
        assert_eq!(ds.read_holding_registers(&ctx, Address(0), Quantity(126)), Err(Error::InvalidQuantity));
        assert_eq!(ds.write_multiple_registers(&ctx, Address(0), &[]), Err(Error::InvalidQuantity));
        assert_eq!(ds.write_multiple_coils(&ctx, Address(0), &[]), Err(Error::InvalidQuantity));

        ds.set_fail(true);
        assert_eq!(ds.read_coils(&ctx, Address(0), Quantity(1)), Err(Error::ServerDeviceFailure));
        assert_eq!(ds.write_single_coil(&ctx, Address(0), true), Err(Error::ServerDeviceFailure));
        ds.set_fail(false);

        ds.set_fail_on_address(Address(5));
        assert_eq!(ds.read_input_registers(&ctx, Address(5), Quantity(1)), Err(Error::ServerDeviceFailure));
        assert_eq!(ds.write_single_register(&ctx, Address(5), 1), Err(Error::ServerDeviceFailure));
        assert!(ds.read_input_registers(&ctx, Address(6), Quantity(1)).is_ok());
        ds.clear_fail_on_address();
        assert!(ds.read_input_registers(&ctx, Address(5), Quantity(1)).is_ok());

        ds.set_fail_on_quantity(Quantity(3));
        assert_eq!(ds.read_coils(&ctx, Address(0), Quantity(3)), Err(Error::InvalidQuantity));
        assert_eq!(ds.write_multiple_registers(&ctx, Address(0), &[1, 2, 3]), Err(Error::InvalidQuantity));
        ds.clear_fail_on_quantity();
        assert!(ds.read_coils(&ctx, Address(0), Quantity(3)).is_ok());
    }
}
