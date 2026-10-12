//! Client and Protocol interfaces. Port of `gomodbus/common/client.go`.

use std::sync::Arc;

use super::context::Context;
use super::device::DeviceIdentification;
use super::errors::Result;
use super::logger::LoggerInterface;
use super::types::*;

/// Client is the interface that all Modbus clients must implement.
///
/// All methods take `&self` (implementations are thread-safe); share clients as
/// `Arc<dyn Client>`. `ctx` carries the deadline/cancellation (Go `context.Context`).
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function codes descriptions)
pub trait Client: Send + Sync {
    /// Establishes a connection to the Modbus server.
    fn connect(&self, ctx: &Context) -> Result<()>;

    /// Closes the connection to the Modbus server.
    fn disconnect(&self, ctx: &Context) -> Result<()>;

    /// Returns true if the client is connected to the server.
    fn is_connected(&self) -> bool;

    /// Reads coils from the server (FC 0x01). `address` is the starting address,
    /// `quantity` the number of coils (1..=2000).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Read Coils)
    fn read_coils(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<CoilValue>>;

    /// Reads discrete inputs from the server (FC 0x02), quantity 1..=2000.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (Read Discrete Inputs)
    fn read_discrete_inputs(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<DiscreteInputValue>>;

    /// Reads holding registers from the server (FC 0x03), quantity 1..=125.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Read Holding Registers)
    fn read_holding_registers(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<RegisterValue>>;

    /// Reads input registers from the server (FC 0x04), quantity 1..=125.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Read Input Registers)
    fn read_input_registers(&self, ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<InputRegisterValue>>;

    /// Writes a single coil to the server (FC 0x05).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5 (Write Single Coil)
    fn write_single_coil(&self, ctx: &Context, address: Address, value: CoilValue) -> Result<()>;

    /// Writes a single register to the server (FC 0x06).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.6 (Write Single Register)
    fn write_single_register(&self, ctx: &Context, address: Address, value: RegisterValue) -> Result<()>;

    /// Writes multiple coils to the server (FC 0x0F), 1..=1968 values.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (Write Multiple Coils)
    fn write_multiple_coils(&self, ctx: &Context, address: Address, values: &[CoilValue]) -> Result<()>;

    /// Writes multiple registers to the server (FC 0x10), 1..=123 values.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (Write Multiple Registers)
    fn write_multiple_registers(&self, ctx: &Context, address: Address, values: &[RegisterValue]) -> Result<()>;

    /// Reads and writes multiple registers (FC 0x17): read 1..=125, write 1..=121.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.17 (Read/Write Multiple Registers)
    fn read_write_multiple_registers(
        &self,
        ctx: &Context,
        read_address: Address,
        read_quantity: Quantity,
        write_address: Address,
        write_values: &[RegisterValue],
    ) -> Result<Vec<RegisterValue>>;

    /// Reads the exception status from the server (FC 0x07).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.7 (Read Exception Status)
    fn read_exception_status(&self, ctx: &Context) -> Result<ExceptionStatus>;

    /// Reads device identification data from the server (FC 0x2B / MEI 0x0E).
    /// `read_device_id_code` selects what to read (`READ_DEVICE_ID_BASIC`,
    /// `_REGULAR`, `_EXTENDED` stream access, or `_SPECIFIC` for one object). With
    /// `_SPECIFIC`, `object_id` selects the object; otherwise pass
    /// `DeviceIdObjectCode(0)`.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Read Device Identification)
    fn read_device_identification(
        &self,
        ctx: &Context,
        read_device_id_code: ReadDeviceIdCode,
        object_id: DeviceIdObjectCode,
    ) -> Result<DeviceIdentification>;

    /// Sets the logger for the client (Go `WithLogger`). Takes `Arc<Self>` so an
    /// implementation may configure and return itself or return a copy (as the Go
    /// `BaseClient` does).
    fn with_logger(self: Arc<Self>, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Client>;
}

/// Protocol defines the interface for a Modbus protocol handler: PDU data
/// encoding/decoding for each supported function.
///
/// "PDU data" is everything after the function code byte.
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (PDU), Section 6 (Function Codes)
pub trait Protocol: Send + Sync {
    /// Generates request PDU data to read coils (FC 0x01). Excludes the function code.
    /// Ref: Section 6.1
    fn generate_read_coils_request(&self, address: Address, quantity: Quantity) -> Result<Vec<u8>>;

    /// Parses response PDU data of a read coils request. Returns `quantity` booleans.
    /// Ref: Section 6.1
    fn parse_read_coils_response(&self, data: &[u8], quantity: Quantity) -> Result<Vec<CoilValue>>;

    /// Generates request PDU data to read discrete inputs (FC 0x02). Ref: Section 6.2
    fn generate_read_discrete_inputs_request(&self, address: Address, quantity: Quantity) -> Result<Vec<u8>>;

    /// Parses response PDU data of a read discrete inputs request. Ref: Section 6.2
    fn parse_read_discrete_inputs_response(&self, data: &[u8], quantity: Quantity) -> Result<Vec<DiscreteInputValue>>;

    /// Generates request PDU data to read holding registers (FC 0x03). Ref: Section 6.3
    fn generate_read_holding_registers_request(&self, address: Address, quantity: Quantity) -> Result<Vec<u8>>;

    /// Parses response PDU data of a read holding registers request. Ref: Section 6.3
    fn parse_read_holding_registers_response(&self, data: &[u8], quantity: Quantity) -> Result<Vec<RegisterValue>>;

    /// Generates request PDU data to read input registers (FC 0x04). Ref: Section 6.4
    fn generate_read_input_registers_request(&self, address: Address, quantity: Quantity) -> Result<Vec<u8>>;

    /// Parses response PDU data of a read input registers request. Ref: Section 6.4
    fn parse_read_input_registers_response(&self, data: &[u8], quantity: Quantity) -> Result<Vec<InputRegisterValue>>;

    /// Generates request PDU data to write a single coil (FC 0x05). Ref: Section 6.5
    fn generate_write_single_coil_request(&self, address: Address, value: CoilValue) -> Result<Vec<u8>>;

    /// Parses response PDU data of a write single coil request; returns (address, value).
    /// Ref: Section 6.5
    fn parse_write_single_coil_response(&self, data: &[u8]) -> Result<(Address, CoilValue)>;

    /// Generates request PDU data to write a single register (FC 0x06). Ref: Section 6.6
    fn generate_write_single_register_request(&self, address: Address, value: RegisterValue) -> Result<Vec<u8>>;

    /// Parses response PDU data of a write single register request; returns (address, value).
    /// Ref: Section 6.6
    fn parse_write_single_register_response(&self, data: &[u8]) -> Result<(Address, RegisterValue)>;

    /// Generates request PDU data to write multiple coils (FC 0x0F). Ref: Section 6.11
    fn generate_write_multiple_coils_request(&self, address: Address, values: &[CoilValue]) -> Result<Vec<u8>>;

    /// Parses response PDU data of a write multiple coils request; returns
    /// (starting address, quantity written). Ref: Section 6.11
    fn parse_write_multiple_coils_response(&self, data: &[u8]) -> Result<(Address, Quantity)>;

    /// Generates request PDU data to write multiple registers (FC 0x10). Ref: Section 6.12
    fn generate_write_multiple_registers_request(&self, address: Address, values: &[RegisterValue]) -> Result<Vec<u8>>;

    /// Parses response PDU data of a write multiple registers request; returns
    /// (starting address, quantity written). Ref: Section 6.12
    fn parse_write_multiple_registers_response(&self, data: &[u8]) -> Result<(Address, Quantity)>;

    /// Generates request PDU data to read and write multiple registers (FC 0x17). Ref: Section 6.17
    fn generate_read_write_multiple_registers_request(
        &self,
        read_address: Address,
        read_quantity: Quantity,
        write_address: Address,
        write_values: &[RegisterValue],
    ) -> Result<Vec<u8>>;

    /// Parses response PDU data of a read/write multiple registers request.
    /// Returns the read register values. Ref: Section 6.17
    fn parse_read_write_multiple_registers_response(&self, data: &[u8], read_quantity: Quantity) -> Result<Vec<RegisterValue>>;

    /// Generates request PDU data to read the exception status (FC 0x07; empty). Ref: Section 6.7
    fn generate_read_exception_status_request(&self) -> Result<Vec<u8>>;

    /// Parses response PDU data of a read exception status request. Ref: Section 6.7
    fn parse_read_exception_status_response(&self, data: &[u8]) -> Result<ExceptionStatus>;

    /// Generates request PDU data to read device identification (FC 0x2B / MEI 0x0E).
    /// Ref: Section 6.21
    fn generate_read_device_identification_request(
        &self,
        read_device_id_code: ReadDeviceIdCode,
        object_id: DeviceIdObjectCode,
    ) -> Result<Vec<u8>>;

    /// Parses response PDU data of a read device identification request. Ref: Section 6.21
    fn parse_read_device_identification_response(&self, data: &[u8]) -> Result<DeviceIdentification>;

    /// Returns a NEW Protocol instance using the given logger (as the Go
    /// `ProtocolHandler.WithLogger` does).
    fn with_logger(&self, logger: Arc<dyn LoggerInterface>) -> Arc<dyn Protocol>;
}
