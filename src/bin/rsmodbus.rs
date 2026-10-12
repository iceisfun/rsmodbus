//! Concurrent request demo. Port of `gomodbus/cmd/modbus/main.go`.
//!
//! Usage: `rsmodbus [host [port]]` (default `localhost`, 502). Issues 10 concurrent
//! Read Holding Registers requests over one connection, exercising transaction
//! id multiplexing (MBAP Transaction Identifier, see the Modbus Messaging on
//! TCP/IP Implementation Guide V1.0b, MBAP header) and
//! Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (03 Read Holding Registers).

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rsmodbus::cli::ModbusArgs;
use rsmodbus::common::*;

fn main() {
    let host = std::env::args().nth(1).unwrap_or_else(|| "localhost".to_string());
    // Optional second argument (not in the Go program): port, default 502.
    let port = std::env::args().nth(2).and_then(|p| p.parse().ok()).unwrap_or(502);

    // Debug logging, 30 s overall timeout, 5 s transport timeout.
    let args = ModbusArgs {
        ip: host,
        port,
        unit_id: 1,
        timeout: Duration::from_secs(5),
        log_level: "debug".into(),
        log_level_id: LogLevel::Debug,
    };
    let ctx = Context::background().with_timeout(Duration::from_secs(30));
    let client: Arc<dyn Client> = rsmodbus::cli::create_client(&args);

    if let Err(e) = client.connect(&ctx) {
        println!("Failed to connect: {e}");
        return;
    }
    println!("Connected to Modbus server");

    let num_requests = 10usize;
    // Collected results/errors (Go used buffered channels).
    let results = Arc::new(Mutex::new(Vec::<String>::new()));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let start_time = Instant::now();

    let mut handles = Vec::new();
    for index in 0..num_requests {
        let (client, ctx, results, errors) = (client.clone(), ctx.clone(), results.clone(), errors.clone());
        handles.push(thread::spawn(move || {
            // Per-request 2 s timeout.
            let req_ctx = ctx.with_timeout(Duration::from_secs(2));
            // Read different holding registers for each request.
            let address = Address((1000 + index * 10) as u16);
            let quantity = Quantity(10);
            println!("Request {index}: Reading {} holding registers starting at {}", quantity.0, address.0);
            match client.read_holding_registers(&req_ctx, address, quantity) {
                Ok(values) => {
                    let result =
                        format!("Request {index} completed: Read {} values from address {}", values.len(), address.0);
                    println!("{result}");
                    results.lock().unwrap().push(result);
                }
                Err(e) => {
                    println!("Request {index} failed: {e}");
                    errors.lock().unwrap().push(format!("request {index} failed: {e}"));
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }

    let results = results.lock().unwrap();
    let errors = errors.lock().unwrap();
    for r in results.iter() {
        println!("Result: {r}");
    }
    for e in errors.iter() {
        println!("Error: {e}");
    }
    println!(
        "\nCompleted {num_requests} requests with {} successes and {} errors in {:?}",
        results.len(),
        errors.len(),
        start_time.elapsed()
    );
    let _ = client.disconnect(&Context::background());
}
