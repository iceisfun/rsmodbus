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

The client (`rsmodbus::client`), server (`rsmodbus::server`) and transports
(`rsmodbus::transport`) implement the `Client`, `Server`, `DataStore` and
`Transport` traits from `rsmodbus::common`; see the module docs of those
modules for constructors and options.

## Testing

```bash
cargo test
```

`rsmodbus::common::test_support` provides `MockRequest`, `MockResponse`,
`MockTransport` and `MockDataStore` (a plain `pub mod`, usable from `tests/`).

## License

Same as the original gomodbus project.
