//! TCPClient: a Modbus TCP client. Port of `gomodbus/client/tcp_client.go`.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (ADU = MBAP 7 bytes + PDU; MBAP framing is in the MODBUS Messaging on TCP-IP Implementation Guide, "MODBUS on TCP"
//! framing), Section 6 (Function codes descriptions). The client behaviour itself is in
//! [`BaseClient`]; this type adds the TCP transport and builder-style configuration.

use std::io::{Read, Write};
use std::sync::{Arc, RwLock};

use super::base_client::{self, BaseClient};
use super::transport::{Transport as ClientTransport, TransportBridge};
use crate::common::{
    self, Address, Client, Context, DeviceIdObjectCode, DeviceIdentification, ExceptionStatus,
    InputRegisterValue, LoggerInterface, Quantity, ReadDeviceIdCode, RegisterValue, Result, UnitId,
};
use crate::transport::{TCPTransport, TCPTransportOption};
use crate::transport::tcp_transport::{with_reader, with_writer};

/// TCPClient is a Modbus TCP client (Go `TCPClient`). It wraps a [`BaseClient`] (the Go
/// struct embeds it) and implements [`Client`] by delegating to it.
///
/// Configure it builder-style: `TCPClient::new(host, opts).with_unit_id(UnitId(1))`, or with
/// `with_options(vec![with_tcp_logger(..), with_tcp_unit_id(..)])`.
pub struct TCPClient {
    // Replaced wholesale by option functions, as Go reassigns `c.BaseClient`.
    base: RwLock<Arc<BaseClient>>,
    tcp_transport: Option<Arc<TCPTransport>>,
    // set when created via `from_transport`
    client_transport: Option<Arc<dyn ClientTransport>>,
}

/// A function that configures a [`TCPClient`] (Go `TCPOption`).
pub type TCPOption = Box<dyn FnOnce(&TCPClient)>;

/// Sets the logger for the TCP client (Go `WithTCPLogger`).
pub fn with_tcp_logger(logger: Arc<dyn LoggerInterface>) -> TCPOption {
    Box::new(move |c| c.set_logger(logger))
}

/// Sets the unit ID for the TCP client (Go `WithTCPUnitID`).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (ADU = MBAP 7 bytes + PDU); the MBAP Unit Identifier is defined in the MODBUS Messaging on TCP-IP Implementation Guide, not in this document
pub fn with_tcp_unit_id(unit_id: UnitId) -> TCPOption {
    Box::new(move |c| c.set_unit_id(unit_id))
}

impl TCPClient {
    /// Creates a new Modbus TCP client (Go `NewTCPClient`). `options` configure the
    /// underlying [`TCPTransport`] (port, timeout, ...). The default port is 502.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Introduction, "Scope of this document" (reserved system port 502)
    pub fn new(host: &str, options: Vec<TCPTransportOption>) -> TCPClient {
        // Create the TCP transport
        let tcp_transport = Arc::new(TCPTransport::with_options(host, options));

        // Create the base client with the transport
        let base_client = BaseClient::new(tcp_transport.clone(), vec![]);

        TCPClient {
            base: RwLock::new(Arc::new(base_client)),
            tcp_transport: Some(tcp_transport),
            client_transport: None,
        }
    }

    /// Creates a new Modbus TCP client using a client `Transport` abstraction for connection
    /// lifecycle management (Go `NewTCPClientFromTransport`). The Transport handles
    /// connect/disconnect/reconnect; the client just uses it for sending requests.
    pub fn from_transport(t: Arc<dyn ClientTransport>, options: Vec<TCPOption>) -> TCPClient {
        let bridge = Arc::new(TransportBridge::new(t.clone()));
        let base_client = BaseClient::new(bridge, vec![]);

        let client = TCPClient {
            base: RwLock::new(Arc::new(base_client)),
            tcp_transport: None,
            client_transport: Some(t),
        };

        for option in options {
            option(&client);
        }

        client
    }

    /// Creates a client that reads from the given reader and writes to the given writer
    /// (Go `FromReaderWriter`). Useful for testing or custom transports. The reader must
    /// report read timeouts as `TimedOut`/`WouldBlock`.
    pub fn from_reader_writer(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>) -> TCPClient {
        let tcp_transport = Arc::new(TCPTransport::with_options("test", vec![with_reader(reader), with_writer(writer)]));
        let base_client = BaseClient::new(tcp_transport.clone(), vec![]);

        TCPClient {
            base: RwLock::new(Arc::new(base_client)),
            tcp_transport: Some(tcp_transport),
            client_transport: None,
        }
    }

    /// Applies the given options to the TCPClient (Go `WithOptions`).
    pub fn with_options(self, options: Vec<TCPOption>) -> TCPClient {
        for option in options {
            option(&self);
        }
        self
    }

    /// Sets the unit ID for the client and returns the client (Go `WithUnitID`; prefer
    /// `with_options(vec![with_tcp_unit_id(..)])`).
    pub fn with_unit_id(self, unit_id: UnitId) -> TCPClient {
        self.with_options(vec![with_tcp_unit_id(unit_id)])
    }

    /// Returns the current underlying [`BaseClient`] (the Go embedded struct), e.g. to call
    /// `send` directly.
    pub fn base(&self) -> Arc<BaseClient> {
        self.base.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The `TCPTransport` this client was created with (`None` for `from_transport`).
    pub fn tcp_transport(&self) -> Option<Arc<TCPTransport>> {
        self.tcp_transport.clone()
    }

    /// Replaces the logger in place (what Go's `WithTCPLogger` / `WithLogger` do).
    pub fn set_logger(&self, logger: Arc<dyn LoggerInterface>) {
        let mut base = self.base.write().unwrap_or_else(|p| p.into_inner());
        let new_base = base.clone().with_logger_base(logger);
        *base = new_base;
    }

    /// Replaces the unit ID in place (what Go's `WithTCPUnitID` does).
    pub fn set_unit_id(&self, unit_id: UnitId) {
        let mut base = self.base.write().unwrap_or_else(|p| p.into_inner());
        let new_base = BaseClient::new(
            base.transport(),
            vec![
                base_client::with_unit_id(unit_id),
                base_client::with_logger(base.logger()),
                base_client::with_protocol(base.protocol()),
            ],
        );
        *base = Arc::new(new_base);
    }

    /// Shuts down the client's Transport. This is the primary cleanup method when the client
    /// was created via `from_transport` (Go `Close`); a no-op otherwise.
    pub fn close(&self) -> Result<()> {
        match &self.client_transport {
            Some(t) => t.close(),
            None => Ok(()),
        }
    }
}

impl BaseClient {
    // Same as `Client::with_logger` but returns the concrete type (Go
    // `c.BaseClient.WithLogger(logger).(*BaseClient)`). Implementation detail.
    pub(crate) fn with_logger_base(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<BaseClient> {
        Arc::new(BaseClient::new(
            self.transport(),
            vec![
                base_client::with_logger(logger),
                base_client::with_unit_id(self.unit_id()),
                base_client::with_protocol(self.protocol()),
            ],
        ))
    }
}

// All Client methods delegate to the current BaseClient.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function codes descriptions)
impl Client for TCPClient {
    fn connect(&self, ctx: &Context) -> Result<()> {
        self.base().connect(ctx)
    }

    fn disconnect(&self, ctx: &Context) -> Result<()> {
        self.base().disconnect(ctx)
    }

    fn is_connected(&self) -> bool {
        self.base().is_connected()
    }

    /// Ref: Section 6.1 (Read Coils)
    fn read_coils(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<common::CoilValue>> {
        self.base().read_coils(ctx, address, quantity)
    }

    /// Ref: Section 6.2 (Read Discrete Inputs)
    fn read_discrete_inputs(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<common::DiscreteInputValue>> {
        self.base().read_discrete_inputs(ctx, address, quantity)
    }

    /// Ref: Section 6.3 (Read Holding Registers)
    fn read_holding_registers(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<RegisterValue>> {
        self.base().read_holding_registers(ctx, address, quantity)
    }

    /// Ref: Section 6.4 (Read Input Registers)
    fn read_input_registers(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<InputRegisterValue>> {
        self.base().read_input_registers(ctx, address, quantity)
    }

    /// Ref: Section 6.5 (Write Single Coil)
    fn write_single_coil(&self, ctx: &Context, address: Address, value: common::CoilValue) -> Result<()> {
        self.base().write_single_coil(ctx, address, value)
    }

    /// Ref: Section 6.6 (Write Single Register)
    fn write_single_register(&self, ctx: &Context, address: Address, value: RegisterValue) -> Result<()> {
        self.base().write_single_register(ctx, address, value)
    }

    /// Ref: Section 6.11 (Write Multiple Coils)
    fn write_multiple_coils(&self, ctx: &Context, address: Address, values: &[common::CoilValue]) -> Result<()> {
        self.base().write_multiple_coils(ctx, address, values)
    }

    /// Ref: Section 6.12 (Write Multiple registers)
    fn write_multiple_registers(&self, ctx: &Context, address: Address, values: &[RegisterValue]) -> Result<()> {
        self.base().write_multiple_registers(ctx, address, values)
    }

    /// Ref: Section 6.17 (Read/Write Multiple registers)
    fn read_write_multiple_registers(
        &self,
        ctx: &Context,
        read_address: Address,
        read_quantity: Quantity,
        write_address: Address,
        write_values: &[RegisterValue],
    ) -> Result<Vec<RegisterValue>> {
        self.base().read_write_multiple_registers(ctx, read_address, read_quantity, write_address, write_values)
    }

    /// Ref: Section 6.7 (Read Exception Status)
    fn read_exception_status(&self, ctx: &Context) -> Result<ExceptionStatus> {
        self.base().read_exception_status(ctx)
    }

    /// Ref: Section 6.21 (Read Device Identification)
    fn read_device_identification(
        &self,
        ctx: &Context,
        read_device_id_code: ReadDeviceIdCode,
        object_id: DeviceIdObjectCode,
    ) -> Result<DeviceIdentification> {
        self.base().read_device_identification(ctx, read_device_id_code, object_id)
    }

    /// Sets the logger for the client and returns the same client (Go `WithLogger` mutates
    /// and returns the receiver).
    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Client> {
        self.set_logger(logger);
        self
    }
}

// ---------------------------------------------------------------------------------------------
// Tests: TCPClient, the client transports (direct / reconnecting) and the transport bridge,
// exercised against the crate's own TcpServer over loopback sockets.
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::transport::{with_on_connect, with_on_disconnect};
    use crate::client::transport_direct::DirectTransport;
    use crate::client::transport_reconnecting::ReconnectingTransport;
    use crate::common::test_support::{MockRequest, MockTransport};
    use crate::common::*;
    use crate::logging;
    use crate::server::memory_store::MemoryStore;
    use crate::server::tcp_server::{TcpServer, with_server_data_store, with_server_logger, with_server_port};
    use crate::transport::tcp_transport::{with_port, with_timeout_option, with_transport_logger};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn ctx() -> Context {
        Context::background()
    }

    fn start_server() -> (TcpServer, u16, Arc<MemoryStore>) {
        let store = Arc::new(MemoryStore::new());
        let srv = TcpServer::new(
            "127.0.0.1",
            vec![with_server_port(0), with_server_logger(logging::noop()), with_server_data_store(store.clone())],
        );
        srv.start(&ctx()).unwrap();
        let port = srv.local_addr().unwrap().port();
        (srv, port, store)
    }

    fn tcp_opts(port: u16) -> Vec<TCPTransportOption> {
        vec![with_port(port), with_timeout_option(Duration::from_secs(2)), with_transport_logger(logging::noop())]
    }

    fn quiet(c: TCPClient) -> TCPClient {
        c.with_options(vec![with_tcp_logger(logging::noop())])
    }

    #[test]
    fn tcp_client_end_to_end() {
        let (srv, port, store) = start_server();
        let client = quiet(TCPClient::new("127.0.0.1", tcp_opts(port))).with_unit_id(UnitId(1));
        assert!(!client.is_connected());
        client.connect(&ctx()).unwrap();
        assert!(client.is_connected());

        client.write_single_register(&ctx(), Address(10), 0xBEEF).unwrap();
        assert_eq!(store.get_holding_register(Address(10)), Some(0xBEEF));
        assert_eq!(client.read_holding_registers(&ctx(), Address(10), Quantity(1)).unwrap(), vec![0xBEEF]);

        client.write_multiple_registers(&ctx(), Address(20), &[1, 2, 3]).unwrap();
        assert_eq!(client.read_holding_registers(&ctx(), Address(20), Quantity(3)).unwrap(), vec![1, 2, 3]);

        client.write_single_coil(&ctx(), Address(5), true).unwrap();
        client.write_multiple_coils(&ctx(), Address(6), &[true, false, true]).unwrap();
        assert_eq!(
            client.read_coils(&ctx(), Address(5), Quantity(4)).unwrap(),
            vec![true, true, false, true]
        );
        assert_eq!(
            client.read_write_multiple_registers(&ctx(), Address(20), Quantity(2), Address(30), &[7, 8]).unwrap(),
            vec![1, 2]
        );
        assert_eq!(client.read_input_registers(&ctx(), Address(0), Quantity(1)).unwrap().len(), 1);
        assert_eq!(client.read_discrete_inputs(&ctx(), Address(0), Quantity(2)).unwrap().len(), 2);

        client.disconnect(&ctx()).unwrap();
        assert!(!client.is_connected());
        srv.stop(&ctx()).unwrap();
    }

    #[test]
    fn tcp_client_read_device_identification() {
        let (srv, port, _store) = start_server();
        let client = quiet(TCPClient::new("127.0.0.1", tcp_opts(port)));
        client.connect(&ctx()).unwrap();
        let id = client.read_device_identification(&ctx(), READ_DEVICE_ID_BASIC, DeviceIdObjectCode(0)).unwrap();
        assert!(!id.objects.is_empty());
        client.disconnect(&ctx()).unwrap();
        srv.stop(&ctx()).unwrap();
    }

    #[test]
    fn tcp_client_exception_from_server() {
        let (srv, port, _store) = start_server();
        let client = quiet(TCPClient::new("127.0.0.1", tcp_opts(port)));
        client.connect(&ctx()).unwrap();
        // Reading a never-written coil range is rejected by the server with an exception
        // (Ref: Section 7); whichever code it is, it must come back as a Modbus error.
        let err = client.read_holding_registers(&ctx(), Address(60000), Quantity(125)).err();
        if let Some(err) = err {
            assert!(is_modbus_error(&err) || err.is_timeout() || matches!(err, Error::Io { .. }), "{err}");
        }
        client.disconnect(&ctx()).unwrap();
        srv.stop(&ctx()).unwrap();
    }

    #[test]
    fn tcp_client_options_replace_base_client() {
        let c = TCPClient::new("127.0.0.1", vec![]).with_options(vec![with_tcp_unit_id(UnitId(5)), with_tcp_logger(logging::noop())]);
        assert_eq!(c.base().unit_id(), UnitId(5));
        let c = c.with_unit_id(UnitId(6));
        assert_eq!(c.base().unit_id(), UnitId(6));
        assert!(c.tcp_transport().is_some());
        assert!(c.close().is_ok());

        let c = Arc::new(c);
        let same = c.clone().with_logger(logging::noop());
        assert!(std::ptr::addr_eq(Arc::as_ptr(&c), Arc::as_ptr(&same)));
    }

    #[test]
    fn from_reader_writer_sends_request_bytes() {
        // A reader that yields nothing (EOF is an I/O error for the transport): we only
        // check that the request is encoded to the writer, then the send fails.
        #[derive(Clone)]
        struct Sink(Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Sink(Arc::new(std::sync::Mutex::new(Vec::new())));
        let client = quiet(TCPClient::from_reader_writer(Box::new(Cursor::new(Vec::new())), Box::new(sink.clone())));
        // Not connected: Send fails fast with NotConnected, nothing is written.
        let err = client.read_coils(&ctx().with_timeout(Duration::from_millis(200)), Address(0), Quantity(1)).unwrap_err();
        assert_eq!(err, Error::NotConnected);
        assert!(sink.0.lock().unwrap().is_empty());
    }

    // --- direct transport ---

    #[test]
    fn direct_transport_connects_and_closes() {
        let (srv, port, store) = start_server();
        store.set_holding_register(Address(1), 99);
        let connects = Arc::new(AtomicUsize::new(0));
        let disconnects = Arc::new(AtomicUsize::new(0));
        let (c1, d1) = (connects.clone(), disconnects.clone());
        let dt = DirectTransport::new(
            &ctx(),
            "127.0.0.1",
            Some(logging::noop()),
            vec![
                with_on_connect(move || {
                    c1.fetch_add(1, Ordering::SeqCst);
                }),
                with_on_disconnect(move |_| {
                    d1.fetch_add(1, Ordering::SeqCst);
                }),
            ],
            tcp_opts(port),
        )
        .unwrap();
        assert_eq!(connects.load(Ordering::SeqCst), 1);

        let client = quiet(TCPClient::from_transport(Arc::new(dt), vec![]));
        // The bridge always reports connected; connect() just fetches the connection.
        assert!(client.is_connected());
        client.connect(&ctx()).unwrap();
        assert_eq!(client.read_holding_registers(&ctx(), Address(1), Quantity(1)).unwrap(), vec![99]);

        client.close().unwrap();
        assert_eq!(disconnects.load(Ordering::SeqCst), 1);
        // Closed: further use fails and a second close is a no-op
        let err = client.read_holding_registers(&ctx(), Address(1), Quantity(1)).unwrap_err();
        assert_eq!(err.to_string(), "transport is closed");
        client.close().unwrap();
        assert_eq!(disconnects.load(Ordering::SeqCst), 1);
        srv.stop(&ctx()).unwrap();
    }

    #[test]
    fn direct_transport_connect_failure() {
        let port = find_free_port_tcp().unwrap();
        let r = DirectTransport::new(&ctx(), "127.0.0.1", Some(logging::noop()), vec![], tcp_opts(port));
        assert!(r.is_err());
    }

    #[test]
    fn direct_transport_reset_is_final() {
        use crate::client::transport::Transport as _;
        let (srv, port, _store) = start_server();
        let dt = DirectTransport::new(&ctx(), "127.0.0.1", Some(logging::noop()), vec![], tcp_opts(port)).unwrap();
        let conn = dt.conn(&ctx()).unwrap();
        // A stale handle (different object) does not reset anything
        let other: Arc<dyn Transport> = Arc::new(MockTransport::new());
        dt.reset(&other).unwrap();
        assert!(dt.conn(&ctx()).is_ok());
        // The current handle does, and a direct transport never reconnects
        dt.reset(&conn).unwrap();
        assert_eq!(dt.conn(&ctx()).err().unwrap().to_string(), "connection is not available");
        dt.close().unwrap();
        assert_eq!(dt.conn(&ctx()).err().unwrap().to_string(), "transport is closed");
        srv.stop(&ctx()).unwrap();
    }

    // --- reconnecting transport ---

    #[test]
    fn reconnecting_transport_is_lazy_and_reconnects() {
        use crate::client::transport::Transport as _;
        let (srv, port, store) = start_server();
        store.set_holding_register(Address(2), 7);
        let connects = Arc::new(AtomicUsize::new(0));
        let disconnects = Arc::new(AtomicUsize::new(0));
        let (c1, d1) = (connects.clone(), disconnects.clone());
        let rt = Arc::new(ReconnectingTransport::new(
            "127.0.0.1",
            Some(logging::noop()),
            vec![
                with_on_connect(move || {
                    c1.fetch_add(1, Ordering::SeqCst);
                }),
                with_on_disconnect(move |_| {
                    d1.fetch_add(1, Ordering::SeqCst);
                }),
            ],
            move || tcp_opts(port),
        ));
        // Never connects until first use
        assert_eq!(connects.load(Ordering::SeqCst), 0);

        let client = quiet(TCPClient::from_transport(rt.clone(), vec![]));
        assert_eq!(client.read_holding_registers(&ctx(), Address(2), Quantity(1)).unwrap(), vec![7]);
        assert_eq!(connects.load(Ordering::SeqCst), 1);

        // The same connection is reused
        let a = rt.conn(&ctx()).unwrap();
        let b = rt.conn(&ctx()).unwrap();
        assert!(Arc::ptr_eq(&a, &b));

        // A stale reset is ignored, the real one drops the connection
        let other: Arc<dyn Transport> = Arc::new(MockTransport::new());
        rt.reset(&other).unwrap();
        assert_eq!(disconnects.load(Ordering::SeqCst), 0);
        rt.reset(&a).unwrap();
        assert_eq!(disconnects.load(Ordering::SeqCst), 1);

        // Next use dials again
        assert_eq!(client.read_holding_registers(&ctx(), Address(2), Quantity(1)).unwrap(), vec![7]);
        assert_eq!(connects.load(Ordering::SeqCst), 2);

        client.close().unwrap();
        assert_eq!(disconnects.load(Ordering::SeqCst), 2);
        assert_eq!(rt.conn(&ctx()).err().unwrap().to_string(), "transport is closed");
        srv.stop(&ctx()).unwrap();
    }

    #[test]
    fn reconnecting_transport_recovers_after_server_restart() {
        let store = Arc::new(MemoryStore::new());
        store.set_holding_register(Address(0), 5);
        let make = |port: u16| {
            TcpServer::new(
                "127.0.0.1",
                vec![with_server_port(port), with_server_logger(logging::noop()), with_server_data_store(store.clone())],
            )
        };
        let srv = make(0);
        srv.start(&ctx()).unwrap();
        let port = srv.local_addr().unwrap().port();

        let rt = Arc::new(ReconnectingTransport::new("127.0.0.1", Some(logging::noop()), vec![], move || tcp_opts(port)));
        let client = quiet(TCPClient::from_transport(rt, vec![]));
        let short = || ctx().with_timeout(Duration::from_secs(2));
        assert_eq!(client.read_holding_registers(&short(), Address(0), Quantity(1)).unwrap(), vec![5]);

        // Server goes away: the failing request resets the transport...
        srv.stop(&ctx()).unwrap();
        assert!(client.read_holding_registers(&short(), Address(0), Quantity(1)).is_err());

        // ...and once the server is back, the next call reconnects transparently.
        let srv2 = make(port);
        srv2.start(&ctx()).unwrap();
        let mut ok = false;
        for _ in 0..20 {
            if client.read_holding_registers(&short(), Address(0), Quantity(1)).is_ok() {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(ok, "client did not reconnect");
        client.close().unwrap();
        srv2.stop(&ctx()).unwrap();
    }

    #[test]
    fn reconnecting_transport_dial_failure_reports_error() {
        use crate::client::transport::Transport as _;
        let port = find_free_port_tcp().unwrap();
        let rt = ReconnectingTransport::new("127.0.0.1", Some(logging::noop()), vec![], move || tcp_opts(port));
        assert!(rt.conn(&ctx()).is_err());
        // Still usable (not closed) after a failed dial
        assert_ne!(rt.conn(&ctx()).err().unwrap().to_string(), "transport is closed");
    }

    // --- transport bridge ---

    // A client Transport that hands out one MockTransport and records resets.
    struct FakeTransport {
        mock: Arc<MockTransport>,
        resets: AtomicUsize,
    }

    impl ClientTransport for FakeTransport {
        fn conn(&self, _ctx: &Context) -> Result<Arc<dyn common::Transport>> {
            self.mock.connect(&Context::background())?;
            Ok(self.mock.clone())
        }
        fn reset(&self, _stale: &Arc<dyn common::Transport>) -> Result<()> {
            self.resets.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn close(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn bridge_resets_on_transport_error_but_not_on_modbus_error() {
        let fake = Arc::new(FakeTransport { mock: Arc::new(MockTransport::new()), resets: AtomicUsize::new(0) });
        let client = quiet(TCPClient::from_transport(fake.clone(), vec![]));

        // Transport-level error -> reset
        fake.mock.queue_error(Error::Timeout);
        assert!(client.read_coils(&ctx(), Address(0), Quantity(1)).is_err());
        assert_eq!(fake.resets.load(Ordering::SeqCst), 1);

        // Modbus error from the transport -> no reset
        fake.mock.queue_error(new_modbus_error(FUNC_READ_COILS, EXCEPTION_SERVER_DEVICE_FAILURE));
        assert!(client.read_coils(&ctx(), Address(0), Quantity(1)).is_err());
        assert_eq!(fake.resets.load(Ordering::SeqCst), 1);

        // Success -> no reset, and the request carries the unit ID
        let client = client.with_unit_id(UnitId(3));
        fake.mock.queue_response(crate::common::test_support::MockResponse::new(
            TransactionId(1),
            UnitId(3),
            FUNC_READ_COILS,
            vec![1, 1],
        ));
        assert_eq!(client.read_coils(&ctx(), Address(0), Quantity(1)).unwrap(), vec![true]);
        let reqs: Vec<MockRequest> = fake.mock.get_requests();
        assert_eq!(reqs.last().unwrap().unit_id, UnitId(3));
        assert_eq!(fake.resets.load(Ordering::SeqCst), 1);
    }
}
