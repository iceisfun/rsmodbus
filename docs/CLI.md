# Command-line programs

Ports of gomodbus `cmd/*`. All are std-only binaries built with `cargo build`
and live in `src/bin/`; argument parsing is hand written in `src/cli.rs`
(accepts `-flag value`, `--flag value`, `-flag=value`; Go-style durations such
as `5s`, `250ms`, `1m30s`).

| Go program | Rust binary |
|---|---|
| `cmd/server` | `rsmodbus-server` |
| `cmd/client/*` (11 examples) | `rsmodbus-client <command>` |
| `cmd/logger` | `rsmodbus-logger` |
| `cmd/modbus` | `rsmodbus` |

## Common client arguments

Used by every `rsmodbus-client` command (Go `cmd/args`):

- `-ip`: Modbus server IP address (default `127.0.0.1`)
- `-port`: Modbus server port (default `502`)
- `-unit`: Modbus unit ID (slave ID) (default `1`)
- `-timeout`: timeout for Modbus operations (default `5s`)
- `-log`: log level `debug`, `info`, `warn`, `error` (default `info`; unknown values fall back to `info`)

## rsmodbus-server

```
cargo run --bin rsmodbus-server -- -address 0.0.0.0 -port 1502 -debug
```

Flags: `-address` (default `0.0.0.0`), `-port` (default `502`), `-debug`
(debug logging and a data store dump every 10 s), `-preload` (default `true`,
loads sample coils, discrete inputs, holding and input registers).
Every second a background thread updates input registers 1000/1001, holding
register 2000 and coil 3000, so clients can see values change.
Ctrl-C ends the process immediately: std has no signal handling, so the
graceful-stop path Go runs on SIGINT/SIGTERM does not exist here.

## rsmodbus-client

```
cargo run --bin rsmodbus-client -- read-coils -ip 127.0.0.1 -port 1502
```

Like the Go examples, each command uses fixed example parameters and prints
the results (write commands read the values back to verify).

| Command | Function code | Spec section (Modbus_Application_Protocol_V1_1b3) |
|---|---|---|
| `read-coils` | 0x01 | 6.1 |
| `read-discrete-inputs` | 0x02 | 6.2 |
| `read-holding-registers` | 0x03 | 6.3 |
| `read-input-registers` | 0x04 | 6.4 |
| `write-single-coil` | 0x05 | 6.5 |
| `write-single-register` | 0x06 | 6.6 |
| `read-exception-status` | 0x07 | 6.7 |
| `write-multiple-coils` | 0x0F | 6.11 |
| `write-multiple-registers` | 0x10 | 6.12 |
| `read-write-multiple-registers` | 0x17 | 6.17 |
| `read-device-identification` | 0x2B / 0x0E | 6.21 |

Exit status is 1 when connecting or the operation fails.

## rsmodbus-logger

Shows a custom `LoggerInterface` implementation (`[MODBUS]` prefix, microsecond
timestamps, level filtering) handed to both the client and its transport.
Connects to `127.0.0.1:502`, unit 1, reads 10 holding registers.

```
cargo run --bin rsmodbus-logger
```

## rsmodbus

Concurrency demo: connects to `[host [port]]` (default `localhost`, 502; the port argument is an addition to the Go program) and
issues 10 simultaneous Read Holding Registers requests over one connection
(addresses 1000, 1010, ..., 1090; 2 s per request, 30 s overall), then prints
a summary. Run it against `rsmodbus-server` to see the transaction pool at work.

```
cargo run --bin rsmodbus -- 127.0.0.1 1502
```
