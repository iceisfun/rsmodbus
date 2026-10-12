//! DirectTransport: connect once, never reconnect. Port of `gomodbus/client/transport_direct.go`.
//!
//! Connection management is implementation detail, not spec.
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction) -
//! the transport only carries the request/response exchange.

use std::sync::{Arc, Mutex, MutexGuard};

use super::transport::{Transport, TransportConfig, TransportOption, build_config, same_transport};
use crate::common::{self, Context, Error, LoggerInterface, Result};
use crate::logging;
use crate::transport::{TCPTransport, TCPTransportOption};

// State guarded by the mutex (Go `mu` protects conn/closed).
struct State {
    conn: Option<Arc<dyn common::Transport>>,
    closed: bool,
}

/// DirectTransport connects once and does not reconnect on failure (Go `directTransport`).
pub struct DirectTransport {
    state: Mutex<State>,
    cfg: TransportConfig,
    #[allow(dead_code)]
    logger: Arc<dyn LoggerInterface>,
}

impl DirectTransport {
    /// Creates a transport that connects immediately and returns an error if the connection
    /// fails. The on-connect callback fires on success (Go `NewDirectTransport`).
    /// `logger` of `None` selects the default logger.
    pub fn new(
        ctx: &Context,
        host: &str,
        logger: Option<Arc<dyn LoggerInterface>>,
        transport_opts: Vec<TransportOption>,
        tcp_opts: Vec<TCPTransportOption>,
    ) -> Result<DirectTransport> {
        let logger = logger.unwrap_or_else(logging::default_logger);
        let cfg = build_config(transport_opts);

        let tcp_transport = Arc::new(TCPTransport::with_options(host, tcp_opts));
        // `with_logger` configures and returns the same transport.
        let conn: Arc<dyn common::Transport> = common::Transport::with_logger(tcp_transport, logger.clone());

        conn.connect(ctx)?;

        let dt = DirectTransport {
            state: Mutex::new(State { conn: Some(conn), closed: false }),
            cfg,
            logger,
        };

        if let Some(on_connect) = &dt.cfg.on_connect {
            on_connect();
        }

        Ok(dt)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Transport for DirectTransport {
    /// Returns the pre-created transport or an error if the transport is closed or has been
    /// reset.
    fn conn(&self, _ctx: &Context) -> Result<Arc<dyn common::Transport>> {
        let st = self.lock();

        if st.closed {
            return Err(Error::msg("transport is closed"));
        }
        match &st.conn {
            None => Err(Error::msg("connection is not available")),
            Some(c) => Ok(c.clone()),
        }
    }

    /// Closes the stale transport and sets it to nothing. A direct transport cannot create a
    /// new connection after reset - subsequent `conn` calls return an error.
    fn reset(&self, stale: &Arc<dyn common::Transport>) -> Result<()> {
        let mut st = self.lock();

        let current = match &st.conn {
            Some(c) if same_transport(c, stale) => c.clone(),
            _ => return Ok(()),
        };

        let result = current.disconnect(&Context::background());
        st.conn = None;

        if let Some(on_disconnect) = &self.cfg.on_disconnect {
            on_disconnect(result.clone().err());
        }

        result
    }

    /// Permanently shuts down the transport.
    fn close(&self) -> Result<()> {
        let mut st = self.lock();

        if st.closed {
            return Ok(());
        }
        st.closed = true;

        let conn = match st.conn.take() {
            None => return Ok(()),
            Some(c) => c,
        };

        let result = conn.disconnect(&Context::background());

        if let Some(on_disconnect) = &self.cfg.on_disconnect {
            on_disconnect(result.clone().err());
        }

        result
    }
}
