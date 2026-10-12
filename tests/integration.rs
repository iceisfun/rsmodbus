//! Integration test: real TCP client against real TCP server over loopback.
//! Port of `gomodbus/integration_test.go`.
//!
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Code
//! Descriptions) and Modbus Messaging on TCP/IP Implementation Guide (MBAP).

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use rsmodbus::client::{TCPClient, with_tcp_logger, with_tcp_unit_id};
use rsmodbus::common::{Address, Client, Context, Quantity, Server, UnitId};
use rsmodbus::common::LogLevel;
use rsmodbus::logging::Logger;
use rsmodbus::server::{
    MemoryStore, TcpServer, with_server_data_store, with_server_listener, with_server_logger,
};
use rsmodbus::transport::{with_port, with_timeout_option, with_transport_logger};

#[test]
fn client_server_integration() {
    // Implementation detail: logger at debug level writing to stdout (captured by the harness).
    let logger: Arc<dyn rsmodbus::common::LoggerInterface> =
        Arc::new(Logger::new().with_level(LogLevel::Debug));
    let ctx = Context::background().with_timeout(Duration::from_secs(30));

    // Server data store with pre-loaded test data
    let store = Arc::new(MemoryStore::new());
    store.set_coil(Address(1000), true);
    store.set_coil(Address(1001), false);
    store.set_coil(Address(1002), true);
    store.set_holding_register(Address(2000), 0x1234);
    store.set_holding_register(Address(2001), 0x5678);
    store.set_input_register(Address(3000), 0xABCD);
    store.set_input_register(Address(3001), 0xEF01);

    // Listener on a random free port
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().unwrap().port();

    let srv = TcpServer::new(
        "127.0.0.1",
        vec![
            with_server_listener(listener),
            with_server_logger(logger.clone()),
            with_server_data_store(store.clone()),
        ],
    );
    srv.start(&ctx).expect("server start");

    let client = TCPClient::new(
        "127.0.0.1",
        vec![
            with_port(port),
            with_timeout_option(Duration::from_secs(5)),
            with_transport_logger(logger.clone()),
        ],
    )
    .with_options(vec![with_tcp_unit_id(UnitId(1)), with_tcp_logger(logger.clone())]);

    client.connect(&ctx).expect("connect");

    // Read coils (FC 01)
    let coils = client.read_coils(&ctx, Address(1000), Quantity(3)).expect("read_coils");
    assert_eq!(coils, vec![true, false, true]);

    // Read holding registers (FC 03)
    let hr = client.read_holding_registers(&ctx, Address(2000), Quantity(2)).expect("fc03");
    assert_eq!(hr, vec![0x1234, 0x5678]);

    // Read input registers (FC 04)
    let ir = client.read_input_registers(&ctx, Address(3000), Quantity(2)).expect("fc04");
    assert_eq!(ir, vec![0xABCD, 0xEF01]);

    // Write single coil (FC 05)
    client.write_single_coil(&ctx, Address(1010), true).expect("fc05");
    assert_eq!(store.get_coil(Address(1010)), Some(true));

    // Write single register (FC 06)
    client.write_single_register(&ctx, Address(2010), 0x4321).expect("fc06");
    assert_eq!(store.get_holding_register(Address(2010)), Some(0x4321));

    // Write multiple coils (FC 15)
    let coil_values = [true, false, true, false];
    client.write_multiple_coils(&ctx, Address(1020), &coil_values).expect("fc15");
    for (i, expected) in coil_values.iter().enumerate() {
        assert_eq!(store.get_coil(Address(1020 + i as u16)), Some(*expected), "coil {i}");
    }

    // Write multiple registers (FC 16)
    let reg_values = [0x1111u16, 0x2222, 0x3333];
    client.write_multiple_registers(&ctx, Address(2020), &reg_values).expect("fc16");
    for (i, expected) in reg_values.iter().enumerate() {
        assert_eq!(store.get_holding_register(Address(2020 + i as u16)), Some(*expected));
    }

    // Read/write multiple registers (FC 23)
    let write_values = [0xAAAAu16, 0xBBBB];
    let read = client
        .read_write_multiple_registers(&ctx, Address(2000), Quantity(2), Address(2030), &write_values)
        .expect("fc23");
    assert_eq!(read, vec![0x1234, 0x5678]);
    for (i, expected) in write_values.iter().enumerate() {
        assert_eq!(store.get_holding_register(Address(2030 + i as u16)), Some(*expected));
    }

    client.disconnect(&Context::background()).expect("disconnect");
    srv.stop(&ctx).expect("stop");
    assert!(!srv.is_running());
}
