//! Client transport abstraction (connection lifecycle). Port of `gomodbus/client/transport.go`.
//!
//! Everything in this file is implementation plumbing around obtaining a usable connection;
//! the Modbus specification does not define connection management beyond requiring a TCP
//! connection to port 502.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction) for the
//! request/response exchange the bridge forwards; MODBUS Messaging on TCP/IP Implementation
//! Guide (connection management) for the connection part, which is not in the file in docs/.

use std::sync::{Arc, Mutex};

use crate::common::{self, Context, Error, LoggerInterface, Request, Response, Result};
use crate::logging;

/// Transport abstracts how a client obtains a usable connection (Go `client.Transport`).
/// Two built-in implementations are provided: `DirectTransport` (connect once) and
/// `ReconnectingTransport` (lazy connect, auto-reconnect).
///
/// Implementation detail, not spec.
pub trait Transport: Send + Sync {
    /// Returns an active transport, creating one if necessary.
    fn conn(&self, ctx: &Context) -> Result<Arc<dyn common::Transport>>;

    /// Invalidates a stale transport. The `stale` parameter prevents thundering-herd resets:
    /// only the caller holding the failed transport triggers the invalidation.
    fn reset(&self, stale: &Arc<dyn common::Transport>) -> Result<()>;

    /// Permanently shuts down the transport.
    fn close(&self) -> Result<()>;
}

/// Callback fired after a connection is established.
pub type OnConnect = Arc<dyn Fn() + Send + Sync>;
/// Callback fired when a connection is lost (`None` when the disconnect succeeded cleanly).
pub type OnDisconnect = Arc<dyn Fn(Option<Error>) + Send + Sync>;

/// Settings collected from [`TransportOption`]s (Go `transportConfig`).
#[derive(Default, Clone)]
pub struct TransportConfig {
    pub(crate) on_connect: Option<OnConnect>,
    pub(crate) on_disconnect: Option<OnDisconnect>,
}

/// Configures transport lifecycle hooks (Go `TransportOption`).
pub type TransportOption = Box<dyn FnOnce(&mut TransportConfig)>;

/// Applies options in order to a fresh config.
pub(crate) fn build_config(options: Vec<TransportOption>) -> TransportConfig {
    let mut cfg = TransportConfig::default();
    for opt in options {
        opt(&mut cfg);
    }
    cfg
}

/// Registers a callback that fires after a connection is established (Go `WithOnConnect`).
pub fn with_on_connect<F: Fn() + Send + Sync + 'static>(f: F) -> TransportOption {
    Box::new(move |cfg| cfg.on_connect = Some(Arc::new(f)))
}

/// Registers a callback that fires when a connection is lost (Go `WithOnDisconnect`).
pub fn with_on_disconnect<F: Fn(Option<Error>) + Send + Sync + 'static>(f: F) -> TransportOption {
    Box::new(move |cfg| cfg.on_disconnect = Some(Arc::new(f)))
}

/// True when both handles point at the same transport object (Go interface `==`).
pub(crate) fn same_transport(a: &Arc<dyn common::Transport>, b: &Arc<dyn common::Transport>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

/// TransportBridge adapts a client [`Transport`] into a `common::Transport` so it can be
/// passed to `BaseClient::new` without modifying `BaseClient` (Go `transportBridge`).
pub struct TransportBridge {
    ct: Arc<dyn Transport>,
    logger: Arc<dyn LoggerInterface>,
    mu: Mutex<()>,
}

impl TransportBridge {
    /// Creates a bridge wrapping the given Transport (Go `newTransportBridge`).
    pub fn new(ct: Arc<dyn Transport>) -> TransportBridge {
        TransportBridge { ct, logger: logging::default_logger(), mu: Mutex::new(()) }
    }
}

impl common::Transport for TransportBridge {
    /// Establishes a connection by calling `conn` on the underlying Transport.
    fn connect(&self, ctx: &Context) -> Result<()> {
        self.ct.conn(ctx).map(|_| ())
    }

    /// Permanently closes the underlying Transport.
    fn disconnect(&self, _ctx: &Context) -> Result<()> {
        self.ct.close()
    }

    /// Returns true. The Transport abstraction manages connection state internally; returning
    /// true ensures `BaseClient::send` proceeds to the bridge's `send`, where `conn` handles
    /// lazy connection and reconnection.
    fn is_connected(&self) -> bool {
        true
    }

    /// Obtains the current transport via `conn`, sends the request through it, and resets on
    /// transport-level errors (non-Modbus errors). No retry is performed - that is the
    /// caller's concern.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7: an exception response is a
    /// valid reply and must not invalidate the connection.
    fn send(&self, ctx: &Context, request: &mut dyn Request) -> Result<Box<dyn Response>> {
        let conn = self.ct.conn(ctx)?;

        let resp = conn.send(ctx, request);
        if let Err(err) = &resp
            && !common::is_modbus_error(err)
        {
            let reset_err = {
                let _guard = self.mu.lock().unwrap_or_else(|p| p.into_inner());
                self.ct.reset(&conn)
            };
            if let Err(reset_err) = reset_err {
                self.logger.error(ctx, format_args!("Failed to reset transport: {}", reset_err));
            }
        }
        resp
    }

    /// Returns a new TransportBridge with the given logger.
    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn common::Transport> {
        Arc::new(TransportBridge { ct: self.ct.clone(), logger, mu: Mutex::new(()) })
    }
}
