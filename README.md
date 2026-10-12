# rsmodbus

A zero-dependency (std only), thread-safe Modbus TCP client/server library for Rust.
It is a faithful port of the Go library `gomodbus`: same package layout, behaviour,
validation order and error messages. No async runtime; blocking `std::net` + threads.

Every type, constant and function carries a `Ref:` comment naming the section of
`Modbus_Application_Protocol_V1_1b3.pdf` (text copy in `docs/`) that defines it.

## Features

* Modbus TCP client and server
* Function codes: 0x01 Read Coils, 0x02 Read Discrete Inputs, 0x03 Read Holding
  Registers, 0x04 Read Input Registers, 0x05 Write Single Coil, 0x06 Write Single
  Register, 0x07 Read Exception Status, 0x0F Write Multiple Coils, 0x10 Write
  Multiple Registers, 0x17 Read/Write Multiple Registers, 0x2B/0x0E Read Device
  Identification
* `Context` (deadline + cancel flag) replaces Go's `context.Context`
* Leveled, structured logging (Trace/Debug/Info/Warn/Error/None), custom
  `Box<dyn Write + Send>` sink, hexdumps at Trace level, `NoopLogger`
* Pluggable `DataStore` trait for the server; in-memory store included
* No dependencies: `[dependencies]` in `Cargo.toml` stays empty

## Layout (Go package -> Rust module)

| Go          | Rust                                   |
|-------------|----------------------------------------|
| `common`    | `src/common/` (types, errors, device, util, logger, context, pdu, request, response, transport, client, server, test_support) |
| `protocol`  | `src/protocol.rs`                      |
| `logging`   | `src/logging.rs`                       |
| `transport` | `src/transport/`                       |
| `client`    | `src/client/`                          |
| `server`    | `src/server/`                          |
| `cmd/*`     | `src/bin/*`                            |

Naming: Go `FooBar` types stay `FooBar` (`UnitID` -> `UnitId`); methods and
functions are snake_case (`GetPDU` -> `get_pdu`); constants are SCREAMING_SNAKE
(`FuncReadCoils` -> `FUNC_READ_COILS`); sentinel errors are `Error` variants
(`ErrTimeout` -> `Error::Timeout`, `errors.Is` -> `err.is(&Error::Timeout)`).
Interfaces are traits used as `Arc<dyn Trait>`; `ctx` parameters are `&Context`.

## Usage

### Contexts

```rust
use std::time::Duration;
use rsmodbus::common::Context;

let ctx = Context::background().with_timeout(Duration::from_secs(2));
// ... pass &ctx to client calls; ctx.cancel() aborts early.
```

### Encoding and decoding PDUs

```rust
use rsmodbus::common::*;
use rsmodbus::protocol::ProtocolHandler;

let p = ProtocolHandler::new();

// Request data for FC 0x03 (Section 6.3): address 100, quantity 2
let data = p.generate_read_holding_registers_request(Address(100), Quantity(2))?;
assert_eq!(data, [0x00, 0x64, 0x00, 0x02]);

// Parse a response: byte count 4, registers 0x1234 and 0xABCD
let regs = p.parse_read_holding_registers_response(&[4, 0x12, 0x34, 0xAB, 0xCD], Quantity(2))?;
assert_eq!(regs, vec![0x1234, 0xABCD]);
# Ok::<(), rsmodbus::common::Error>(())
```

### Errors

```rust
use rsmodbus::common::*;

fn handle(err: &Error) {
    if is_function_not_supported_error(err) {
        println!("device does not support this function");
    } else if err.is_timeout() {
        println!("timed out");
    } else if let Error::Modbus(e) = err {
        println!("exception {} for {}", e.exception_code, e.function_code);
    }
}
```

### Logging

```rust
use rsmodbus::common::{Context, LogLevel, LoggerInterface};
use rsmodbus::logging::Logger;

let logger = Logger::new().with_level(LogLevel::Debug); // stdout by default
logger.info(&Context::background(), format_args!("connected to {}", "127.0.0.1"));
```

### Client and server

```rust
use std::sync::Arc;
use rsmodbus::common::{Address, Client, Context, Quantity, Server, UnitId};
use rsmodbus::client::{TCPClient, with_tcp_unit_id};
use rsmodbus::server::{MemoryStore, TcpServer, with_server_data_store, with_server_port};
use rsmodbus::transport::with_port;

let ctx = Context::background();
let store = Arc::new(MemoryStore::new());
store.set_holding_register(Address(0), 0x1234);

// Port 0 asks the OS for a free port; use local_addr() to find it.
let srv = TcpServer::new("127.0.0.1", vec![with_server_port(0), with_server_data_store(store)]);
srv.start(&ctx)?;
let port = srv.local_addr().unwrap().port();

let client = TCPClient::new("127.0.0.1", vec![with_port(port)])
    .with_options(vec![with_tcp_unit_id(UnitId(1))]);
client.connect(&ctx)?;
let regs = client.read_holding_registers(&ctx, Address(0), Quantity(1))?;
assert_eq!(regs, vec![0x1234]);
client.disconnect(&ctx)?;
srv.stop(&ctx)?;
# Ok::<(), rsmodbus::common::Error>(())
```

Other pieces: `client::transport_reconnecting::ReconnectingTransport` (lazy
redial after errors), `client::transport_direct::DirectTransport` (connect
once), `server::tcp_server::with_on_client_connect` / `with_on_client_disconnect`
callbacks, and custom function handlers via `Server::set_handler`. A full
client/server round trip is in `tests/integration.rs`.

### Command-line programs

`rsmodbus-server`, `rsmodbus-client <command>`, `rsmodbus-logger` and `rsmodbus`
(ports of gomodbus `cmd/*`) are documented in [docs/CLI.md](docs/CLI.md).

## Testing

```bash
cargo test
```

`rsmodbus::common::test_support` provides `MockRequest`, `MockResponse`,
`MockTransport` and `MockDataStore` (a plain `pub mod`, usable from `tests/`).

## License

Same as the original gomodbus project.
