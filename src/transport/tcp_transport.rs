//! Modbus TCP client transport. Port of `gomodbus/transport/tcp_transport.go`.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4 (MODBUS Data Model) and Section 4.1
//! (MBAP Header). The spec only defines the ADU framing (MBAP header + PDU) and the
//! request/response pairing by Transaction Identifier; everything about threads, queues,
//! deadlines and reconnect handling below is implementation detail, not spec.
//!
//! Threading model (Go goroutines -> `std::thread`): `connect` spawns one reader and one writer
//! thread per connection. `send` places a [`Transaction`] in the [`TransactionPool`] (which
//! assigns the Transaction Identifier), queues it for the writer thread and waits for the reader
//! thread to complete it with the matching response. All state belonging to a single connection
//! lives in a `ConnHandle` identified by a generation number, so threads of an old connection can
//! never disturb a newer one after a reconnect.
//!
//! Typical use (consumers import the [`Transport`] trait for `connect`/`send`):
//!
//! ```no_run
//! use rsmodbus::common::{Context, Transport};
//! use rsmodbus::transport::tcp_transport::{TCPTransport, with_port};
//!
//! let t = TCPTransport::with_options("192.168.0.10", vec![with_port(502)]);
//! t.connect(&Context::background()).unwrap();
//! ```

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::common::{
    Context, DEFAULT_TCP_PORT, Error, FunctionCode, LoggerInterface, Request, Response, Result,
    TCP_HEADER_LENGTH, TCP_PROTOCOL_IDENTIFIER, TransactionId, Transport, UnitId,
};
use crate::logging;

use super::request::eof_error;
use super::response::Response as TcpResponse;
use super::transaction::Transaction;
use super::transaction_pool::TransactionPool;

/// Read deadline used by the reader thread so it can notice shutdown promptly (Go: 100ms).
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// Capacity of the write queue (Go: `make(chan *Transaction, 100)`).
const WRITE_QUEUE_CAPACITY: usize = 100;
/// Poll slice for blocking waits (implementation detail, not spec).
const POLL_SLICE: Duration = Duration::from_millis(10);
/// Default connect timeout when the context has no deadline (Go: 30s).
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------------------------
// Connection abstraction
// ---------------------------------------------------------------------------------------------

/// Minimal byte-stream connection the transport runs over: a `TcpStream` in production
/// ([`TcpConnection`]), a mock in tests. All methods take `&self` because the reader thread,
/// the writer thread and `disconnect` use the connection concurrently (Go `net.Conn`).
/// Implementation detail, not spec.
pub trait Connection: Send + Sync {
    /// Reads some bytes. A read deadline expiring must be reported as `ErrorKind::TimedOut` or
    /// `ErrorKind::WouldBlock`; `Ok(0)` means end of stream.
    fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
    /// Writes all of `buf`.
    fn write_all(&self, buf: &[u8]) -> io::Result<()>;
    /// Sets the read deadline (Go `SetReadDeadline`); `None` blocks indefinitely.
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// Closes the connection, unblocking pending reads and writes.
    fn close(&self) -> io::Result<()>;
}

/// [`Connection`] over a `std::net::TcpStream`.
pub struct TcpConnection(pub TcpStream);

impl Connection for TcpConnection {
    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.0).read(buf)
    }
    fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        (&self.0).write_all(buf)
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(timeout)
    }
    fn close(&self) -> io::Result<()> {
        match self.0.shutdown(Shutdown::Both) {
            // Already closed by the peer: closing again is not an error worth reporting.
            Err(e) if e.kind() == io::ErrorKind::NotConnected => Ok(()),
            other => other,
        }
    }
}

fn is_timeout_kind(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
}

// ---------------------------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------------------------

/// Settings collected from [`TCPTransportOption`]s.
pub struct TCPTransportConfig {
    logger: Arc<dyn LoggerInterface>,
    port: u16,
    timeout: Duration,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
}

/// A function that configures a [`TCPTransport`] (Go `TCPTransportOption`).
pub type TCPTransportOption = Box<dyn FnOnce(&mut TCPTransportConfig)>;

/// Sets the TCP port (default 502, Ref: Section 4.1 / registered port 502).
pub fn with_port(port: u16) -> TCPTransportOption {
    Box::new(move |c| c.port = port)
}

/// Sets the connect timeout, used when the `connect` context has no deadline
/// (Go `WithTimeoutOption`).
pub fn with_timeout_option(timeout: Duration) -> TCPTransportOption {
    Box::new(move |c| c.timeout = timeout)
}

/// Sets a custom reader used instead of the connection for incoming bytes (Go `WithReader`).
/// Its read timeouts must be reported as `TimedOut`/`WouldBlock` like the connection's.
pub fn with_reader(reader: Box<dyn Read + Send>) -> TCPTransportOption {
    Box::new(move |c| c.reader = Some(reader))
}

/// Sets a custom writer used instead of the connection for outgoing bytes (Go `WithWriter`).
pub fn with_writer(writer: Box<dyn Write + Send>) -> TCPTransportOption {
    Box::new(move |c| c.writer = Some(writer))
}

/// Sets the logger for the transport (Go `WithTransportLogger`).
pub fn with_transport_logger(logger: Arc<dyn LoggerInterface>) -> TCPTransportOption {
    Box::new(move |c| c.logger = logger)
}

// ---------------------------------------------------------------------------------------------
// Write queue
// ---------------------------------------------------------------------------------------------

/// Bounded FIFO between `send` callers and the writer thread (Go `writeChan`).
struct WriteQueue {
    queue: Mutex<VecDeque<Arc<Transaction>>>,
    not_empty: Condvar,
    not_full: Condvar,
}

/// Why a [`WriteQueue::push`] gave up.
enum PushError {
    /// The caller's context ended first.
    Context(Error),
    /// The connection is shutting down.
    Closing,
}

impl WriteQueue {
    fn new() -> WriteQueue {
        WriteQueue { queue: Mutex::new(VecDeque::new()), not_empty: Condvar::new(), not_full: Condvar::new() }
    }

    /// Queues `tx`, blocking while the queue is full; gives up when `ctx` ends or `done` is set.
    fn push(&self, tx: Arc<Transaction>, ctx: &Context, done: &AtomicBool) -> std::result::Result<(), PushError> {
        let mut q = self.queue.lock().unwrap();
        loop {
            if done.load(Ordering::SeqCst) {
                return Err(PushError::Closing);
            }
            if let Some(e) = ctx.err() {
                return Err(PushError::Context(e));
            }
            if q.len() < WRITE_QUEUE_CAPACITY {
                q.push_back(tx);
                self.not_empty.notify_one();
                return Ok(());
            }
            q = self.not_full.wait_timeout(q, ctx.clamp(POLL_SLICE)).unwrap().0;
        }
    }

    /// Pops the next transaction, waiting at most `timeout`.
    fn pop(&self, timeout: Duration) -> Option<Arc<Transaction>> {
        let mut q = self.queue.lock().unwrap();
        if q.is_empty() {
            q = self.not_empty.wait_timeout(q, timeout).unwrap().0;
        }
        let tx = q.pop_front();
        if tx.is_some() {
            self.not_full.notify_one();
        }
        tx
    }

    fn wake_all(&self) {
        self.not_empty.notify_all();
        self.not_full.notify_all();
    }
}

// ---------------------------------------------------------------------------------------------
// Transport state
// ---------------------------------------------------------------------------------------------

/// Everything belonging to one established connection (Go: `conn`, `done`, `writeChan`).
struct ConnHandle {
    /// Generation number, distinguishes this connection from earlier/later ones.
    generation: u64,
    conn: Arc<dyn Connection>,
    /// Signals shutdown of this connection's threads (Go `done` channel).
    done: AtomicBool,
    queue: WriteQueue,
}

impl ConnHandle {
    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Signals the threads to exit.
    fn signal_done(&self) {
        self.done.store(true, Ordering::SeqCst);
        self.queue.wake_all();
    }
}

/// State protected by the transport mutex (Go `mutex`).
struct State {
    connected: bool,
    generation: u64,
    handle: Option<Arc<ConnHandle>>,
}

struct Inner {
    logger: RwLock<Arc<dyn LoggerInterface>>,
    host: String,
    port: u16,
    timeout: Duration,
    reader: Option<Arc<Mutex<Box<dyn Read + Send>>>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    state: Mutex<State>,
    pool: TransactionPool,
}

/// Result of trying to fill a buffer from the connection.
enum Fill {
    Done,
    /// The read deadline expired; progress is kept in `filled` so no bytes are lost
    /// (Go's `io.ReadFull` loses partial data here, see module docs of the deviations).
    Timeout,
    Failed(Error),
}

impl Inner {
    fn logger(&self) -> Arc<dyn LoggerInterface> {
        self.logger.read().unwrap().clone()
    }

    /// True while `h` is the live connection (Go `IsConnected` check inside the loops).
    fn is_alive(&self, h: &ConnHandle) -> bool {
        let st = self.state.lock().unwrap();
        st.connected && st.generation == h.generation
    }

    /// Installs `conn` as the active connection and starts the reader and writer threads.
    /// The caller holds the state lock (Go: the tail of `Connect`).
    fn install(self: &Arc<Self>, st: &mut State, conn: Arc<dyn Connection>) {
        st.generation += 1;
        let handle = Arc::new(ConnHandle {
            generation: st.generation,
            conn,
            done: AtomicBool::new(false),
            queue: WriteQueue::new(),
        });
        st.handle = Some(handle.clone());
        st.connected = true;

        // Start the read and write threads (implementation detail, not spec).
        let (a, h) = (self.clone(), handle.clone());
        std::thread::spawn(move || a.read_loop(&h));
        let (a, h) = (self.clone(), handle);
        std::thread::spawn(move || a.write_loop(&h));
    }

    /// Marks the transport as disconnected (Go `setDisconnected`). Only acts if `h` is still the
    /// live connection. Unlike Go it also stops the sibling thread and closes the socket, so no
    /// thread or file descriptor outlives a failed connection.
    fn set_disconnected(&self, h: &ConnHandle, err: &Error) {
        let ctx = Context::background();
        let was_connected = {
            let mut st = self.state.lock().unwrap();
            let was = st.connected && st.generation == h.generation;
            if was {
                st.connected = false;
            }
            was
        };

        if was_connected {
            self.logger().error(&ctx, format_args!("Transport disconnected: {}", err));
            h.signal_done();
            let _ = h.conn.close();
            // Reset the transaction pool to clean state for next reconnection
            // (this cancels all pending transactions).
            self.pool.reset();
        }
    }

    /// Reads from the custom reader if one was configured, otherwise from the connection.
    fn io_read(&self, h: &ConnHandle, buf: &mut [u8]) -> io::Result<usize> {
        match &self.reader {
            Some(r) => r.lock().unwrap().read(buf),
            None => h.conn.read(buf),
        }
    }

    /// Writes to the custom writer if one was configured, otherwise to the connection.
    fn io_write(&self, h: &ConnHandle, data: &[u8]) -> io::Result<()> {
        match &self.writer {
            Some(w) => {
                let mut w = w.lock().unwrap();
                w.write_all(data)?;
                w.flush()
            }
            None => h.conn.write_all(data),
        }
    }

    /// Keeps reading until `buf` is full (Go `io.ReadFull`), tolerating read-deadline expiry.
    fn fill(&self, h: &ConnHandle, buf: &mut [u8], filled: &mut usize) -> Fill {
        while *filled < buf.len() {
            match self.io_read(h, &mut buf[*filled..]) {
                Ok(0) => return Fill::Failed(eof_error(*filled > 0)),
                Ok(n) => *filled += n,
                Err(e) if is_timeout_kind(e.kind()) => return Fill::Timeout,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Fill::Failed(Error::from(e)),
            }
        }
        Fill::Done
    }

    /// Handles an error for a specific transaction (Go `processError`).
    fn process_error(&self, tx_id: TransactionId, err: Error) {
        let ctx = Context::background();
        let logger = self.logger();
        // Try to find the transaction and complete it with error
        if let Some(tx) = self.pool.release(tx_id) {
            logger.debug(&ctx, format_args!("Processing error for transaction {}: {}", tx_id.0, err));
            tx.complete(Err(err));
        } else {
            logger.warn(&ctx, format_args!("Error for unknown transaction {}: {}", tx_id.0, err));
        }
    }

    /// Reader thread entry (Go `readLoop` with its deferred `setDisconnected`).
    fn read_loop(self: &Arc<Self>, h: &Arc<ConnHandle>) {
        let ctx = Context::background();
        let logger = self.logger();
        logger.debug(&ctx, format_args!("Starting read loop"));
        self.read_loop_body(h, &ctx);
        logger.debug(&ctx, format_args!("Exiting read loop"));
        self.set_disconnected(h, &Error::msg("read loop exited"));
    }

    /// Continuously reads from the connection and completes transactions with the responses.
    /// This implements the client side of the Modbus TCP protocol.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header) and Section 4 (data model)
    fn read_loop_body(&self, h: &Arc<ConnHandle>, ctx: &Context) {
        let logger = self.logger();
        // MBAP Header is 7 bytes: Transaction ID (2), Protocol ID (2), Length (2), Unit ID (1)
        let mut header = [0u8; TCP_HEADER_LENGTH];
        let mut header_filled = 0usize;

        loop {
            if h.is_done() {
                return;
            }
            // Check if we're still connected
            if !self.is_alive(h) {
                return;
            }

            // Set a deadline for this read operation so we notice shutdown quickly
            let _ = h.conn.set_read_timeout(Some(READ_TIMEOUT));

            // Read the response header (7 bytes)
            // Ref: Section 4.1 (MBAP Header)
            match self.fill(h, &mut header, &mut header_filled) {
                Fill::Done => {}
                // Timeouts are expected during idle periods and shutdown: re-check and retry.
                Fill::Timeout => continue,
                Fill::Failed(err) => {
                    // If we're already disconnected or shutting down, just exit
                    if h.is_done() {
                        return;
                    }
                    logger.error(ctx, format_args!("Error reading header: {}", err));
                    self.set_disconnected(h, &Error::wrap("read error", err));
                    return;
                }
            }
            header_filled = 0;

            // Hexdump of the header (no-op unless the logger implements it)
            logger.hexdump(ctx, &header);

            // Parse the MBAP header
            // Ref: Section 4.1, MBAP Header fields (all big-endian, Section 4.2)
            // Field 1: Transaction Identifier (2 bytes)
            let transaction_id = TransactionId(u16::from_be_bytes([header[0], header[1]]));
            // Field 2: Protocol Identifier (2 bytes) - 0 for MODBUS protocol
            let protocol_id = crate::common::ProtocolId(u16::from_be_bytes([header[2], header[3]]));
            // Field 3: Length (2 bytes) - number of following bytes (Unit Identifier + PDU)
            let length = u16::from_be_bytes([header[4], header[5]]);
            // Field 4: Unit Identifier (1 byte)
            let unit_id = UnitId(header[6]);

            logger.debug(ctx, format_args!("Received response: txID={}, length={}", transaction_id.0, length));

            // Check Protocol Identifier - must be 0 for MODBUS
            // Ref: Section 4.1 (Protocol Identifier = 0 for MODBUS protocol)
            if protocol_id != TCP_PROTOCOL_IDENTIFIER {
                logger.error(ctx, format_args!("Invalid protocol ID: {}", protocol_id.0));
                self.process_error(transaction_id, Error::InvalidProtocolHeader);
                continue;
            }

            // Length counts the Unit Identifier (already read) plus the PDU, so the PDU is length-1.
            // Ref: Section 4.1 (Length field)
            let body_length = length as i32 - 1;
            if body_length <= 0 {
                logger.error(ctx, format_args!("Invalid response length: {}", length));
                self.process_error(transaction_id, Error::InvalidResponseLength);
                continue;
            }

            // Read the function code and data (PDU)
            // Ref: Section 6 (MODBUS Function Codes)
            let mut body = vec![0u8; body_length as usize];
            let mut body_filled = 0usize;
            loop {
                match self.fill(h, &mut body, &mut body_filled) {
                    Fill::Done => break,
                    // Keep the bytes already read and carry on (Go would discard them).
                    Fill::Timeout => {
                        if h.is_done() {
                            return;
                        }
                    }
                    Fill::Failed(err) => {
                        // If we're shutting down, just exit
                        if h.is_done() {
                            return;
                        }
                        logger.error(ctx, format_args!("Error reading body: {}", err));
                        self.process_error(transaction_id, Error::wrap("read body error", err.clone()));
                        self.set_disconnected(h, &err);
                        return;
                    }
                }
            }

            // Hexdump of the body (no-op unless the logger implements it)
            logger.hexdump(ctx, &body);

            // Create a response
            // Ref: Section 4.1 / 6: the first PDU byte is the function code, the rest is the
            // function specific response data.
            let function_code = FunctionCode(body[0]);
            let response_data = body[1..].to_vec();
            let response = TcpResponse::new(transaction_id, unit_id, function_code, response_data);

            // Find and complete the transaction (matched by Transaction Identifier)
            // Ref: Section 4.1 (Transaction Identifier: "for transaction pairing")
            let Some(tx) = self.pool.release(transaction_id) else {
                logger.warn(ctx, format_args!("Received response for unknown transaction ID: {}", transaction_id.0));
                continue;
            };

            logger.debug(ctx, format_args!("Completing transaction {}", transaction_id.0));
            // Complete the transaction with the response
            tx.complete(Ok(Box::new(response)));
        }
    }

    /// Writer thread entry (Go `writeLoop` with its deferred `setDisconnected`).
    fn write_loop(self: &Arc<Self>, h: &Arc<ConnHandle>) {
        let ctx = Context::background();
        let logger = self.logger();
        logger.debug(&ctx, format_args!("Starting write loop"));
        self.write_loop_body(h, &ctx);
        logger.debug(&ctx, format_args!("Exiting write loop"));
        self.set_disconnected(h, &Error::msg("write loop exited"));
    }

    /// Continuously takes queued transactions and writes their requests.
    /// This implements the client side of sending Modbus TCP requests.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP Header) and Section 4 (data model)
    fn write_loop_body(&self, h: &Arc<ConnHandle>, ctx: &Context) {
        let logger = self.logger();
        loop {
            // First check if we're still connected
            if h.is_done() || !self.is_alive(h) {
                return;
            }

            let Some(tx) = h.queue.pop(POLL_SLICE * 2) else { continue };
            let tx_id = tx.request.get_transaction_id();

            // Check if we're still connected
            if !self.is_alive(h) {
                tx.complete(Err(Error::NotConnected));
                return;
            }

            // Check if the transaction is still valid
            if tx.context().is_done() {
                logger.debug(ctx, format_args!("Transaction {} was cancelled before writing", tx_id.0));
                continue;
            }
            if h.is_done() {
                // Transport is shutting down
                tx.complete(Err(Error::TransportClosing));
                return;
            }

            logger.debug(ctx, format_args!("Writing request for transaction {}", tx_id.0));

            // Encode the request: MBAP header + PDU
            // Ref: Section 4.1 (MBAP Header)
            let data = match tx.request.encode() {
                Ok(d) => d,
                Err(err) => {
                    logger.error(ctx, format_args!("Error encoding request: {}", err));
                    tx.complete(Err(err));
                    continue;
                }
            };

            // Hexdump of the encoded request (no-op unless the logger implements it)
            logger.hexdump(ctx, &data);

            // Check again if we should exit before writing
            if h.is_done() {
                tx.complete(Err(Error::TransportClosing));
                return;
            }

            // Write the request
            if let Err(e) = self.io_write(h, &data) {
                // If we're shutting down, don't report the error
                if h.is_done() {
                    tx.complete(Err(Error::TransportClosing));
                    return;
                }
                let err = Error::from(e);
                logger.error(ctx, format_args!("Error writing request: {}", err));
                tx.complete(Err(err.clone()));
                self.set_disconnected(h, &Error::wrap("write error", err));
                return;
            }

            logger.debug(ctx, format_args!("Wrote request for transaction {}", tx_id.0));
        }
    }

    /// Dials the server within `ctx`'s deadline (or the configured timeout).
    fn dial(&self, ctx: &Context) -> Result<TcpStream> {
        let logger = self.logger();
        let addr = format!("{}:{}", self.host, self.port);

        // Get deadline from context or use default timeout
        let deadline = ctx.deadline().unwrap_or_else(|| Instant::now() + self.timeout);

        let result: Result<TcpStream> = (|| {
            if let Some(e) = ctx.err() {
                return Err(e);
            }
            let addrs = addr.to_socket_addrs().map_err(Error::from)?;
            let mut last = Error::msg(format!("no addresses found for {}", addr));
            for a in addrs {
                if let Some(e) = ctx.err() {
                    return Err(e);
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(Error::DeadlineExceeded);
                }
                match TcpStream::connect_timeout(&a, remaining) {
                    Ok(s) => return Ok(s),
                    Err(e) => last = Error::from(e),
                }
            }
            Err(last)
        })();

        if let Err(e) = &result {
            logger.error(ctx, format_args!("Failed to connect to {}: {}", addr, e));
        }
        result
    }
}

// ---------------------------------------------------------------------------------------------
// TCPTransport
// ---------------------------------------------------------------------------------------------

/// TCPTransport implements the [`Transport`] trait for Modbus TCP.
///
/// Wrap it in an `Arc` (or call [`TCPTransport::into_arc`]) to share it with a client; all
/// methods take `&self` and are thread-safe. After a connection failure `is_connected` turns
/// false, pending transactions fail with `Error::TransportClosing`, and `connect` can be called
/// again (this is what the reconnecting client builds on).
///
/// Error kinds returned by `send`: `NotConnected` (not connected), `Context`/`DeadlineExceeded`
/// (caller's context ended), `TransactionTimeout` (no response within the pool timeout, 5s),
/// `TransportClosing` (disconnect/reset while waiting), `InvalidProtocolHeader` /
/// `InvalidResponseLength` (malformed MBAP header of the reply), or an `Io` error.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4 (MODBUS Data Model), Section 4.1 (MBAP Header)
pub struct TCPTransport {
    inner: Arc<Inner>,
}

impl TCPTransport {
    /// Creates a new TCPTransport for `host` with default settings (port 502, 30s connect
    /// timeout, default logger).
    pub fn new(host: impl Into<String>) -> TCPTransport {
        TCPTransport::with_options(host, Vec::new())
    }

    /// Creates a new TCPTransport and applies `options` in order (Go `NewTCPTransport`).
    pub fn with_options(host: impl Into<String>, options: Vec<TCPTransportOption>) -> TCPTransport {
        let mut config = TCPTransportConfig {
            logger: logging::default_logger(),
            port: DEFAULT_TCP_PORT,
            timeout: DEFAULT_CONNECT_TIMEOUT,
            reader: None,
            writer: None,
        };
        for option in options {
            option(&mut config);
        }
        // The pool shares the transport's logger (Go creates it with the default logger).
        let pool = TransactionPool::with_options(vec![super::transaction_pool::with_logger(config.logger.clone())]);
        TCPTransport {
            inner: Arc::new(Inner {
                logger: RwLock::new(config.logger),
                host: host.into(),
                port: config.port,
                timeout: config.timeout,
                reader: config.reader.map(|r| Arc::new(Mutex::new(r))),
                writer: config.writer.map(|w| Arc::new(Mutex::new(w))),
                state: Mutex::new(State { connected: false, generation: 0, handle: None }),
                pool,
            }),
        }
    }

    /// Convenience: wraps the transport in an `Arc<dyn Transport>`.
    pub fn into_arc(self) -> Arc<dyn Transport> {
        Arc::new(self)
    }

    /// Resets the transaction pool without disconnecting. Useful to recover from error states
    /// where the connection is still valid but transaction state may be corrupted: all pending
    /// transactions are cancelled with `Error::TransportClosing` and all IDs become free.
    pub fn reset_transactions(&self, ctx: &Context) {
        let logger = self.inner.logger();
        logger.info(ctx, format_args!("Resetting transaction pool"));
        self.inner.pool.reset();
        logger.info(ctx, format_args!("Transaction pool has been reset"));
    }

    /// Number of in-flight transactions (diagnostics).
    pub fn pending_transactions(&self) -> usize {
        self.inner.pool.get_count()
    }

    /// Runs the transport over an already established `conn` instead of dialling, as if
    /// `connect` had succeeded (useful for tests and custom byte streams).
    pub fn connect_with(&self, ctx: &Context, conn: Arc<dyn Connection>) -> Result<()> {
        let mut st = self.inner.state.lock().unwrap();
        if st.connected {
            return Err(Error::AlreadyConnected);
        }
        self.inner.logger().info(ctx, format_args!("Using provided connection for {}:{}", self.inner.host, self.inner.port));
        self.inner.pool.reset();
        self.inner.install(&mut st, conn);
        Ok(())
    }

    /// Shared body of `disconnect`; `quiet` suppresses the info logs (used on drop).
    fn disconnect_impl(&self, ctx: &Context, quiet: bool) -> Result<()> {
        let inner = &self.inner;
        let mut st = inner.state.lock().unwrap();

        if !st.connected {
            return Ok(());
        }

        let logger = inner.logger();
        if !quiet {
            logger.info(ctx, format_args!("Disconnecting from Modbus TCP server"));
        }

        // Mark as disconnected first to prevent new operations
        st.connected = false;
        let handle = st.handle.take();

        let mut result = Ok(());
        if let Some(h) = handle {
            // Signal the threads to exit
            h.signal_done();

            // Give the reader and writer threads a moment to notice shutdown. This helps
            // prevent "use of closed network connection" errors.
            std::thread::sleep(POLL_SLICE);

            // Reset the transaction pool instead of closing it: this cancels all pending
            // transactions and leaves the pool usable for the next connection.
            inner.pool.reset();

            // Close the connection
            result = h.conn.close().map_err(Error::from);
        }

        if !quiet {
            logger.info(ctx, format_args!("Disconnected from Modbus TCP server"));
        }
        result
    }
}

impl Drop for TCPTransport {
    /// Stops the connection threads when the last handle goes away (Go relies on the GC and
    /// leaks the goroutines; implementation detail, not spec).
    fn drop(&mut self) {
        let _ = self.disconnect_impl(&Context::background(), true);
    }
}

impl Transport for TCPTransport {
    /// Establishes a connection to the Modbus TCP server. Fails with `AlreadyConnected` if
    /// already connected. Uses the context's deadline, or the transport timeout (default 30s).
    fn connect(&self, ctx: &Context) -> Result<()> {
        let inner = &self.inner;
        let mut st = inner.state.lock().unwrap();

        if st.connected {
            return Err(Error::AlreadyConnected);
        }

        let logger = inner.logger();
        logger.info(ctx, format_args!("Connecting to Modbus TCP server at {}:{}", inner.host, inner.port));

        // Reset the transaction pool to ensure clean state during reconnection
        inner.pool.reset();

        // Connect with timeout
        let stream = inner.dial(ctx)?;
        // Modbus requests are small and latency sensitive (Go's net package also defaults to this)
        let _ = stream.set_nodelay(true);

        inner.install(&mut st, Arc::new(TcpConnection(stream)));

        logger.info(ctx, format_args!("Connected to Modbus TCP server at {}:{}", inner.host, inner.port));
        Ok(())
    }

    /// Closes the connection. Calling it when not connected (or repeatedly) is a no-op.
    fn disconnect(&self, ctx: &Context) -> Result<()> {
        self.disconnect_impl(ctx, false)
    }

    /// Returns true if connected to the server.
    fn is_connected(&self) -> bool {
        self.inner.state.lock().unwrap().connected
    }

    /// Sends a request and returns the response. This implements the client side of the
    /// request/response pattern: the pool assigns the Transaction Identifier (also written to
    /// `request`), the writer thread sends the ADU, the reader thread completes the transaction.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4 (MODBUS Data Model), Section 4.1 (MBAP Header)
    fn send(&self, ctx: &Context, request: &mut dyn Request) -> Result<Box<dyn Response>> {
        let inner = &self.inner;
        let handle = {
            let st = inner.state.lock().unwrap();
            match (&st.handle, st.connected) {
                (Some(h), true) => h.clone(),
                _ => return Err(Error::NotConnected),
            }
        };
        let logger = inner.logger();

        // Log the function code being sent
        // Ref: Section 6 (MODBUS Function Codes)
        logger.debug(ctx, format_args!("Sending request: function={}", request.get_pdu().function_code.0));

        // Create a transaction and add it to the pool
        // Ref: Section 4.1 (MBAP Header): the pool assigns the Transaction Identifier used to
        // match the response.
        let tx = match inner.pool.place(ctx, request) {
            Ok(tx) => tx,
            Err(e) => {
                logger.error(ctx, format_args!("Failed to create transaction: {}", e));
                return Err(Error::wrap("failed to create transaction", e));
            }
        };
        let tx_id = request.get_transaction_id();

        logger.debug(ctx, format_args!("Created transaction {}", tx_id.0));

        // Send the transaction to the write loop
        match handle.queue.push(tx.clone(), ctx, &handle.done) {
            Ok(()) => logger.debug(ctx, format_args!("Queued transaction {} for writing", tx_id.0)),
            Err(PushError::Context(e)) => {
                // Context cancelled before we could queue
                logger.debug(ctx, format_args!("Context cancelled before queueing transaction {}", tx_id.0));
                inner.pool.release_tx(tx_id, &tx);
                return Err(e);
            }
            Err(PushError::Closing) => {
                // Transport is shutting down
                logger.debug(ctx, format_args!("Transport shutting down, cancelling transaction {}", tx_id.0));
                inner.pool.release_tx(tx_id, &tx);
                return Err(Error::TransportClosing);
            }
        }

        // Wait for the response
        match tx.wait(ctx) {
            Ok(response) => {
                logger.debug(ctx, format_args!("Received response for transaction {}", tx_id.0));
                Ok(response)
            }
            Err(e) => {
                // On a caller context error the transaction is left to the timeout monitor,
                // exactly like Go.
                logger.debug(ctx, format_args!("Received error for transaction {}: {}", tx_id.0, e));
                Err(e)
            }
        }
    }

    /// Sets the logger for the transport and returns the same transport (Go mutates in place).
    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Transport> {
        *self.inner.logger.write().unwrap() = logger.clone();
        self.inner.pool.set_logger(logger);
        self
    }
}

// ---------------------------------------------------------------------------------------------
// Tests (port of tcp_transport_test.go, plus loopback tests of the real socket path)
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_support::MockRequest;
    use crate::transport::request::Request as TcpRequest;

    /// mockConn implements [`Connection`] for testing (Go `mockConn`).
    struct MockConn {
        state: Mutex<MockConnState>,
    }

    struct MockConnState {
        read_data: Vec<u8>,
        read_index: usize,
        written_data: Vec<u8>,
        closed: bool,
    }

    impl MockConn {
        fn new() -> Arc<MockConn> {
            Arc::new(MockConn {
                state: Mutex::new(MockConnState {
                    read_data: Vec::new(),
                    read_index: 0,
                    written_data: Vec::new(),
                    closed: false,
                }),
            })
        }
        fn closed(&self) -> bool {
            self.state.lock().unwrap().closed
        }
        fn written(&self) -> Vec<u8> {
            self.state.lock().unwrap().written_data.clone()
        }
        fn feed(&self, data: &[u8]) {
            self.state.lock().unwrap().read_data.extend_from_slice(data);
        }
    }

    impl Connection for MockConn {
        fn read(&self, b: &mut [u8]) -> io::Result<usize> {
            let mut m = self.state.lock().unwrap();
            if m.closed {
                return Err(io::Error::other("use of closed network connection"));
            }
            if m.read_index >= m.read_data.len() {
                // No data: behave like a read deadline expiring after a short wait
                drop(m);
                std::thread::sleep(Duration::from_millis(10));
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
            }
            let n = b.len().min(m.read_data.len() - m.read_index);
            let start = m.read_index;
            b[..n].copy_from_slice(&m.read_data[start..start + n]);
            m.read_index += n;
            Ok(n)
        }
        fn write_all(&self, b: &[u8]) -> io::Result<()> {
            let mut m = self.state.lock().unwrap();
            if m.closed {
                return Err(io::Error::other("use of closed network connection"));
            }
            m.written_data.extend_from_slice(b);
            Ok(())
        }
        fn set_read_timeout(&self, _t: Option<Duration>) -> io::Result<()> {
            Ok(())
        }
        fn close(&self) -> io::Result<()> {
            self.state.lock().unwrap().closed = true;
            Ok(())
        }
    }

    fn quiet_transport() -> TCPTransport {
        TCPTransport::with_options("localhost", vec![with_transport_logger(logging::noop())])
    }

    fn sleep(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// createTestRequest creates a test request for tests
    fn create_test_request(unit_id: UnitId, function_code: FunctionCode, data: Vec<u8>) -> TcpRequest {
        TcpRequest::new(unit_id, function_code, data)
    }

    /// TestDisconnectClosedConnection tests that the Disconnect method handles
    /// closed connections gracefully.
    #[test]
    fn test_disconnect_closed_connection() {
        // Create a TCPTransport with the mock connection, marked connected with loops running
        let conn = MockConn::new();
        let transport = quiet_transport();
        let ctx = Context::background();
        transport.connect_with(&ctx, conn.clone()).unwrap();
        assert!(transport.is_connected());

        // Wait a moment for threads to start
        sleep(100);

        // Disconnect should close the threads cleanly
        transport.disconnect(&ctx).expect("Disconnect returned an error");

        // Wait a moment for threads to exit
        sleep(100);

        // Check that the connection was closed
        assert!(conn.closed(), "Connection was not closed");

        // Make sure we can reconnect after disconnect: a brand new transport
        let transport = quiet_transport();
        let conn = MockConn::new();
        transport.connect_with(&ctx, conn.clone()).unwrap();
        sleep(100);

        // Disconnect again
        transport.disconnect(&ctx).expect("Second disconnect returned an error");
        sleep(100);

        // The connection should be closed
        assert!(conn.closed(), "Connection was not closed on second disconnect");
    }

    /// TestMultipleDisconnects tests that calling Disconnect multiple times is safe.
    #[test]
    fn test_multiple_disconnects() {
        let conn = MockConn::new();
        let transport = quiet_transport();
        let ctx = Context::background();
        transport.connect_with(&ctx, conn).unwrap();
        sleep(100);

        transport.disconnect(&ctx).expect("First disconnect returned an error");
        // Disconnect again, should be a no-op
        transport.disconnect(&ctx).expect("Second disconnect returned an error");
        // Disconnect a third time, should still be a no-op
        transport.disconnect(&ctx).expect("Third disconnect returned an error");
    }

    /// TestRaceConditionDisconnect tests that there's no race condition when
    /// disconnecting while read/write operations are in progress.
    #[test]
    fn test_race_condition_disconnect() {
        let conn = MockConn::new();
        let transport = Arc::new(quiet_transport());
        let ctx = Context::background();
        transport.connect_with(&ctx, conn.clone()).unwrap();
        sleep(100);

        // Start a thread that will disconnect after a short delay
        let t2 = transport.clone();
        std::thread::spawn(move || {
            sleep(50);
            let _ = t2.disconnect(&Context::background());
        });

        // Wait for disconnection to complete
        sleep(200);

        // Check that we're disconnected
        assert!(!transport.is_connected(), "Transport should be disconnected");
        // The connection should be closed
        assert!(conn.closed(), "Connection was not closed");
    }

    /// TestResetTransactions tests the ResetTransactions method
    #[test]
    fn test_reset_transactions() {
        let transport = quiet_transport();
        let ctx = Context::background();

        // Get initial pool size
        assert_eq!(transport.inner.pool.get_count(), 0, "Expected initial transaction count to be 0");

        // Create a dummy request to add to the pool
        let mut request = create_test_request(UnitId(1), FunctionCode(0x03), vec![0x00, 0x01, 0x00, 0x02]);

        // Add it to the pool
        let tx = transport.inner.pool.place(&ctx, &mut request).expect("Failed to place transaction");

        // Verify pool now has one transaction
        assert_eq!(transport.inner.pool.get_count(), 1);

        // Detect when the transaction is cancelled
        let (tx_done, rx_done) = std::sync::mpsc::channel();
        let tx2 = tx.clone();
        std::thread::spawn(move || {
            match tx2.wait_timeout(Duration::from_secs(1)) {
                Some(Err(_)) => tx_done.send(true).unwrap(),
                _ => tx_done.send(false).unwrap(),
            }
        });

        // Reset transactions
        transport.reset_transactions(&ctx);

        // Wait for transaction to be cancelled
        assert!(rx_done.recv().unwrap(), "Timeout waiting for transaction to be cancelled");

        // Verify pool is empty
        assert_eq!(transport.inner.pool.get_count(), 0, "Expected transaction count to be 0 after reset");

        // Verify we can add transactions after reset
        transport.inner.pool.place(&ctx, &mut request).expect("Failed to place transaction after reset");

        // Verify pool now has one transaction
        assert_eq!(transport.inner.pool.get_count(), 1);
    }

    // ---- additional tests (not in Go) -------------------------------------------------------

    /// A response frame (MBAP + PDU) for transaction `tid`.
    fn frame(tid: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&tid.to_be_bytes());
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
        v.push(unit);
        v.extend_from_slice(pdu);
        v
    }

    #[test]
    fn send_when_not_connected() {
        let t = quiet_transport();
        let mut r = MockRequest::new(TransactionId(0), UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
        assert_eq!(t.send(&Context::background(), &mut r).err(), Some(Error::NotConnected));
        assert!(!t.is_connected());
    }

    #[test]
    fn connect_with_twice_is_already_connected() {
        let t = quiet_transport();
        let ctx = Context::background();
        t.connect_with(&ctx, MockConn::new()).unwrap();
        assert_eq!(t.connect_with(&ctx, MockConn::new()), Err(Error::AlreadyConnected));
        assert_eq!(t.connect(&ctx), Err(Error::AlreadyConnected));
        t.disconnect(&ctx).unwrap();
    }

    #[test]
    fn send_writes_frame_and_matches_response_over_mock() {
        let conn = MockConn::new();
        let t = Arc::new(quiet_transport());
        let ctx = Context::background();
        t.connect_with(&ctx, conn.clone()).unwrap();

        // Response for the first transaction (ID 0), delivered a little later, in two chunks
        // to exercise partial reads.
        let c2 = conn.clone();
        let feeder = std::thread::spawn(move || {
            sleep(100);
            let f = frame(0, 1, &[0x03, 0x02, 0x00, 0x2A]);
            c2.feed(&f[..4]);
            sleep(150);
            c2.feed(&f[4..]);
        });

        let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0x6B, 0, 1]);
        let resp = t.send(&ctx.with_timeout(Duration::from_secs(3)), &mut req).unwrap();
        feeder.join().unwrap();

        assert_eq!(crate::common::Request::get_transaction_id(&req), TransactionId(0));
        assert_eq!(resp.get_transaction_id(), TransactionId(0));
        assert_eq!(resp.get_unit_id(), UnitId(1));
        assert_eq!(resp.get_pdu().function_code, FunctionCode(3));
        assert_eq!(resp.get_pdu().data, vec![0x02, 0x00, 0x2A]);
        assert_eq!(conn.written(), vec![0, 0, 0, 0, 0, 6, 1, 3, 0, 0x6B, 0, 1]);
        assert_eq!(t.pending_transactions(), 0);
        t.disconnect(&ctx).unwrap();
    }

    #[test]
    fn send_times_out_with_context_and_leaves_transaction_pending() {
        let conn = MockConn::new();
        let t = quiet_transport();
        let ctx = Context::background();
        t.connect_with(&ctx, conn).unwrap();
        let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
        let err = t.send(&ctx.with_timeout(Duration::from_millis(150)), &mut req).err().unwrap();
        assert_eq!(err, Error::DeadlineExceeded);
        assert!(err.is_timeout());
        // Left for the timeout monitor, as in Go
        assert_eq!(t.pending_transactions(), 1);
        t.disconnect(&ctx).unwrap();
        assert_eq!(t.pending_transactions(), 0);
    }

    #[test]
    fn send_with_cancelled_context() {
        let t = quiet_transport();
        t.connect_with(&Context::background(), MockConn::new()).unwrap();
        let ctx = Context::background().with_cancel();
        ctx.cancel();
        let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
        assert_eq!(t.send(&ctx, &mut req).err(), Some(Error::ContextCanceled));
    }

    #[test]
    fn disconnect_cancels_pending_send() {
        let conn = MockConn::new();
        let t = Arc::new(quiet_transport());
        let ctx = Context::background();
        t.connect_with(&ctx, conn).unwrap();
        let t2 = t.clone();
        let sender = std::thread::spawn(move || {
            let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
            t2.send(&Context::background().with_timeout(Duration::from_secs(5)), &mut req).err()
        });
        sleep(150);
        t.disconnect(&ctx).unwrap();
        assert_eq!(sender.join().unwrap(), Some(Error::TransportClosing));
    }

    #[test]
    fn invalid_protocol_id_and_length_fail_the_transaction() {
        // Protocol ID != 0 for transaction 0
        let conn = MockConn::new();
        let t = quiet_transport();
        let ctx = Context::background();
        t.connect_with(&ctx, conn.clone()).unwrap();
        let mut bad = frame(0, 1, &[3, 0]);
        bad[3] = 1;
        conn.feed(&bad);
        let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
        let err = t.send(&ctx.with_timeout(Duration::from_secs(2)), &mut req).err().unwrap();
        assert_eq!(err, Error::InvalidProtocolHeader);
        t.disconnect(&ctx).unwrap();

        // Length 1 (no PDU) for transaction 0
        let conn = MockConn::new();
        let t = quiet_transport();
        t.connect_with(&ctx, conn.clone()).unwrap();
        conn.feed(&[0, 0, 0, 0, 0, 1, 1]);
        let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
        let err = t.send(&ctx.with_timeout(Duration::from_secs(2)), &mut req).err().unwrap();
        assert_eq!(err, Error::InvalidResponseLength);
        t.disconnect(&ctx).unwrap();
    }

    #[test]
    fn response_for_unknown_transaction_is_ignored() {
        let conn = MockConn::new();
        let t = quiet_transport();
        let ctx = Context::background();
        t.connect_with(&ctx, conn.clone()).unwrap();
        conn.feed(&frame(999, 1, &[3, 2, 0, 1]));
        conn.feed(&frame(0, 1, &[3, 2, 0, 7]));
        let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
        let resp = t.send(&ctx.with_timeout(Duration::from_secs(2)), &mut req).unwrap();
        assert_eq!(resp.get_pdu().data, vec![2, 0, 7]);
        assert!(t.is_connected());
        t.disconnect(&ctx).unwrap();
    }

    #[test]
    fn with_logger_returns_same_transport() {
        let t = Arc::new(quiet_transport());
        let t2: Arc<dyn Transport> = t.clone().with_logger(logging::noop());
        assert!(!t2.is_connected());
    }

    // --- loopback tests over a real socket ---

    use std::net::TcpListener;

    /// Spawns a one-connection-at-a-time server answering each request with `respond(frame)`;
    /// returns the port. `respond` returning None closes the connection.
    fn spawn_server(respond: impl Fn(&[u8]) -> Option<Vec<u8>> + Send + 'static) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                loop {
                    let mut hdr = [0u8; 7];
                    if s.read_exact(&mut hdr).is_err() {
                        break;
                    }
                    let len = u16::from_be_bytes([hdr[4], hdr[5]]) as usize;
                    let mut body = vec![0u8; len - 1];
                    if s.read_exact(&mut body).is_err() {
                        break;
                    }
                    let mut req = hdr.to_vec();
                    req.extend_from_slice(&body);
                    match respond(&req) {
                        Some(r) => {
                            if s.write_all(&r).is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        });
        port
    }

    fn tcp_transport(port: u16) -> TCPTransport {
        TCPTransport::with_options(
            "127.0.0.1",
            vec![with_port(port), with_transport_logger(logging::noop()), with_timeout_option(Duration::from_secs(2))],
        )
    }

    #[test]
    fn loopback_request_response_and_reconnect() {
        // Echo server: replies to FC03 with one register holding the low byte of the transaction ID
        let port = spawn_server(|req| {
            let tid = u16::from_be_bytes([req[0], req[1]]);
            Some(frame(tid, req[6], &[req[7], 2, 0, tid as u8]))
        });
        let t = tcp_transport(port);
        let ctx = Context::background().with_timeout(Duration::from_secs(5));
        t.connect(&ctx).unwrap();
        assert!(t.is_connected());
        for i in 0..5u8 {
            let mut req = TcpRequest::new(UnitId(3), FunctionCode(3), vec![0, 0, 0, 1]);
            let resp = t.send(&ctx, &mut req).unwrap();
            assert_eq!(resp.get_transaction_id(), TransactionId(i as u16));
            assert_eq!(resp.get_unit_id(), UnitId(3));
            assert_eq!(resp.get_pdu().data, vec![2, 0, i]);
        }
        t.disconnect(&ctx).unwrap();
        assert!(!t.is_connected());
        let mut req = TcpRequest::new(UnitId(3), FunctionCode(3), vec![0, 0, 0, 1]);
        assert_eq!(t.send(&ctx, &mut req).err(), Some(Error::NotConnected));

        // Reconnect on the same transport; IDs start again at 0 after the pool reset
        t.connect(&ctx).unwrap();
        let mut req = TcpRequest::new(UnitId(3), FunctionCode(3), vec![0, 0, 0, 1]);
        let resp = t.send(&ctx, &mut req).unwrap();
        assert_eq!(resp.get_transaction_id(), TransactionId(0));
        t.disconnect(&ctx).unwrap();
    }

    #[test]
    fn loopback_concurrent_sends_are_matched_by_transaction_id() {
        let port = spawn_server(|req| {
            let tid = u16::from_be_bytes([req[0], req[1]]);
            // echo the requested address back as the response data
            Some(frame(tid, req[6], &[0x03, 2, req[8], req[9]]))
        });
        let t = Arc::new(tcp_transport(port));
        let ctx = Context::background().with_timeout(Duration::from_secs(10));
        t.connect(&ctx).unwrap();
        let mut handles = Vec::new();
        for n in 0..8u8 {
            let t = t.clone();
            let ctx = ctx.clone();
            handles.push(std::thread::spawn(move || {
                for k in 0..10u8 {
                    let addr = n * 10 + k;
                    let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, addr, 0, 1]);
                    let resp = t.send(&ctx, &mut req).unwrap();
                    assert_eq!(resp.get_pdu().data, vec![2, 0, addr]);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(t.pending_transactions(), 0);
        t.disconnect(&ctx).unwrap();
    }

    #[test]
    fn loopback_exception_response_is_returned_as_response() {
        let port = spawn_server(|req| {
            let tid = u16::from_be_bytes([req[0], req[1]]);
            Some(frame(tid, req[6], &[req[7] | 0x80, 0x02]))
        });
        let t = tcp_transport(port);
        let ctx = Context::background().with_timeout(Duration::from_secs(5));
        t.connect(&ctx).unwrap();
        let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
        let resp = t.send(&ctx, &mut req).unwrap();
        assert!(resp.is_exception());
        assert!(resp.to_error().is_some());
        t.disconnect(&ctx).unwrap();
    }

    #[test]
    fn loopback_peer_close_disconnects_and_fails_pending() {
        let port = spawn_server(|_| None);
        let t = tcp_transport(port);
        let ctx = Context::background().with_timeout(Duration::from_secs(5));
        t.connect(&ctx).unwrap();
        let mut req = TcpRequest::new(UnitId(1), FunctionCode(3), vec![0, 0, 0, 1]);
        let err = t.send(&ctx, &mut req).err().unwrap();
        // The pending transaction is failed by the reset on disconnect
        assert!(matches!(err, Error::TransportClosing | Error::Io { .. }), "unexpected: {err:?}");
        // is_connected turns false
        let end = Instant::now() + Duration::from_secs(2);
        while t.is_connected() && Instant::now() < end {
            sleep(10);
        }
        assert!(!t.is_connected());
    }

    #[test]
    fn connect_failure_returns_io_error() {
        let port = crate::common::find_free_port_tcp().unwrap();
        let t = tcp_transport(port);
        let err = t.connect(&Context::background()).err().unwrap();
        assert!(matches!(err, Error::Io { .. }), "unexpected: {err:?}");
        assert!(!t.is_connected());
    }

    #[test]
    fn connect_with_expired_context() {
        let t = tcp_transport(1);
        let ctx = Context::background().with_timeout(Duration::ZERO);
        assert_eq!(t.connect(&ctx).err(), Some(Error::DeadlineExceeded));
    }

    #[test]
    fn custom_writer_receives_frames() {
        struct Shared(Arc<Mutex<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let sink = Arc::new(Mutex::new(Vec::new()));
        let t = TCPTransport::with_options(
            "localhost",
            vec![with_transport_logger(logging::noop()), with_writer(Box::new(Shared(sink.clone())))],
        );
        let ctx = Context::background();
        t.connect_with(&ctx, MockConn::new()).unwrap();
        let mut req = TcpRequest::new(UnitId(1), FunctionCode(5), vec![0, 1, 0xFF, 0]);
        let _ = t.send(&ctx.with_timeout(Duration::from_millis(150)), &mut req);
        assert_eq!(*sink.lock().unwrap(), vec![0, 0, 0, 0, 0, 6, 1, 5, 0, 1, 0xFF, 0]);
        t.disconnect(&ctx).unwrap();
    }
}
