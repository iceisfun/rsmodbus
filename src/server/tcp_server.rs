//! Modbus TCP server. Port of `gomodbus/server/tcp_server.go`.
//!
//! Implements the Modbus TCP server side: accepts connections, reads MBAP-framed
//! requests, dispatches them by function code and writes the responses.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4 (Modbus Protocol Description)
//! Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3 (Modbus TCP/IP Protocol)
//!
//! Threading model (implementation detail, not spec): one accept thread plus one
//! thread per connection. `stop` closes the listener and all client sockets and
//! joins every thread. Go's goroutines/`context.Context` map to `std::thread` and
//! [`Context`].

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use super::connected_client::{ClientConn, ConnectedClient};
use super::memory_store::MemoryStore;
use super::protocol_handler::{ServerProtocolHandler, ServerRequest, ServerResponse};
use crate::common::*;
use crate::logging;

// Implementation detail (not spec): how long a blocked accept/read sleeps before
// re-checking the stop flag, and the Go read deadline.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Client lifecycle callback (Go `func(ConnectedClient)`).
pub type ClientCallback = Arc<dyn Fn(ConnectedClient) + Send + Sync>;

// Mutable server configuration/state guarded by one lock (Go `mutex`).
struct State {
    // Server binding configuration
    address: String,
    port: u16,
    listener: Option<TcpListener>,
    // Function code handlers map. Ref: Section 6 (Function Codes)
    handlers: HashMap<FunctionCode, HandlerFunc>,
    // Data storage
    default_store: Arc<dyn DataStore>,
    // Server state
    running: bool,
    logger: Arc<dyn LoggerInterface>,
    stop: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
    // Client lifecycle callbacks
    on_client_connect: Option<ClientCallback>,
    on_client_disconnect: Option<ClientCallback>,
}

// State shared between the `TcpServer` handle and its worker threads.
struct Shared {
    state: RwLock<State>,
    // Tracked connections keyed by remote address (Go `clients` + `clientsMutex`)
    clients: RwLock<HashMap<String, Arc<ClientConn>>>,
    // Per-connection threads, joined on stop
    client_threads: Mutex<Vec<JoinHandle<()>>>,
    // Protocol handler for processing requests
    protocol: ServerProtocolHandler,
}

// Implementation detail: lock helpers recovering from poisoning (Go mutexes cannot poison).
fn rd<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}
fn wr<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn logger(&self) -> Arc<dyn LoggerInterface> {
        rd(&self.state).logger.clone()
    }
}

/// TcpServer implements a Modbus TCP server (Go `TCPServer`).
///
/// Wrap it in an `Arc` to use it as an `Arc<dyn Server>`.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4 (Modbus Protocol Description)
/// Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3 (Modbus TCP/IP Protocol)
pub struct TcpServer {
    shared: Arc<Shared>,
}

/// TcpServerOption configures a [`TcpServer`] (Go `TCPServerOption`).
pub type TcpServerOption = Box<dyn FnOnce(&mut TcpServer)>;

/// Sets the TCP port for the server (Go `WithServerPort`).
/// Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3 (well-known port 502)
pub fn with_server_port(port: u16) -> TcpServerOption {
    Box::new(move |s| wr(&s.shared.state).port = port)
}

/// Sets the logger for the TCP server (Go `WithServerLogger`). Implementation detail, not spec.
pub fn with_server_logger(logger: Arc<dyn LoggerInterface>) -> TcpServerOption {
    Box::new(move |s| wr(&s.shared.state).logger = logger)
}

/// Sets the data store for the TCP server (Go `WithServerDataStore`).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (MODBUS Data model)
pub fn with_server_data_store(store: Arc<dyn DataStore>) -> TcpServerOption {
    Box::new(move |s| wr(&s.shared.state).default_store = store)
}

/// Sets a pre-configured listener for the server (Go `WithServerListener`).
/// This avoids the TOCTOU race of `find_free_port_tcp` where the port could be
/// taken between finding it and binding to it.
pub fn with_server_listener(listener: TcpListener) -> TcpServerOption {
    Box::new(move |s| {
        let mut st = wr(&s.shared.state);
        if let Ok(addr) = listener.local_addr() {
            st.port = addr.port();
            st.address = addr.ip().to_string();
        }
        st.listener = Some(listener);
    })
}

/// Sets a callback that fires when a new client connects (Go `WithOnClientConnect`).
/// The callback receives a snapshot with `remote_addr` and `connected_at`.
pub fn with_on_client_connect<F: Fn(ConnectedClient) + Send + Sync + 'static>(f: F) -> TcpServerOption {
    Box::new(move |s| wr(&s.shared.state).on_client_connect = Some(Arc::new(f)))
}

/// Sets a callback that fires when a client disconnects (Go `WithOnClientDisconnect`).
/// The callback receives a snapshot with the final rx/tx transaction counts.
pub fn with_on_client_disconnect<F: Fn(ConnectedClient) + Send + Sync + 'static>(f: F) -> TcpServerOption {
    Box::new(move |s| wr(&s.shared.state).on_client_disconnect = Some(Arc::new(f)))
}

impl TcpServer {
    /// Creates a new Modbus TCP server (Go `NewTCPServer`). Defaults: port 502,
    /// an empty [`MemoryStore`], the default stdout logger.
    pub fn new(address: &str, options: Vec<TcpServerOption>) -> TcpServer {
        let state = State {
            address: address.to_string(),
            // Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3 (default port 502)
            port: DEFAULT_TCP_PORT,
            listener: None,
            handlers: HashMap::new(),
            default_store: Arc::new(MemoryStore::new()),
            running: false,
            logger: logging::default_logger(),
            stop: Arc::new(AtomicBool::new(false)),
            accept_thread: None,
            on_client_connect: None,
            on_client_disconnect: None,
        };
        let mut server = TcpServer {
            shared: Arc::new(Shared {
                state: RwLock::new(state),
                clients: RwLock::new(HashMap::new()),
                client_threads: Mutex::new(Vec::new()),
                protocol: ServerProtocolHandler::new(),
            }),
        };

        // Apply options
        for option in options {
            option(&mut server);
        }

        // Setup default handlers based on data store
        server.setup_default_handlers();
        server
    }

    /// Sets the logger (inherent counterpart of [`Server::with_logger`]).
    pub fn set_logger(&self, logger: Arc<dyn LoggerInterface>) {
        wr(&self.shared.state).logger = logger;
    }

    /// Sets the data store and re-installs the default handlers (Go `WithDataStore`;
    /// like Go this discards custom handlers registered with `set_handler`).
    pub fn set_data_store(&self, data_store: Arc<dyn DataStore>) {
        wr(&self.shared.state).default_store = data_store;
        self.setup_default_handlers();
    }

    // Configures handlers for the standard Modbus functions.
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Codes)
    fn setup_default_handlers(&self) {
        let store = {
            let mut st = wr(&self.shared.state);
            // Clear existing handlers
            st.handlers.clear();
            st.default_store.clone()
        };

        // Registers one default handler: `$method` of the protocol handler against the store.
        macro_rules! default_handler {
            ($fc:expr, $method:ident) => {{
                let store = store.clone();
                let shared = self.shared.clone();
                self.set_handler(
                    $fc,
                    Arc::new(move |ctx: &Context, req: &dyn Request| shared.protocol.$method(ctx, req, store.as_ref())),
                );
            }};
        }

        // Read Coils (0x01) - Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1
        default_handler!(FUNC_READ_COILS, handle_read_coils);
        // Read Discrete Inputs (0x02) - Ref: Section 6.2
        default_handler!(FUNC_READ_DISCRETE_INPUTS, handle_read_discrete_inputs);
        // Read Holding Registers (0x03) - Ref: Section 6.3
        default_handler!(FUNC_READ_HOLDING_REGISTERS, handle_read_holding_registers);
        // Read Input Registers (0x04) - Ref: Section 6.4
        default_handler!(FUNC_READ_INPUT_REGISTERS, handle_read_input_registers);
        // Write Single Coil (0x05) - Ref: Section 6.5
        default_handler!(FUNC_WRITE_SINGLE_COIL, handle_write_single_coil);
        // Write Single Register (0x06) - Ref: Section 6.6
        default_handler!(FUNC_WRITE_SINGLE_REGISTER, handle_write_single_register);
        // Write Multiple Coils (0x0F) - Ref: Section 6.11
        default_handler!(FUNC_WRITE_MULTIPLE_COILS, handle_write_multiple_coils);
        // Write Multiple Registers (0x10) - Ref: Section 6.12
        default_handler!(FUNC_WRITE_MULTIPLE_REGISTERS, handle_write_multiple_registers);
        // Read/Write Multiple Registers (0x17) - Ref: Section 6.17
        default_handler!(FUNC_READ_WRITE_MULTIPLE_REGISTERS, handle_read_write_multiple_registers);
        // Read Device Identification (0x2B / MEI 0x0E) - Ref: Section 6.21
        default_handler!(FUNC_READ_DEVICE_IDENTIFICATION, handle_read_device_identification);
    }

    /// Returns the local address the server is bound to (once started or when a
    /// listener was supplied). Go tests read `srv.listener.Addr()`.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        rd(&self.shared.state).listener.as_ref().and_then(|l| l.local_addr().ok())
    }

    /// Returns a snapshot of all currently connected clients with their
    /// connection statistics (Go `ConnectedClients`).
    pub fn connected_clients(&self) -> Vec<ConnectedClient> {
        rd(&self.shared.clients).values().map(|c| c.snapshot()).collect()
    }

    // Test hook: tracks a synthetic client (Go tests write `srv.clients[...]` directly).
    #[cfg(test)]
    pub(crate) fn insert_client_for_test(&self, key: &str, client: Arc<ClientConn>) {
        wr(&self.shared.clients).insert(key.to_string(), client);
    }
}

impl Server for TcpServer {
    /// Starts the server: binds (unless a listener was supplied) and spawns the accept thread.
    fn start(&self, ctx: &Context) -> Result<()> {
        let (logger, address, port, stop, listener) = {
            let mut st = wr(&self.shared.state);
            if st.running {
                return Err(Error::msg("server already running"));
            }

            // If no listener was provided via with_server_listener, create one
            if st.listener.is_none() {
                let addr = format!("{}:{}", st.address, st.port);
                st.listener = Some(TcpListener::bind(addr)?);
            }

            let listener = st.listener.as_ref().expect("listener set above");
            // Update address/port from listener in case it was dynamic (port 0)
            if let Ok(addr) = listener.local_addr() {
                st.port = addr.port();
                st.address = addr.ip().to_string();
            }
            // The accept thread polls a non-blocking listener so it can notice stop requests.
            let listener = st.listener.as_ref().expect("listener set above");
            listener.set_nonblocking(true)?;
            let accept_listener = listener.try_clone()?;

            st.running = true;
            st.stop = Arc::new(AtomicBool::new(false));
            (st.logger.clone(), st.address.clone(), st.port, st.stop.clone(), accept_listener)
        };

        logger.info(ctx, format_args!("Modbus TCP server started on {}:{}", address, port));

        // Start accepting connections
        let shared = self.shared.clone();
        let ctx = ctx.clone();
        let handle = thread::spawn(move || accept_loop(shared, listener, stop, ctx));
        wr(&self.shared.state).accept_thread = Some(handle);
        Ok(())
    }

    /// Stops the server: signals the accept loop, closes the listener and every
    /// client connection, then joins all threads.
    fn stop(&self, ctx: &Context) -> Result<()> {
        let (listener, accept_thread, logger) = {
            let mut st = wr(&self.shared.state);
            if !st.running {
                return Ok(()); // Already stopped
            }
            // Signal accept loop to stop
            st.stop.store(true, Ordering::SeqCst);
            st.running = false;
            // Drop the listener so start() creates a fresh one
            (st.listener.take(), st.accept_thread.take(), st.logger.clone())
        };

        // Join the accept loop (it owns a cloned listener handle), then close ours.
        // Joining happens outside the state lock: worker threads take that lock.
        let me = thread::current().id();
        if let Some(h) = accept_thread.filter(|h| h.thread().id() != me) {
            let _ = h.join();
        }
        drop(listener);

        // Close all client connections
        let clients: Vec<Arc<ClientConn>> = {
            let mut map = wr(&self.shared.clients);
            let v = map.values().cloned().collect();
            map.clear();
            v
        };
        for client in clients {
            if let Some(conn) = &client.conn {
                let _ = conn.shutdown(std::net::Shutdown::Both);
            }
        }

        // Join connection threads (never the calling thread itself, e.g. when stop is
        // invoked from a handler or callback).
        let threads: Vec<JoinHandle<()>> = std::mem::take(&mut *self.shared.client_threads.lock().unwrap_or_else(|e| e.into_inner()));
        for h in threads {
            if h.thread().id() != me {
                let _ = h.join();
            }
        }

        logger.info(ctx, format_args!("Modbus TCP server stopped"));
        Ok(())
    }

    /// Returns true if the server is running.
    fn is_running(&self) -> bool {
        rd(&self.shared.state).running
    }

    /// Sets the handler for a specific Modbus function code.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Codes)
    fn set_handler(&self, function_code: FunctionCode, handler: HandlerFunc) {
        wr(&self.shared.state).handlers.insert(function_code, handler);
    }

    /// Sets the logger for the server (Go `WithLogger`); returns the same server.
    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Server> {
        self.set_logger(logger);
        self
    }

    /// Sets the data store for the server (Go `WithDataStore`); returns the same server.
    fn with_data_store(self: Arc<Self>, data_store: Arc<dyn DataStore>) -> Arc<dyn Server> {
        self.set_data_store(data_store);
        self
    }
}

impl Drop for TcpServer {
    // Implementation detail: do not leak threads/sockets if the owner forgets to stop.
    fn drop(&mut self) {
        let _ = Server::stop(self, &Context::background());
    }
}

// Accepts incoming connections (Go `acceptLoop`). Implementation detail, not spec.
fn accept_loop(shared: Arc<Shared>, listener: TcpListener, stop: Arc<AtomicBool>, ctx: Context) {
    loop {
        // Check if we should stop
        if stop.load(Ordering::SeqCst) {
            return;
        }

        let (stream, peer) = match listener.accept() {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Nothing pending, retry (Go: accept deadline timeout)
                thread::sleep(ACCEPT_POLL_INTERVAL);
                continue;
            }
            Err(e) => {
                // Check if we're shutting down
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                shared.logger().error(&ctx, format_args!("Error accepting connection: {}", e));
                thread::sleep(ACCEPT_POLL_INTERVAL);
                continue;
            }
        };

        let remote_addr = peer.to_string();
        shared.logger().info(&ctx, format_args!("New client connected: {}", remote_addr));

        // Accepted sockets must block (the listener is non-blocking)
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));

        // Add client to tracked connections
        let client = Arc::new(ClientConn::new(remote_addr.clone(), SystemTime::now(), Some(stream)));
        wr(&shared.clients).insert(remote_addr.clone(), client.clone());

        let on_connect = rd(&shared.state).on_client_connect.clone();
        if let Some(cb) = on_connect {
            cb(ConnectedClient {
                remote_addr,
                connected_at: client.connected_at,
                ..Default::default()
            });
        }

        // Handle the client connection (thread-per-connection)
        let shared2 = shared.clone();
        let stop2 = stop.clone();
        let handle = thread::spawn(move || handle_connection(shared2, client, stop2));
        let mut threads = shared.client_threads.lock().unwrap_or_else(|e| e.into_inner());
        threads.retain(|h| !h.is_finished()); // drop handles of finished connections
        threads.push(handle);
    }
}

// Outcome of a framed read.
enum ReadOutcome {
    Ok,
    // Clean EOF before any byte of this read arrived
    Eof,
    // Read timeout before any byte of this read arrived
    Timeout,
    Err(std::io::Error),
}

// Reads exactly `buf.len()` bytes (Go `io.ReadFull`). With `idle_ok`, a timeout before the
// first byte is reported as `Timeout` (caller retries) instead of an error; a timeout in
// the middle of a frame keeps waiting, so no partial bytes are lost.
fn read_full(stream: &TcpStream, buf: &mut [u8], stop: &AtomicBool, idle_ok: bool) -> ReadOutcome {
    let mut reader = stream;
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => {
                return if filled == 0 {
                    ReadOutcome::Eof
                } else {
                    ReadOutcome::Err(std::io::ErrorKind::UnexpectedEof.into())
                }
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                if stop.load(Ordering::SeqCst) {
                    return ReadOutcome::Err(e);
                }
                if filled == 0 && idle_ok {
                    return ReadOutcome::Timeout;
                }
                if !idle_ok {
                    return ReadOutcome::Err(e);
                }
            }
            Err(e) => return ReadOutcome::Err(e),
        }
    }
    ReadOutcome::Ok
}

// Handles a client connection (Go `handleConnection`). Implements Modbus TCP message
// handling: read MBAP header, read PDU, dispatch, write response.
// Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3 (Message Processing)
fn handle_connection(shared: Arc<Shared>, client: Arc<ClientConn>, stop: Arc<AtomicBool>) {
    let ctx = Context::background();
    let conn = client.conn.as_ref().expect("tracked server connection has a socket");
    let remote_addr = client.remote_addr.clone();

    // Run the request loop, then always clean up (Go `defer`).
    connection_loop(&shared, &client, conn, &remote_addr, &stop, &ctx);

    let on_disconnect = rd(&shared.state).on_client_disconnect.clone();
    if let Some(cb) = on_disconnect {
        cb(client.snapshot());
    }

    // Remove client from tracked connections (unless stop() already cleared it)
    {
        let mut clients = wr(&shared.clients);
        if clients.get(&remote_addr).is_some_and(|c| Arc::ptr_eq(c, &client)) {
            clients.remove(&remote_addr);
        }
    }

    // Close the connection
    let _ = conn.shutdown(std::net::Shutdown::Both);
    shared.logger().info(&ctx, format_args!("Client disconnected: {}", remote_addr));
}

fn connection_loop(shared: &Shared, client: &ClientConn, conn: &TcpStream, remote_addr: &str, stop: &AtomicBool, ctx: &Context) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }

        // Read the Modbus TCP header (7 bytes)
        // Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3.1 (MBAP Header)
        // The MBAP header contains:
        // - Transaction Identifier (2 bytes)
        // - Protocol Identifier (2 bytes = 0 for Modbus)
        // - Length (2 bytes, number of following bytes including unit ID)
        // - Unit Identifier (1 byte)
        let mut header = [0u8; TCP_HEADER_LENGTH];
        match read_full(conn, &mut header, stop, true) {
            ReadOutcome::Ok => {}
            // Normal client disconnect
            ReadOutcome::Eof => return,
            // Timeout, just continue
            ReadOutcome::Timeout => continue,
            ReadOutcome::Err(e) => {
                if stop.load(Ordering::SeqCst) {
                    return; // closed by stop()
                }
                shared.logger().error(ctx, format_args!("Error reading header from {}: {}", remote_addr, e));
                return;
            }
        }

        // Parse MBAP header, using big-endian as per Modbus specification
        // Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3.1 (MBAP Header)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.2 (Data Encoding)
        let transaction_id = TransactionId(u16::from_be_bytes([header[0], header[1]]));
        let protocol_id = ProtocolId(u16::from_be_bytes([header[2], header[3]]));
        let length = u16::from_be_bytes([header[4], header[5]]);
        let unit_id = UnitId(header[6]);

        // Validate protocol ID (0 = MODBUS protocol)
        // Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3.1.1 (Protocol Identifier)
        if protocol_id != TCP_PROTOCOL_IDENTIFIER {
            shared.logger().error(ctx, format_args!("Invalid protocol ID from {}: {}", remote_addr, protocol_id.0));
            continue;
        }

        // Read the PDU (length - 1 bytes; the unit ID is already read)
        // Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3.1.1 (Length field)
        let data_length = length as i64 - 1;
        if data_length <= 0 {
            shared.logger().error(ctx, format_args!("Invalid data length from {}: {}", remote_addr, length));
            continue;
        }

        let mut data = vec![0u8; data_length as usize];
        match read_full(conn, &mut data, stop, false) {
            ReadOutcome::Ok => {}
            ReadOutcome::Eof => {
                shared.logger().error(ctx, format_args!("Error reading data from {}: EOF", remote_addr));
                return;
            }
            ReadOutcome::Timeout => unreachable!("idle_ok is false"),
            ReadOutcome::Err(e) => {
                if !stop.load(Ordering::SeqCst) {
                    shared.logger().error(ctx, format_args!("Error reading data from {}: {}", remote_addr, e));
                }
                return;
            }
        }

        // Extract function code and PDU data
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (Protocol Data Unit)
        // The PDU consists of:
        // - Function Code (1 byte)
        // - Data (variable length, function-specific)
        let function_code = FunctionCode(data[0]);
        let pdu_data = data[1..].to_vec();

        // Create a request
        let mut request = ServerRequest::new(unit_id, function_code, pdu_data);
        request.set_transaction_id(transaction_id);

        // Count received transaction
        client.rx_count.fetch_add(1, Ordering::SeqCst);
        client.fc_count[function_code.0 as usize].fetch_add(1, Ordering::SeqCst);

        shared.logger().debug(
            ctx,
            format_args!(
                "Received request from {}: txID={}, unit={}, function={}",
                remote_addr, transaction_id.0, unit_id.0, function_code
            ),
        );

        // Handle the request
        match dispatch_request(shared, ctx, &request) {
            Ok(response) => {
                // Send the response
                send_response(shared, conn, response.as_ref());
                client.tx_count.fetch_add(1, Ordering::SeqCst);
            }
            // If it's a Modbus error, create an exception response
            // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (MODBUS Exception Responses)
            Err(err @ Error::Modbus(_)) => {
                let Error::Modbus(modbus_err) = &err else { unreachable!() };
                shared.logger().debug(ctx, format_args!("Modbus exception: {}", err));

                // Exception Response PDU: function code with the high bit (0x80) set,
                // followed by a one byte exception code.
                // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Response PDU)
                let exception_response = ServerResponse::new(
                    transaction_id,
                    unit_id,
                    FunctionCode(function_code.0 | EXCEPTION_BIT),
                    vec![modbus_err.exception_code.0],
                );
                send_response(shared, conn, &exception_response);
                client.tx_count.fetch_add(1, Ordering::SeqCst);
            }
            // For other errors, log and disconnect
            Err(err) => {
                shared.logger().error(ctx, format_args!("Error processing request from {}: {}", remote_addr, err));
                return;
            }
        }
    }
}

// Dispatches a request to the handler registered for its function code (Go `dispatchRequest`).
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Codes)
fn dispatch_request(shared: &Shared, ctx: &Context, request: &dyn Request) -> Result<Box<dyn Response>> {
    // Get the function code
    let function_code = request.get_pdu().function_code;

    // Find the handler
    let handler = rd(&shared.state).handlers.get(&function_code).cloned();

    match handler {
        // Function code not supported, return an exception
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Codes)
        // Exception code 0x01 = Illegal Function
        None => Err(Error::Modbus(ModbusError::new(function_code, EXCEPTION_FUNCTION_CODE_NOT_SUPPORTED))),
        // Call the handler
        Some(h) => h(ctx, request),
    }
}

// Sends a response back to the client (Go `sendResponse`): encodes the MBAP header + PDU
// and writes it to the socket.
// Ref: Modbus_Messaging_Implementation_Guide_V1_0b.pdf, Section 3 (Message Encoding)
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP header + PDU = ADU)
fn send_response(shared: &Shared, conn: &TcpStream, response: &dyn Response) {
    let ctx = Context::background();
    let data = match response.encode() {
        Ok(d) => d,
        Err(e) => {
            shared.logger().error(&ctx, format_args!("Error encoding response: {}", e));
            return;
        }
    };

    // Send the encoded response to the client
    let mut writer = conn;
    if let Err(e) = writer.write_all(&data) {
        shared.logger().error(&ctx, format_args!("Error sending response: {}", e));
        return;
    }

    shared.logger().debug(
        &ctx,
        format_args!(
            "Sent response: txID={}, function={}",
            response.get_transaction_id().0,
            response.get_pdu().function_code
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_support::MockDataStore;
    use std::io::{Read, Write};

    fn quiet() -> TcpServerOption {
        with_server_logger(logging::noop())
    }

    fn started(options: Vec<TcpServerOption>) -> (TcpServer, SocketAddr) {
        let srv = TcpServer::new("127.0.0.1", options);
        srv.start(&Context::background()).expect("start");
        let addr = srv.local_addr().expect("addr");
        (srv, addr)
    }

    // Sends one MBAP frame and reads back one full response ADU.
    fn roundtrip(conn: &mut TcpStream, tid: u16, unit: u8, fc: u8, data: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&tid.to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        frame.push(unit);
        frame.push(fc);
        frame.extend_from_slice(data);
        conn.write_all(&frame).unwrap();

        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut header = [0u8; 7];
        conn.read_exact(&mut header).unwrap();
        let len = u16::from_be_bytes([header[4], header[5]]) as usize;
        let mut body = vec![0u8; len - 1];
        conn.read_exact(&mut body).unwrap();
        let mut out = header.to_vec();
        out.extend(body);
        out
    }

    #[test]
    fn serves_read_and_write_requests() {
        let store = Arc::new(MemoryStore::new());
        store.set_holding_register(Address(10), 0x1234);
        store.set_holding_register(Address(11), 0xABCD);
        let (srv, addr) = started(vec![with_server_port(0), quiet(), with_server_data_store(store.clone())]);
        let mut conn = TcpStream::connect(addr).unwrap();

        // Read Holding Registers: spec example style response (Section 6.3)
        let resp = roundtrip(&mut conn, 0x0007, 1, 0x03, &[0, 10, 0, 2]);
        assert_eq!(resp, vec![0, 7, 0, 0, 0, 7, 1, 0x03, 4, 0x12, 0x34, 0xAB, 0xCD]);

        // Write Single Register echoes and updates the store (Section 6.6)
        let resp = roundtrip(&mut conn, 8, 1, 0x06, &[0, 20, 0x55, 0xAA]);
        assert_eq!(resp, vec![0, 8, 0, 0, 0, 6, 1, 0x06, 0, 20, 0x55, 0xAA]);
        assert_eq!(store.get_holding_register(Address(20)), Some(0x55AA));

        // Stats are tracked
        let clients = srv.connected_clients();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].rx_transactions, 2);
        assert_eq!(clients[0].tx_transactions, 2);
        assert_eq!(clients[0].function_code_stats[&FUNC_READ_HOLDING_REGISTERS], 1);
        assert_eq!(clients[0].function_code_stats[&FUNC_WRITE_SINGLE_REGISTER], 1);

        srv.stop(&Context::background()).unwrap();
    }

    #[test]
    fn unsupported_function_and_invalid_data_give_exceptions() {
        let (srv, addr) = started(vec![with_server_port(0), quiet()]);
        let mut conn = TcpStream::connect(addr).unwrap();

        // Unknown function code 0x41 -> exception 0x01 (Illegal Function), Section 7
        let resp = roundtrip(&mut conn, 1, 1, 0x41, &[]);
        assert_eq!(resp, vec![0, 1, 0, 0, 0, 3, 1, 0xC1, 0x01]);

        // Read holding registers with quantity 0 -> exception 0x03 (Illegal Data Value)
        let resp = roundtrip(&mut conn, 2, 1, 0x03, &[0, 0, 0, 0]);
        assert_eq!(resp, vec![0, 2, 0, 0, 0, 3, 1, 0x83, 0x03]);

        // Connection stays usable after exceptions
        let resp = roundtrip(&mut conn, 3, 1, 0x03, &[0, 0, 0, 1]);
        assert_eq!(resp, vec![0, 3, 0, 0, 0, 5, 1, 0x03, 2, 0, 0]);

        srv.stop(&Context::background()).unwrap();
    }

    #[test]
    fn custom_handler_and_non_modbus_error_disconnects() {
        let (srv, addr) = started(vec![with_server_port(0), quiet()]);
        srv.set_handler(
            FunctionCode(0x41),
            Arc::new(|_ctx, req| {
                Ok(Box::new(ServerResponse::new(
                    req.get_transaction_id(),
                    req.get_unit_id(),
                    req.get_pdu().function_code,
                    vec![0xEE],
                )))
            }),
        );
        srv.set_handler(FunctionCode(0x42), Arc::new(|_ctx, _req| Err(Error::msg("boom"))));

        let mut conn = TcpStream::connect(addr).unwrap();
        assert_eq!(roundtrip(&mut conn, 9, 2, 0x41, &[]), vec![0, 9, 0, 0, 0, 3, 2, 0x41, 0xEE]);

        // Non-Modbus handler error: server logs and closes the connection (no response)
        conn.write_all(&[0, 10, 0, 0, 0, 2, 2, 0x42]).unwrap();
        let mut buf = [0u8; 16];
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(conn.read(&mut buf).unwrap(), 0);

        srv.stop(&Context::background()).unwrap();
    }

    #[test]
    fn start_twice_fails_and_stop_is_idempotent() {
        let (srv, _addr) = started(vec![with_server_port(0), quiet()]);
        assert!(srv.is_running());
        let err = srv.start(&Context::background()).unwrap_err();
        assert_eq!(err.to_string(), "server already running");

        srv.stop(&Context::background()).unwrap();
        assert!(!srv.is_running());
        srv.stop(&Context::background()).unwrap();

        // Can be restarted with a fresh listener
        srv.start(&Context::background()).unwrap();
        assert!(srv.is_running());
        srv.stop(&Context::background()).unwrap();
    }

    #[test]
    fn stop_closes_clients_and_joins() {
        let (srv, addr) = started(vec![with_server_port(0), quiet()]);
        let mut conn = TcpStream::connect(addr).unwrap();
        // make sure it is registered
        roundtrip(&mut conn, 1, 1, 0x03, &[0, 0, 0, 1]);

        srv.stop(&Context::background()).unwrap();
        assert_eq!(srv.connected_clients().len(), 0);

        // Server side closed: reads see EOF (or reset)
        let mut buf = [0u8; 4];
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert!(matches!(conn.read(&mut buf), Ok(0) | Err(_)));
        // Listener is closed: connecting fails
        assert!(TcpStream::connect(addr).is_err());
    }

    #[test]
    fn with_server_listener_and_trait_object() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = Arc::new(TcpServer::new("127.0.0.1", vec![with_server_listener(listener), quiet()]));
        let dyn_srv: Arc<dyn Server> = srv.clone();

        let store = Arc::new(MockDataStore::new());
        store.set_coil(Address(0), true);
        let dyn_srv = dyn_srv.with_data_store(store);
        dyn_srv.start(&Context::background()).unwrap();

        let mut conn = TcpStream::connect(addr).unwrap();
        let resp = roundtrip(&mut conn, 1, 1, 0x01, &[0, 0, 0, 1]);
        assert_eq!(resp, vec![0, 1, 0, 0, 0, 4, 1, 0x01, 1, 1]);

        dyn_srv.stop(&Context::background()).unwrap();
    }

    #[test]
    fn short_length_frames_are_skipped() {
        let (srv, addr) = started(vec![with_server_port(0), quiet()]);
        let mut conn = TcpStream::connect(addr).unwrap();
        // length = 1 (no PDU): logged and skipped, like Go; next valid frame is served
        conn.write_all(&[0, 1, 0, 0, 0, 1, 1]).unwrap();
        let resp = roundtrip(&mut conn, 2, 1, 0x03, &[0, 0, 0, 1]);
        assert_eq!(resp[0..2], [0, 2]);
        srv.stop(&Context::background()).unwrap();
    }
}
