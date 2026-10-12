//! ReconnectingTransport: lazy connect, re-create after failure. Port of
//! `gomodbus/client/transport_reconnecting.go`.
//!
//! Connection management is implementation detail, not spec. Note that, as in the Go
//! implementation, there is no backoff: every `conn` call after a reset dials again
//! (bounded by the caller's context deadline / the TCP transport connect timeout).
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.5 (Define MODBUS Transaction).

use std::sync::{Arc, RwLock};

use super::transport::{Transport, TransportConfig, TransportOption, build_config, same_transport};
use crate::common::{self, Context, Error, LoggerInterface, Result};
use crate::logging;
use crate::transport::{TCPTransport, TCPTransportOption};

// Option closures are FnOnce, but a reconnecting transport dials repeatedly, so the TCP
// options are turned into a factory that is re-run for every new connection.
type TcpOptionFactory = Box<dyn Fn() -> Vec<TCPTransportOption> + Send + Sync>;

struct State {
    conn: Option<Arc<dyn common::Transport>>,
    closed: bool,
}

/// ReconnectingTransport creates connections lazily and re-creates them after failures. It
/// uses an RwLock double-check locking pattern for efficiency (Go `reconnectingTransport`).
pub struct ReconnectingTransport {
    host: String,
    tcp_opts: TcpOptionFactory,
    logger: Arc<dyn LoggerInterface>,
    cfg: TransportConfig,
    state: RwLock<State>,
}

impl ReconnectingTransport {
    /// Creates a transport that connects lazily and reconnects on failure. The constructor
    /// never fails and never connects (Go `NewReconnectingTransport`).
    ///
    /// Go reuses the same `[]TCPTransportOption` for every dial. Rust options are `FnOnce`,
    /// so `tcp_opts` is a factory returning a fresh option list per connection.
    /// `logger` of `None` selects the default logger.
    pub fn new<F>(
        host: &str,
        logger: Option<Arc<dyn LoggerInterface>>,
        transport_opts: Vec<TransportOption>,
        tcp_opts: F,
    ) -> ReconnectingTransport
    where
        F: Fn() -> Vec<TCPTransportOption> + Send + Sync + 'static,
    {
        ReconnectingTransport {
            host: host.to_string(),
            tcp_opts: Box::new(tcp_opts),
            logger: logger.unwrap_or_else(logging::default_logger),
            cfg: build_config(transport_opts),
            state: RwLock::new(State { conn: None, closed: false }),
        }
    }

    // Creates a new TCPTransport and connects it (Go `connect`).
    fn connect(&self, ctx: &Context) -> Result<Arc<dyn common::Transport>> {
        let t = Arc::new(TCPTransport::with_options(self.host.clone(), (self.tcp_opts)()));
        let t: Arc<dyn common::Transport> = common::Transport::with_logger(t, self.logger.clone());

        t.connect(ctx)?;

        Ok(t)
    }
}

impl Transport for ReconnectingTransport {
    /// Returns the current transport or creates a new one if needed.
    fn conn(&self, ctx: &Context) -> Result<Arc<dyn common::Transport>> {
        // Fast path: read lock
        {
            let st = self.state.read().unwrap_or_else(|p| p.into_inner());
            if st.closed {
                return Err(Error::msg("transport is closed"));
            }
            if let Some(conn) = &st.conn {
                return Ok(conn.clone());
            }
        }

        // Slow path: write lock, double-check
        let mut st = self.state.write().unwrap_or_else(|p| p.into_inner());

        if st.closed {
            return Err(Error::msg("transport is closed"));
        }
        if let Some(conn) = &st.conn {
            return Ok(conn.clone());
        }

        let conn = self.connect(ctx)?;

        st.conn = Some(conn.clone());

        if let Some(on_connect) = &self.cfg.on_connect {
            on_connect();
        }

        Ok(conn)
    }

    /// Invalidates the stale transport if it is still the current one. The next `conn` call
    /// will create a fresh connection. Always returns `Ok` (as in Go, the disconnect error is
    /// only reported to the on-disconnect callback).
    fn reset(&self, stale: &Arc<dyn common::Transport>) -> Result<()> {
        let mut st = self.state.write().unwrap_or_else(|p| p.into_inner());

        let current = match &st.conn {
            Some(c) if same_transport(c, stale) => c.clone(),
            _ => return Ok(()),
        };

        let err = current.disconnect(&Context::background());
        st.conn = None;

        if let Some(on_disconnect) = &self.cfg.on_disconnect {
            on_disconnect(err.err());
        }

        Ok(())
    }

    /// Permanently shuts down the transport.
    fn close(&self) -> Result<()> {
        let mut st = self.state.write().unwrap_or_else(|p| p.into_inner());

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
