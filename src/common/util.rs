//! Small helpers. Port of `gomodbus/common/util.go` (implementation detail, not spec).

use std::net::TcpListener;

use super::errors::{Error, Result};

/// Finds an available TCP port by listening on port 0 of 127.0.0.1.
/// The listener is closed immediately and the port number returned.
///
/// Implementation detail, not part of the Modbus spec (used by tests/examples).
pub fn find_free_port_tcp() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::wrap("failed to bind to port 0", Error::from(e)))?;
    let addr = listener
        .local_addr()
        .map_err(|e| Error::wrap("unexpected address type", Error::from(e)))?;
    Ok(addr.port())
}

/// Formats bytes the way Go's `%v` prints a `[]byte`: `[1 2 255]`.
/// Used by the protocol/transport debug logs; implementation detail, not spec.
pub fn format_bytes(data: &[u8]) -> String {
    let mut s = String::from("[");
    for (i, b) in data.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&b.to_string());
    }
    s.push(']');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of common/util_test.go: TestFindFreePortTCP
    #[test]
    fn test_find_free_port_tcp() {
        // Get a free port
        let port = find_free_port_tcp().expect("Failed to find free port");

        // Verify the port is valid
        assert!(port > 0, "Invalid port number returned: {}", port);

        // Verify we can actually listen on this port
        let listener = TcpListener::bind(("127.0.0.1", port))
            .unwrap_or_else(|e| panic!("Could not listen on port {}: {}", port, e));

        // Verify multiple calls return different ports
        let port2 = find_free_port_tcp().expect("Failed to find second free port");
        assert_ne!(port, port2, "Got same port ({}) for two consecutive calls", port);
        drop(listener);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(&[]), "[]");
        assert_eq!(format_bytes(&[1, 2, 255]), "[1 2 255]");
    }
}
