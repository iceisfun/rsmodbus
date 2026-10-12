//! Modbus TCP server demo. Port of `gomodbus/cmd/server/main.go`.
//!
//! Implementation detail, not part of the Modbus spec; the data model it
//! serves follows Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3
//! (MODBUS Data model: coils, discrete inputs, input registers, holding registers).
//!
//! Note: std has no signal handling, so Ctrl-C terminates the process
//! directly instead of running the graceful-stop path Go runs on SIGINT/SIGTERM.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rsmodbus::cli::{FlagKind, FlagSpec, parse_or_exit};
use rsmodbus::common::{
    Address, Context, DEFAULT_TCP_PORT, DataStore, LogLevel, LoggerInterface, Server,
};
use rsmodbus::logging::Logger;
use rsmodbus::server::memory_store::MemoryStore;
use rsmodbus::server::tcp_server::{TcpServer, with_server_data_store, with_server_logger, with_server_port};

fn main() {
    let specs = [
        FlagSpec { name: "address", kind: FlagKind::Str, default: "0.0.0.0", usage: "Server address to bind to" },
        FlagSpec { name: "port", kind: FlagKind::Int, default: "502", usage: "TCP port to listen on" },
        FlagSpec { name: "debug", kind: FlagKind::Bool, default: "false", usage: "Enable debug logging" },
        FlagSpec { name: "preload", kind: FlagKind::Bool, default: "true", usage: "Preload some example data in the memory store" },
    ];
    let argv: Vec<String> = std::env::args().collect();
    let p = parse_or_exit(&argv[0], &specs, &argv[1..]);
    let address = p.string("address");
    let port = p.int("port");
    let debug = p.bool("debug");
    let preload = p.bool("preload");
    let _ = DEFAULT_TCP_PORT; // default documented in the flag table above

    let level = if debug { LogLevel::Debug } else { LogLevel::Info };
    let logger: Arc<dyn LoggerInterface> = Arc::new(Logger::new().with_level(level));
    let ctx = Context::background().with_cancel();

    let store = Arc::new(MemoryStore::new());
    if preload {
        preload_sample_data(&store, &logger);
    }

    let server = Arc::new(TcpServer::new(
        &address,
        vec![
            with_server_port(port as u16),
            with_server_logger(logger.clone()),
            with_server_data_store(store.clone() as Arc<dyn DataStore>),
        ],
    ));

    logger.info(&ctx, format_args!("Starting Modbus TCP server on {}:{}...", address, port));
    if let Err(e) = server.start(&ctx) {
        logger.error(&ctx, format_args!("Failed to start server: {}", e));
        std::process::exit(1);
    }

    // Periodically dump the data store's content (debug only).
    if debug {
        let (store, logger, ctx) = (store.clone(), logger.clone(), ctx.clone());
        thread::spawn(move || {
            while ctx.sleep(Duration::from_secs(10)).is_ok() {
                logger.debug(&ctx, format_args!("Current data store contents:\n{}", store.dump_registers()));
            }
        });
    }

    // Periodically update some registers to demonstrate changing values.
    {
        let (store, ctx) = (store.clone(), ctx.clone());
        thread::spawn(move || {
            let mut counter: u16 = 0;
            while ctx.sleep(Duration::from_secs(1)).is_ok() {
                counter = counter.wrapping_add(1);
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                store.set_input_register(Address(1000), counter);
                store.set_input_register(Address(1001), (now & 0xFFFF) as u16);
                store.set_holding_register(Address(2000), counter);
                store.set_coil(Address(3000), counter.is_multiple_of(2)); // toggles every second
            }
        });
    }

    // Block until the context is cancelled (never, absent signal handling).
    while ctx.sleep(Duration::from_secs(3600)).is_ok() {}
    logger.info(&ctx, format_args!("Server shutdown complete"));
}

/// Initialises the data store with sample values (Go `preloadSampleData`).
fn preload_sample_data(store: &MemoryStore, logger: &Arc<dyn LoggerInterface>) {
    let ctx = Context::background();
    logger.info(&ctx, format_args!("Preloading sample data..."));

    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (Coils: single bit, read-write)
    for (i, v) in [true, false, true, true, false].into_iter().enumerate() {
        store.set_coil(Address(i as u16), v);
    }
    // Ref: Section 4.3 (Discrete Inputs: single bit, read-only)
    for (i, v) in [false, true, false, true, true].into_iter().enumerate() {
        store.set_discrete_input(Address(i as u16), v);
    }
    // Ref: Section 4.3 (Holding Registers: 16-bit, read-write)
    for (i, v) in [1000u16, 2000, 3000, 4000, 5000].into_iter().enumerate() {
        store.set_holding_register(Address(i as u16), v);
    }
    // Ref: Section 4.3 (Input Registers: 16-bit, read-only)
    for (i, v) in [100u16, 200, 300, 400, 500].into_iter().enumerate() {
        store.set_input_register(Address(i as u16), v);
    }

    // Special registers.
    store.set_input_register(Address(1000), 0); // counter (updated)
    store.set_input_register(Address(1001), 0); // timestamp (updated)
    store.set_holding_register(Address(2000), 0); // counter (updated)
    store.set_holding_register(Address(5000), 12345); // fixed value
    store.set_coil(Address(3000), false); // toggle (updated)

    logger.info(&ctx, format_args!("Sample data preloaded"));
    logger.debug(&ctx, format_args!("Initial data store contents:\n{}", store.dump_registers()));
}
