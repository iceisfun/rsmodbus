//! Modbus TCP client examples, one subcommand per function code.
//! Port of `gomodbus/cmd/client/*` (ReadCoils, ReadDiscreteInputs, ...).
//!
//! Usage: `rsmodbus-client <command> [-ip 127.0.0.1] [-port 502] [-unit 1] [-timeout 5s] [-log info]`
//!
//! Spec references are given per command below; see
//! Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Code Descriptions).

use std::process::exit;

use rsmodbus::cli::{ModbusArgs, common_specs, parse_or_exit};
use rsmodbus::common::*;

/// Command table: (name, function code text, handler).
type Handler = fn(&dyn Client, &Context) -> Result<()>;

const COMMANDS: &[(&str, &str, Handler)] = &[
    ("read-coils", "0x01", read_coils),
    ("read-discrete-inputs", "0x02", read_discrete_inputs),
    ("read-holding-registers", "0x03", read_holding_registers),
    ("read-input-registers", "0x04", read_input_registers),
    ("write-single-coil", "0x05", write_single_coil),
    ("write-single-register", "0x06", write_single_register),
    ("read-exception-status", "0x07", read_exception_status),
    ("write-multiple-coils", "0x0F", write_multiple_coils),
    ("write-multiple-registers", "0x10", write_multiple_registers),
    ("read-write-multiple-registers", "0x17", read_write_multiple_registers),
    ("read-device-identification", "0x2B/0x0E", read_device_identification),
];

fn usage() -> ! {
    eprintln!("Usage: rsmodbus-client <command> [flags]\n\nCommands:");
    for (name, fc, _) in COMMANDS {
        eprintln!("  {name:<32} function code {fc}");
    }
    eprintln!("\nFlags (after the command):");
    eprint!("{}", rsmodbus::cli::usage_text("rsmodbus-client", &common_specs()));
    exit(2);
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let Some(cmd) = argv.get(1) else { usage() };
    if cmd == "-h" || cmd == "-help" || cmd == "--help" {
        usage();
    }
    let Some((_, _, handler)) = COMMANDS.iter().find(|(n, _, _)| n == cmd) else {
        eprintln!("unknown command: {cmd}");
        usage();
    };
    let args = ModbusArgs::from_parsed(&parse_or_exit(&format!("rsmodbus-client {cmd}"), &common_specs(), &argv[2..]));

    // Create a Modbus client and connect to the server.
    let client = rsmodbus::cli::create_client(&args);
    let ctx = Context::background();
    if let Err(e) = client.connect(&ctx) {
        println!("Failed to connect to Modbus server: {e}");
        exit(1);
    }
    let result = handler(client.as_ref(), &ctx);
    let _ = client.disconnect(&ctx);
    if result.is_err() {
        exit(1);
    }
}

/// Prints `msg: err` (Go `fmt.Println("Failed ...:", err)`) and returns the error.
fn fail<T>(msg: &str, e: Error) -> Result<T> {
    println!("{msg}: {e}");
    Err(e)
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (01 Read Coils)
fn read_coils(c: &dyn Client, ctx: &Context) -> Result<()> {
    let (start, quantity) = (Address(0), Quantity(10));
    let coils = c.read_coils(ctx, start, quantity).or_else(|e| fail("Failed to read coils", e))?;
    println!("Read {} coils starting at address {}:", quantity.0, start.0);
    for (i, v) in coils.iter().enumerate() {
        println!("Coil {}: {}", start.0 as usize + i, v);
    }
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (02 Read Discrete Inputs)
fn read_discrete_inputs(c: &dyn Client, ctx: &Context) -> Result<()> {
    let (start, quantity) = (Address(0), Quantity(10));
    let inputs = c
        .read_discrete_inputs(ctx, start, quantity)
        .or_else(|e| fail("Failed to read discrete inputs", e))?;
    println!("Read {} discrete inputs starting at address {}:", quantity.0, start.0);
    for (i, v) in inputs.iter().enumerate() {
        println!("Input {}: {}", start.0 as usize + i, v);
    }
    Ok(())
}

/// Prints registers as `Register N: dec (0xHHHH)`.
fn print_registers(start: usize, values: &[u16]) {
    for (i, v) in values.iter().enumerate() {
        println!("Register {}: {} (0x{:04X})", start + i, v, v);
    }
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (03 Read Holding Registers)
fn read_holding_registers(c: &dyn Client, ctx: &Context) -> Result<()> {
    let (start, quantity) = (Address(0), Quantity(10));
    let regs = c
        .read_holding_registers(ctx, start, quantity)
        .or_else(|e| fail("Failed to read holding registers", e))?;
    println!("Read {} holding registers starting at address {}:", quantity.0, start.0);
    print_registers(start.0 as usize, &regs);
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (04 Read Input Registers)
fn read_input_registers(c: &dyn Client, ctx: &Context) -> Result<()> {
    let (start, quantity) = (Address(0), Quantity(10));
    let regs = c
        .read_input_registers(ctx, start, quantity)
        .or_else(|e| fail("Failed to read input registers", e))?;
    println!("Read {} input registers starting at address {}:", quantity.0, start.0);
    print_registers(start.0 as usize, &regs);
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5 (05 Write Single Coil)
fn write_single_coil(c: &dyn Client, ctx: &Context) -> Result<()> {
    let (address, value) = (Address(0), true);
    c.write_single_coil(ctx, address, value).or_else(|e| fail("Failed to write coil", e))?;
    println!("Successfully set coil at address {} to {}", address.0, value);
    // Read back the value to verify it was written.
    let coils = c.read_coils(ctx, address, Quantity(1)).or_else(|e| fail("Failed to read back coil value", e))?;
    if let Some(v) = coils.first() {
        println!("Read back coil {}: {}", address.0, v);
    }
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.6 (06 Write Single Register)
fn write_single_register(c: &dyn Client, ctx: &Context) -> Result<()> {
    let (address, value) = (Address(0), 12345u16);
    c.write_single_register(ctx, address, value).or_else(|e| fail("Failed to write register", e))?;
    println!("Successfully set register at address {} to {} (0x{:04X})", address.0, value, value);
    let regs = c
        .read_holding_registers(ctx, address, Quantity(1))
        .or_else(|e| fail("Failed to read back register value", e))?;
    if let Some(v) = regs.first() {
        println!("Read back register {}: {} (0x{:04X})", address.0, v, v);
    }
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.7 (07 Read Exception Status)
fn read_exception_status(c: &dyn Client, ctx: &Context) -> Result<()> {
    let status = c.read_exception_status(ctx).or_else(|e| fail("Failed to read exception status", e))?;
    println!("Exception Status: {status}");
    println!("\nIndividual exception bits:");
    for i in 0..8 {
        if status.0 & (1 << i) != 0 {
            println!("  Exception bit {i} is set");
        } else {
            println!("  Exception bit {i} is clear");
        }
    }
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (15 Write Multiple Coils)
fn write_multiple_coils(c: &dyn Client, ctx: &Context) -> Result<()> {
    let start = Address(0);
    let values = [true, false, true, true, false];
    c.write_multiple_coils(ctx, start, &values).or_else(|e| fail("Failed to write coils", e))?;
    println!("Successfully wrote {} coils starting at address {}", values.len(), start.0);
    let read = c
        .read_coils(ctx, start, Quantity(values.len() as u16))
        .or_else(|e| fail("Failed to read back coil values", e))?;
    println!("\nVerifying written values:");
    for (i, v) in read.iter().enumerate() {
        println!("Coil {}: {}", start.0 as usize + i, v);
    }
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (16 Write Multiple Registers)
fn write_multiple_registers(c: &dyn Client, ctx: &Context) -> Result<()> {
    let start = Address(0);
    let values = [1000u16, 2000, 3000, 4000, 5000];
    c.write_multiple_registers(ctx, start, &values).or_else(|e| fail("Failed to write registers", e))?;
    println!("Successfully wrote {} registers starting at address {}", values.len(), start.0);
    let read = c
        .read_holding_registers(ctx, start, Quantity(values.len() as u16))
        .or_else(|e| fail("Failed to read back register values", e))?;
    println!("\nVerifying written values:");
    print_registers(start.0 as usize, &read);
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.17 (23 Read/Write Multiple registers)
fn read_write_multiple_registers(c: &dyn Client, ctx: &Context) -> Result<()> {
    let (read_address, read_quantity, write_address) = (Address(10), Quantity(5), Address(20));
    let write_values = [10000u16, 20000, 30000];
    let read = c
        .read_write_multiple_registers(ctx, read_address, read_quantity, write_address, &write_values)
        .or_else(|e| fail("Failed to perform read/write operation", e))?;
    println!("Read {} registers from address {}:", read_quantity.0, read_address.0);
    print_registers(read_address.0 as usize, &read);
    println!("\nWrote {} registers to address {}", write_values.len(), write_address.0);
    print_registers(write_address.0 as usize, &write_values);
    // Verify the written values.
    let verify = c
        .read_holding_registers(ctx, write_address, Quantity(write_values.len() as u16))
        .or_else(|e| fail("Failed to verify written values", e))?;
    println!("\nVerifying written values:");
    print_registers(write_address.0 as usize, &verify);
    Ok(())
}

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (43 / 14 Read Device Identification)
fn read_device_identification(c: &dyn Client, ctx: &Context) -> Result<()> {
    println!("Reading basic device identification...");
    let identity = match c.read_device_identification(ctx, READ_DEVICE_ID_BASIC_STREAM, DeviceIdObjectCode(0)) {
        Ok(i) => i,
        Err(e) => {
            if is_function_not_supported_error(&e) {
                println!("Note: Device identification is not supported by this device");
            } else {
                println!("Error reading device identification: {e}");
            }
            return Err(e);
        }
    };
    println!("Basic Device Information:");
    println!("-------------------------");
    println!("Vendor Name:    {}", identity.get_vendor_name());
    println!("Product Code:   {}", identity.get_product_code());
    println!("Revision:       {}", identity.get_revision());

    println!("\nAttempting to read extended device identification...");
    match c.read_device_identification(ctx, READ_DEVICE_ID_EXTENDED_STREAM, DeviceIdObjectCode(0)) {
        Ok(ext) => {
            println!("Extended Device Information:");
            println!("---------------------------");
            let fields = [
                ("Vendor URL:     ", ext.get_vendor_url()),
                ("Product Name:   ", ext.get_product_name()),
                ("Model Name:     ", ext.get_model_name()),
                ("User App Name:  ", ext.get_user_application_name()),
            ];
            for (label, v) in fields {
                if !v.is_empty() {
                    println!("{label}{v}");
                }
            }
            // Display any additional (extended, >= 0x80) objects.
            let mut found = false;
            for obj in &ext.objects {
                if obj.id.0 >= 0x80 {
                    if !found {
                        println!("\nExtended Objects:");
                        println!("----------------");
                        found = true;
                    }
                    println!("Object 0x{:02X}:    {}", obj.id.0, obj.value);
                }
            }
        }
        Err(e) if !is_function_not_supported_error(&e) => {
            println!("\nError reading extended device identification: {e}");
        }
        Err(_) => println!("\nExtended device identification not supported"),
    }
    Ok(())
}
