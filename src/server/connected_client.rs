//! Connected-client tracking. Port of `gomodbus/server/connected_client.go`.
//!
//! Everything in this file is server implementation detail (connection
//! bookkeeping and statistics), not part of the Modbus specification; the only
//! spec-related piece is the function-code statistics, keyed by the function
//! code field of the request PDU.
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (function code field of the PDU)

use std::collections::BTreeMap;
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::common::FunctionCode;

/// ClientConn is the internal per-connection tracking state (Go `clientConn`).
/// It contains atomics and the socket, so it is shared via `Arc` and never copied.
///
/// Implementation detail, not spec.
pub struct ClientConn {
    /// Remote address of the peer ("ip:port").
    pub remote_addr: String,
    /// Time the client connected.
    pub connected_at: SystemTime,
    /// The client socket (`None` for synthetic entries, e.g. in tests).
    pub conn: Option<TcpStream>,
    /// Number of requests received from this client.
    pub rx_count: AtomicU64,
    /// Number of responses sent to this client.
    pub tx_count: AtomicU64,
    /// Per-function-code count of received requests, indexed by the function
    /// code byte. Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (Function code, 1 byte)
    pub fc_count: [AtomicU64; 256],
}

impl ClientConn {
    /// Creates tracking state for a new connection with all counters at zero.
    pub fn new(remote_addr: impl Into<String>, connected_at: SystemTime, conn: Option<TcpStream>) -> ClientConn {
        ClientConn {
            remote_addr: remote_addr.into(),
            connected_at,
            conn,
            rx_count: AtomicU64::new(0),
            tx_count: AtomicU64::new(0),
            fc_count: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Creates a [`ConnectedClient`] snapshot of the current counters.
    pub fn snapshot(&self) -> ConnectedClient {
        ConnectedClient {
            remote_addr: self.remote_addr.clone(),
            connected_at: self.connected_at,
            rx_transactions: self.rx_count.load(Ordering::SeqCst),
            tx_transactions: self.tx_count.load(Ordering::SeqCst),
            function_code_stats: fc_snapshot(self),
        }
    }
}

/// ConnectedClient is a snapshot of a connected client's state.
/// Returned by `TcpServer::connected_clients()`. Safe to clone and store.
///
/// Implementation detail, not spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedClient {
    /// Remote address of the connected client.
    pub remote_addr: String,

    /// Time the client connected (`UNIX_EPOCH` when unset, like Go's zero time).
    pub connected_at: SystemTime,

    /// Number of requests received from this client.
    pub rx_transactions: u64,

    /// Number of responses sent to this client.
    pub tx_transactions: u64,

    /// Per-function-code count of received requests. Only non-zero entries are
    /// included. Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (Function code)
    pub function_code_stats: BTreeMap<FunctionCode, u64>,
}

impl Default for ConnectedClient {
    fn default() -> ConnectedClient {
        ConnectedClient {
            remote_addr: String::new(),
            connected_at: SystemTime::UNIX_EPOCH,
            rx_transactions: 0,
            tx_transactions: 0,
            function_code_stats: BTreeMap::new(),
        }
    }
}

// Formats a whole-seconds duration the way Go's `time.Duration.String` does
// ("0s", "45s", "1m30s", "2h0m0s").
fn format_go_duration(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{}h{}m{}s", h, m, s)
    } else if m > 0 {
        format!("{}m{}s", m, s)
    } else {
        format!("{}s", s)
    }
}

impl std::fmt::Display for ConnectedClient {
    /// Returns a human-readable summary of the connected client (Go `String`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let elapsed = SystemTime::now().duration_since(self.connected_at).unwrap_or_default();
        write!(
            f,
            "{} | connected {} | rx: {} tx: {}",
            self.remote_addr,
            format_go_duration(elapsed),
            self.rx_transactions,
            self.tx_transactions
        )?;
        if !self.function_code_stats.is_empty() {
            // BTreeMap iterates in function code order => deterministic output
            let parts: Vec<String> = self
                .function_code_stats
                .iter()
                .map(|(fc, n)| format!("{}={}", fc, n))
                .collect();
            write!(f, " | fc: {}", parts.join(" "))?;
        }
        Ok(())
    }
}

/// Creates a function-code statistics map from a [`ClientConn`]'s atomic counters
/// (Go `fcSnapshot`). Only non-zero entries are included.
pub fn fc_snapshot(c: &ClientConn) -> BTreeMap<FunctionCode, u64> {
    let mut stats = BTreeMap::new();
    for (i, counter) in c.fc_count.iter().enumerate() {
        let v = counter.load(Ordering::SeqCst);
        if v > 0 {
            stats.insert(FunctionCode(i as u8), v);
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::*;
    use crate::server::tcp_server::{with_on_client_connect, with_on_client_disconnect, with_server_port, TcpServer};
    use std::net::TcpStream;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::sleep;

    fn hours_ago(h: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(h * 3600)
    }

    #[test]
    fn connected_client_string() {
        let client = ConnectedClient {
            remote_addr: "192.168.1.10:54321".into(),
            connected_at: hours_ago(2),
            rx_transactions: 1523,
            tx_transactions: 1520,
            ..Default::default()
        };
        let s = client.to_string();
        assert!(s.contains("192.168.1.10:54321"), "missing remote address: {}", s);
        assert!(s.contains("connected"), "missing 'connected' label: {}", s);
        assert!(s.contains("rx: 1523"), "missing rx count: {}", s);
        assert!(s.contains("tx: 1520"), "missing tx count: {}", s);
        assert!(s.contains("2h0m0s"), "expected Go-style duration: {}", s);
    }

    #[test]
    fn connected_client_string_zero_counts() {
        let client = ConnectedClient {
            remote_addr: "10.0.0.1:12345".into(),
            connected_at: SystemTime::now(),
            ..Default::default()
        };
        let s = client.to_string();
        assert!(s.contains("rx: 0"), "got: {}", s);
        assert!(s.contains("tx: 0"), "got: {}", s);
    }

    #[test]
    fn client_conn_atomic_counters() {
        let client = ClientConn::new("127.0.0.1:9999", SystemTime::now(), None);
        for _ in 0..3 {
            client.rx_count.fetch_add(1, Ordering::SeqCst);
        }
        for _ in 0..2 {
            client.tx_count.fetch_add(1, Ordering::SeqCst);
        }
        assert_eq!(client.rx_count.load(Ordering::SeqCst), 3);
        assert_eq!(client.tx_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn tcp_server_connected_clients_empty() {
        let srv = TcpServer::new("127.0.0.1", vec![with_server_port(0)]);
        assert_eq!(srv.connected_clients().len(), 0);
    }

    #[test]
    fn tcp_server_connected_clients_snapshot() {
        let srv = TcpServer::new("127.0.0.1", vec![with_server_port(0)]);

        // Inject a tracked client to test snapshot logic without a real TCP connection
        let now = SystemTime::now();
        let client = Arc::new(ClientConn::new("10.0.0.5:40000", now, None));
        client.rx_count.store(100, Ordering::SeqCst);
        client.tx_count.store(99, Ordering::SeqCst);
        srv.insert_client_for_test("10.0.0.5:40000", client);

        let snapshots = srv.connected_clients();
        assert_eq!(snapshots.len(), 1);
        let snap = &snapshots[0];
        assert_eq!(snap.remote_addr, "10.0.0.5:40000");
        assert_eq!(snap.connected_at, now);
        assert_eq!(snap.rx_transactions, 100);
        assert_eq!(snap.tx_transactions, 99);
    }

    #[test]
    fn with_on_client_connect_callback() {
        let got: Arc<Mutex<Option<ConnectedClient>>> = Arc::new(Mutex::new(None));
        let got2 = got.clone();
        let srv = TcpServer::new(
            "127.0.0.1",
            vec![
                with_server_port(0),
                with_on_client_connect(move |c| {
                    *got2.lock().unwrap() = Some(c);
                }),
            ],
        );

        let ctx = Context::background();
        srv.start(&ctx).expect("Failed to start server");

        let addr = srv.local_addr().unwrap();
        let conn = TcpStream::connect(addr).expect("Failed to connect");

        // Give the accept loop time to process
        sleep(Duration::from_millis(100));

        let got = got.lock().unwrap();
        let got = got.as_ref().expect("OnClientConnect callback was not called");
        assert!(!got.remote_addr.is_empty(), "empty RemoteAddr");
        assert_ne!(got.connected_at, SystemTime::UNIX_EPOCH, "zero ConnectedAt");
        assert_eq!((got.rx_transactions, got.tx_transactions), (0, 0));

        drop(conn);
        srv.stop(&ctx).unwrap();
    }

    #[test]
    fn with_on_client_disconnect_callback() {
        let (tx, rx) = mpsc::channel::<ConnectedClient>();
        let tx = Mutex::new(tx);
        let srv = TcpServer::new(
            "127.0.0.1",
            vec![
                with_server_port(0),
                with_on_client_disconnect(move |c| {
                    let _ = tx.lock().unwrap().send(c);
                }),
            ],
        );

        let ctx = Context::background();
        srv.start(&ctx).expect("Failed to start server");

        // Connect and immediately close
        let conn = TcpStream::connect(srv.local_addr().unwrap()).expect("Failed to connect");
        // Let the server register the connection
        sleep(Duration::from_millis(50));
        drop(conn);

        let got = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("OnClientDisconnect callback was not called within timeout");
        assert!(!got.remote_addr.is_empty(), "empty RemoteAddr");
        assert_ne!(got.connected_at, SystemTime::UNIX_EPOCH, "zero ConnectedAt");

        srv.stop(&ctx).unwrap();
    }

    #[test]
    fn nil_callbacks_do_not_panic() {
        // Server with no callbacks set should not panic
        let srv = TcpServer::new("127.0.0.1", vec![with_server_port(0)]);
        let ctx = Context::background();
        srv.start(&ctx).expect("Failed to start server");

        let conn = TcpStream::connect(srv.local_addr().unwrap()).expect("Failed to connect");
        drop(conn);

        // Give time for connect+disconnect to process without panicking
        sleep(Duration::from_millis(100));
        srv.stop(&ctx).unwrap();
    }

    #[test]
    fn client_conn_fc_count_atomics() {
        let client = ClientConn::new("127.0.0.1:9999", SystemTime::now(), None);
        let at = |fc: FunctionCode| &client.fc_count[fc.0 as usize];

        at(FUNC_READ_COILS).fetch_add(5, Ordering::SeqCst);
        at(FUNC_READ_HOLDING_REGISTERS).fetch_add(10, Ordering::SeqCst);
        at(FUNC_WRITE_SINGLE_REGISTER).fetch_add(3, Ordering::SeqCst);

        assert_eq!(at(FUNC_READ_COILS).load(Ordering::SeqCst), 5);
        assert_eq!(at(FUNC_READ_HOLDING_REGISTERS).load(Ordering::SeqCst), 10);
        assert_eq!(at(FUNC_WRITE_SINGLE_REGISTER).load(Ordering::SeqCst), 3);
        // Unset function code should be zero
        assert_eq!(at(FUNC_WRITE_MULTIPLE_COILS).load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fc_snapshot_values() {
        let client = ClientConn::new("127.0.0.1:9999", SystemTime::now(), None);
        client.fc_count[FUNC_READ_COILS.0 as usize].store(100, Ordering::SeqCst);
        client.fc_count[FUNC_WRITE_MULTIPLE_REGISTERS.0 as usize].store(50, Ordering::SeqCst);

        let stats = fc_snapshot(&client);
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[&FUNC_READ_COILS], 100);
        assert_eq!(stats[&FUNC_WRITE_MULTIPLE_REGISTERS], 50);
    }

    #[test]
    fn fc_snapshot_empty() {
        let client = ClientConn::new("127.0.0.1:9999", SystemTime::now(), None);
        assert_eq!(fc_snapshot(&client).len(), 0);
    }

    #[test]
    fn connected_client_string_with_fc_stats() {
        let mut stats = BTreeMap::new();
        stats.insert(FUNC_READ_HOLDING_REGISTERS, 1000);
        stats.insert(FUNC_READ_COILS, 523);
        let client = ConnectedClient {
            remote_addr: "192.168.1.10:54321".into(),
            connected_at: hours_ago(2),
            rx_transactions: 1523,
            tx_transactions: 1520,
            function_code_stats: stats,
        };
        let s = client.to_string();
        assert!(s.contains("fc:"), "missing fc stats section: {}", s);
        assert!(s.contains("ReadCoils=523"), "missing ReadCoils stat: {}", s);
        assert!(s.contains("ReadHoldingRegisters=1000"), "missing ReadHoldingRegisters stat: {}", s);
        // sorted by function code
        assert!(s.find("ReadCoils").unwrap() < s.find("ReadHoldingRegisters").unwrap());
    }

    #[test]
    fn connected_client_string_no_fc_stats() {
        let client = ConnectedClient {
            remote_addr: "10.0.0.1:12345".into(),
            connected_at: SystemTime::now(),
            ..Default::default()
        };
        assert!(!client.to_string().contains("fc:"));
    }

    #[test]
    fn tcp_server_connected_clients_snapshot_with_fc_stats() {
        let srv = TcpServer::new("127.0.0.1", vec![with_server_port(0)]);
        let client = Arc::new(ClientConn::new("10.0.0.5:40000", SystemTime::now(), None));
        client.rx_count.store(150, Ordering::SeqCst);
        client.tx_count.store(149, Ordering::SeqCst);
        client.fc_count[FUNC_READ_COILS.0 as usize].store(50, Ordering::SeqCst);
        client.fc_count[FUNC_READ_HOLDING_REGISTERS.0 as usize].store(100, Ordering::SeqCst);
        srv.insert_client_for_test("10.0.0.5:40000", client);

        let snapshots = srv.connected_clients();
        assert_eq!(snapshots.len(), 1);
        let snap = &snapshots[0];
        assert_eq!(snap.function_code_stats.len(), 2);
        assert_eq!(snap.function_code_stats[&FUNC_READ_COILS], 50);
        assert_eq!(snap.function_code_stats[&FUNC_READ_HOLDING_REGISTERS], 100);
    }
}
