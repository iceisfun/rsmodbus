//! Core Modbus value types, function codes, exception codes and protocol constants.
//!
//! Port of `gomodbus/common/types.go`.
//!
//! Naming rule used across the whole crate: a Go identifier `FooBar` becomes
//! `FooBar` for types (with `ID` -> `Id`, e.g. `UnitID` -> `UnitId`), `foo_bar`
//! for functions/methods and `FOO_BAR` for constants (`FuncReadCoils` ->
//! `FUNC_READ_COILS`).

use std::fmt;
use std::ops::{Add, Sub};

// Implementation detail (not part of the Modbus spec): generates the small
// integer newtypes that mirror Go's typed integers (`type Address uint16`).
macro_rules! int_newtype {
    ($(#[$meta:meta])* $name:ident, $prim:ty) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub $prim);

        impl From<$prim> for $name {
            fn from(v: $prim) -> Self {
                $name(v)
            }
        }

        impl From<$name> for $prim {
            fn from(v: $name) -> $prim {
                v.0
            }
        }

        // Go typed integers wrap on overflow; mirror that with wrapping ops.
        impl Add for $name {
            type Output = $name;
            fn add(self, rhs: $name) -> $name {
                $name(self.0.wrapping_add(rhs.0))
            }
        }

        impl Add<$prim> for $name {
            type Output = $name;
            fn add(self, rhs: $prim) -> $name {
                $name(self.0.wrapping_add(rhs))
            }
        }

        impl Sub for $name {
            type Output = $name;
            fn sub(self, rhs: $name) -> $name {
                $name(self.0.wrapping_sub(rhs.0))
            }
        }

        impl Sub<$prim> for $name {
            type Output = $name;
            fn sub(self, rhs: $prim) -> $name {
                $name(self.0.wrapping_sub(rhs))
            }
        }
    };
}

// Implementation detail: plain decimal Display for the purely numeric newtypes
// (the code-like types below have hand-written names instead).
macro_rules! numeric_display {
    ($($name:ident),*) => {$(
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    )*};
}

int_newtype!(
    /// TransactionId is a unique identifier for a transaction.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP 7 bytes); field layout: MODBUS Messaging on TCP/IP Implementation Guide, Section 3.1.3 (MBAP header), outside the supplied spec
    TransactionId,
    u16
);

int_newtype!(
    /// ProtocolId identifies the protocol used (0 = Modbus).
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP 7 bytes); field layout: MODBUS Messaging on TCP/IP Implementation Guide, Section 3.1.3 (MBAP header), outside the supplied spec
    ProtocolId,
    u16
);

int_newtype!(
    /// UnitId identifies a specific device on a Modbus network.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (MBAP 7 bytes); field layout: MODBUS Messaging on TCP/IP Implementation Guide, Section 3.1.3 (MBAP header), outside the supplied spec
    UnitId,
    u8
);

int_newtype!(
    /// ExceptionCode represents an exception code in a Modbus response.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
    ExceptionCode,
    u8
);

int_newtype!(
    /// FunctionCode represents a Modbus function code.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (MODBUS Function Codes)
    FunctionCode,
    u8
);

int_newtype!(
    /// Address represents a Modbus address (coil, register, etc.).
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (MODBUS Data Model)
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.4 (Addressing Model - specifies 0-65535 range)
    Address,
    u16
);

int_newtype!(
    /// Quantity represents the number of coils or registers to read/write.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, e.g., Section 6.1 (Read Coils Request PDU defines "Quantity of Coils")
    Quantity,
    u16
);

int_newtype!(
    /// ExceptionStatus represents the return value from ReadExceptionStatus
    /// (eight "output coils" packed into one byte).
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.7 (Read Exception Status)
    ExceptionStatus,
    u8
);

int_newtype!(
    /// ReadDeviceIdCode represents a device identification access type.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Read Device Identification)
    ReadDeviceIdCode,
    u8
);

int_newtype!(
    /// DeviceIdObjectCode represents a device identification object code.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Read Device Identification)
    DeviceIdObjectCode,
    u8
);

int_newtype!(
    /// MeiType represents a Modbus Encapsulated Interface type.
    /// Used in function code 0x2B (Modbus Encapsulated Interface); it indicates
    /// which sub-function to execute.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.19 (Encapsulated Interface Transport) / 6.21
    MeiType,
    u8
);

int_newtype!(
    /// ConformityLevel indicates the device's identification conformity level.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response parameters: conformity level)
    ConformityLevel,
    u8
);

int_newtype!(
    /// MoreFollows indicates whether additional device identification objects are
    /// available in a subsequent request. Used in Read Device Identification
    /// (FC 0x2B/0x0E) responses.
    ///
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response PDU)
    MoreFollows,
    u8
);

numeric_display!(TransactionId, ProtocolId, UnitId, Address, Quantity);

/// CoilValue represents a coil value.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Read Coils) and 6.5 (Write Single Coil)
pub type CoilValue = bool;

/// DiscreteInputValue represents a discrete input value.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (Read Discrete Inputs)
pub type DiscreteInputValue = bool;

/// RegisterValue represents a holding register value.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Read Holding Registers)
pub type RegisterValue = u16;

/// InputRegisterValue represents an input register value.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Read Input Registers)
pub type InputRegisterValue = u16;

// ---------------------------------------------------------------------------
// Function codes
// ---------------------------------------------------------------------------

// Function codes as defined by the Modbus specification.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Codes)

/// Read Coils (0x01). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1
pub const FUNC_READ_COILS: FunctionCode = FunctionCode(0x01);
/// Read Discrete Inputs (0x02). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2
pub const FUNC_READ_DISCRETE_INPUTS: FunctionCode = FunctionCode(0x02);
/// Read Holding Registers (0x03). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3
pub const FUNC_READ_HOLDING_REGISTERS: FunctionCode = FunctionCode(0x03);
/// Read Input Registers (0x04). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4
pub const FUNC_READ_INPUT_REGISTERS: FunctionCode = FunctionCode(0x04);
/// Write Single Coil (0x05). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5
pub const FUNC_WRITE_SINGLE_COIL: FunctionCode = FunctionCode(0x05);
/// Write Single Register (0x06). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.6
pub const FUNC_WRITE_SINGLE_REGISTER: FunctionCode = FunctionCode(0x06);
/// Read Exception Status (0x07). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.7
pub const FUNC_READ_EXCEPTION_STATUS: FunctionCode = FunctionCode(0x07);
/// Write Multiple Coils (0x0F). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11
pub const FUNC_WRITE_MULTIPLE_COILS: FunctionCode = FunctionCode(0x0F);
/// Write Multiple Registers (0x10). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12
pub const FUNC_WRITE_MULTIPLE_REGISTERS: FunctionCode = FunctionCode(0x10);
/// Read/Write Multiple Registers (0x17). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.17
pub const FUNC_READ_WRITE_MULTIPLE_REGISTERS: FunctionCode = FunctionCode(0x17);
/// Encapsulated Interface Transport / Read Device Identification (0x2B, MEI 0x0E).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.19 (MEI Transport) and 6.21
pub const FUNC_READ_DEVICE_IDENTIFICATION: FunctionCode = FunctionCode(0x2B);

// ---------------------------------------------------------------------------
// Exception codes
// ---------------------------------------------------------------------------

// Exception codes.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Codes)

/// Illegal Function (0x01). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 01)
pub const EXCEPTION_FUNCTION_CODE_NOT_SUPPORTED: ExceptionCode = ExceptionCode(0x01);
/// Illegal Data Address (0x02). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 02)
pub const EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE: ExceptionCode = ExceptionCode(0x02);
/// Illegal Data Value (0x03). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 03)
pub const EXCEPTION_INVALID_DATA_VALUE: ExceptionCode = ExceptionCode(0x03);
/// Server Device Failure (0x04). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 04)
pub const EXCEPTION_SERVER_DEVICE_FAILURE: ExceptionCode = ExceptionCode(0x04);
/// Acknowledge (0x05). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 05)
pub const EXCEPTION_ACKNOWLEDGE: ExceptionCode = ExceptionCode(0x05);
/// Server Device Busy (0x06). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 06)
pub const EXCEPTION_SERVER_DEVICE_BUSY: ExceptionCode = ExceptionCode(0x06);
/// Memory Parity Error (0x08). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 08)
pub const EXCEPTION_MEMORY_PARITY_ERROR: ExceptionCode = ExceptionCode(0x08);
/// Gateway Path Unavailable (0x0A). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 0A)
pub const EXCEPTION_GATEWAY_PATH_UNAVAILABLE: ExceptionCode = ExceptionCode(0x0A);
/// Gateway Target Device Failed to Respond (0x0B). Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (code 0B)
pub const EXCEPTION_GATEWAY_TARGET_NO_RESPONSE: ExceptionCode = ExceptionCode(0x0B);

// ---------------------------------------------------------------------------
// MEI / Read Device Identification
// ---------------------------------------------------------------------------

/// MEI type for reading device identification (0x0E).
///
/// Other MEI types from the specification could be added here:
/// 0x0D - CANopen General Reference Request and Response PDU.
/// All other values (0x00-0x0C, 0x0F-0xFF) are reserved.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21
pub const MEI_READ_DEVICE_ID: MeiType = MeiType(0x0E);

/// Requests basic device identification (stream access for objects 0x00-0x02).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Request parameters: ReadDevId codes 01-04)
pub const READ_DEVICE_ID_BASIC_STREAM: ReadDeviceIdCode = ReadDeviceIdCode(0x01);
/// Requests regular device identification (stream access through UserApplicationName).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Request parameters: ReadDevId codes 01-04)
pub const READ_DEVICE_ID_REGULAR_STREAM: ReadDeviceIdCode = ReadDeviceIdCode(0x02);
/// Requests extended device identification (stream access for all objects).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Request parameters: ReadDevId codes 01-04)
pub const READ_DEVICE_ID_EXTENDED_STREAM: ReadDeviceIdCode = ReadDeviceIdCode(0x03);
/// Requests a specific identification object (individual access).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Request parameters: ReadDevId codes 01-04)
pub const READ_DEVICE_ID_SPECIFIC_OBJECT: ReadDeviceIdCode = ReadDeviceIdCode(0x04);

/// Alias of [`READ_DEVICE_ID_BASIC_STREAM`] (old name kept for compatibility, as in Go).
pub const READ_DEVICE_ID_BASIC: ReadDeviceIdCode = READ_DEVICE_ID_BASIC_STREAM;
/// Alias of [`READ_DEVICE_ID_REGULAR_STREAM`].
pub const READ_DEVICE_ID_REGULAR: ReadDeviceIdCode = READ_DEVICE_ID_REGULAR_STREAM;
/// Alias of [`READ_DEVICE_ID_EXTENDED_STREAM`].
pub const READ_DEVICE_ID_EXTENDED: ReadDeviceIdCode = READ_DEVICE_ID_EXTENDED_STREAM;
/// Alias of [`READ_DEVICE_ID_SPECIFIC_OBJECT`].
pub const READ_DEVICE_ID_SPECIFIC: ReadDeviceIdCode = READ_DEVICE_ID_SPECIFIC_OBJECT;

// Conformity levels for device identification.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response parameters: conformity level)

/// Basic identification (stream access only).
pub const CONFORMITY_LEVEL_BASIC: ConformityLevel = ConformityLevel(0x01);
/// Regular identification (stream access only).
pub const CONFORMITY_LEVEL_REGULAR: ConformityLevel = ConformityLevel(0x02);
/// Extended identification (stream access only).
pub const CONFORMITY_LEVEL_EXTENDED: ConformityLevel = ConformityLevel(0x03);
/// Basic identification (stream + individual access).
pub const CONFORMITY_LEVEL_BASIC_INDIVIDUAL: ConformityLevel = ConformityLevel(0x81);
/// Regular identification (stream + individual access).
pub const CONFORMITY_LEVEL_REGULAR_INDIVIDUAL: ConformityLevel = ConformityLevel(0x82);
/// Extended identification (stream + individual access).
pub const CONFORMITY_LEVEL_EXTENDED_INDIVIDUAL: ConformityLevel = ConformityLevel(0x83);

// MoreFollows values for device identification responses.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response PDU)

/// No more objects available (0x00).
pub const MORE_FOLLOWS_NO: MoreFollows = MoreFollows(0x00);
/// More objects available (0xFF), request again with NextObjectID.
pub const MORE_FOLLOWS_YES: MoreFollows = MoreFollows(0xFF);

// Device identification object IDs.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
//
// Objects 0x07-0x7F are reserved for future standard objects; objects
// 0x80-0xFF are vendor-specific extended (private) objects.

/// VendorName - mandatory basic object.
pub const DEVICE_ID_VENDOR_NAME: DeviceIdObjectCode = DeviceIdObjectCode(0x00);
/// ProductCode - mandatory basic object.
pub const DEVICE_ID_PRODUCT_CODE: DeviceIdObjectCode = DeviceIdObjectCode(0x01);
/// MajorMinorRevision - mandatory basic object.
pub const DEVICE_ID_MAJOR_MINOR_REVISION: DeviceIdObjectCode = DeviceIdObjectCode(0x02);
/// VendorUrl - standard regular object.
pub const DEVICE_ID_VENDOR_URL: DeviceIdObjectCode = DeviceIdObjectCode(0x03);
/// ProductName - standard regular object.
pub const DEVICE_ID_PRODUCT_NAME: DeviceIdObjectCode = DeviceIdObjectCode(0x04);
/// ModelName - standard regular object.
pub const DEVICE_ID_MODEL_NAME: DeviceIdObjectCode = DeviceIdObjectCode(0x05);
/// UserApplicationName - standard regular object.
pub const DEVICE_ID_USER_APP_NAME: DeviceIdObjectCode = DeviceIdObjectCode(0x06);

// ---------------------------------------------------------------------------
// Display ("String()") implementations
// ---------------------------------------------------------------------------

/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response parameters: conformity level)
impl fmt::Display for ConformityLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            CONFORMITY_LEVEL_BASIC => f.write_str("Basic (stream)"),
            CONFORMITY_LEVEL_REGULAR => f.write_str("Regular (stream)"),
            CONFORMITY_LEVEL_EXTENDED => f.write_str("Extended (stream)"),
            CONFORMITY_LEVEL_BASIC_INDIVIDUAL => f.write_str("Basic (stream+individual)"),
            CONFORMITY_LEVEL_REGULAR_INDIVIDUAL => f.write_str("Regular (stream+individual)"),
            CONFORMITY_LEVEL_EXTENDED_INDIVIDUAL => f.write_str("Extended (stream+individual)"),
            other => write!(f, "Unknown(0x{:02X})", other.0),
        }
    }
}

/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response PDU)
impl fmt::Display for MoreFollows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            MORE_FOLLOWS_NO => f.write_str("No"),
            MORE_FOLLOWS_YES => f.write_str("Yes"),
            other => write!(f, "Unknown(0x{:02X})", other.0),
        }
    }
}

/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6 (Function Codes)
/// An exception response (function code with bit 7 set) renders as
/// `Exception(<original function>)`; Ref: Section 7.
impl fmt::Display for FunctionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            FUNC_READ_COILS => "ReadCoils",
            FUNC_READ_DISCRETE_INPUTS => "ReadDiscreteInputs",
            FUNC_READ_HOLDING_REGISTERS => "ReadHoldingRegisters",
            FUNC_READ_INPUT_REGISTERS => "ReadInputRegisters",
            FUNC_WRITE_SINGLE_COIL => "WriteSingleCoil",
            FUNC_WRITE_SINGLE_REGISTER => "WriteSingleRegister",
            FUNC_READ_EXCEPTION_STATUS => "ReadExceptionStatus",
            FUNC_WRITE_MULTIPLE_COILS => "WriteMultipleCoils",
            FUNC_WRITE_MULTIPLE_REGISTERS => "WriteMultipleRegisters",
            FUNC_READ_WRITE_MULTIPLE_REGISTERS => "ReadWriteMultipleRegisters",
            FUNC_READ_DEVICE_IDENTIFICATION => "ReadDeviceIdentification",
            other => {
                // If it's an exception response
                if is_exception(other.0) {
                    let original = get_original_function_code(other.0);
                    return write!(f, "Exception({})", FunctionCode(original));
                }
                return write!(f, "Unknown(0x{:02X})", other.0);
            }
        };
        f.write_str(name)
    }
}

/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Codes)
impl fmt::Display for ExceptionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            EXCEPTION_FUNCTION_CODE_NOT_SUPPORTED => "FunctionCodeNotSupported",
            EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE => "DataAddressNotAvailable",
            EXCEPTION_INVALID_DATA_VALUE => "InvalidDataValue",
            EXCEPTION_SERVER_DEVICE_FAILURE => "ServerDeviceFailure",
            EXCEPTION_ACKNOWLEDGE => "Acknowledge",
            EXCEPTION_SERVER_DEVICE_BUSY => "ServerDeviceBusy",
            EXCEPTION_MEMORY_PARITY_ERROR => "MemoryParityError",
            EXCEPTION_GATEWAY_PATH_UNAVAILABLE => "GatewayPathUnavailable",
            EXCEPTION_GATEWAY_TARGET_NO_RESPONSE => "GatewayTargetNoResponse",
            other => return write!(f, "Unknown(0x{:02X})", other.0),
        };
        f.write_str(name)
    }
}

/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (MEI type, 0x0E = Read Device Identification)
impl fmt::Display for MeiType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            MEI_READ_DEVICE_ID => f.write_str("ReadDeviceIdentification"),
            other => write!(f, "UnknownMEIType(0x{:02X})", other.0),
        }
    }
}

/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Request parameters: ReadDevId codes 01-04)
impl fmt::Display for ReadDeviceIdCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            READ_DEVICE_ID_BASIC_STREAM => f.write_str("BasicStream"),
            READ_DEVICE_ID_REGULAR_STREAM => f.write_str("RegularStream"),
            READ_DEVICE_ID_EXTENDED_STREAM => f.write_str("ExtendedStream"),
            READ_DEVICE_ID_SPECIFIC_OBJECT => f.write_str("SpecificObject"),
            other => write!(f, "UnknownReadDeviceIDCode(0x{:02X})", other.0),
        }
    }
}

/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
impl fmt::Display for DeviceIdObjectCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            DEVICE_ID_VENDOR_NAME => "VendorName",
            DEVICE_ID_PRODUCT_CODE => "ProductCode",
            DEVICE_ID_MAJOR_MINOR_REVISION => "MajorMinorRevision",
            DEVICE_ID_VENDOR_URL => "VendorURL",
            DEVICE_ID_PRODUCT_NAME => "ProductName",
            DEVICE_ID_MODEL_NAME => "ModelName",
            DEVICE_ID_USER_APP_NAME => "UserApplicationName",
            other => {
                // 0x80-0xFF are vendor-specific extended objects (Section 6.21 object Id list).
                if other.0 >= 0x80 {
                    return write!(f, "ExtendedObject(0x{:02X})", other.0);
                }
                return write!(f, "UnknownObject(0x{:02X})", other.0);
            }
        };
        f.write_str(name)
    }
}

/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.7 (Read Exception Status)
/// ExceptionStatus is a bit field (8 "coils"); the display lists which bits are set.
impl fmt::Display for ExceptionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bits: Vec<String> = (0..8)
            .filter(|i| self.0 & (1 << i) != 0)
            .map(|i| i.to_string())
            .collect();
        if bits.is_empty() {
            return f.write_str("ExceptionStatus(None)");
        }
        write!(f, "ExceptionStatus(Bits: [{}], Value: 0x{:02X})", bits.join(" "), self.0)
    }
}

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (Protocol description; MBAP header layout in MODBUS Messaging on TCP/IP Implementation Guide, Section 3.1.3)

/// MBAP header length: Transaction ID (2) + Protocol ID (2) + Length (2) + Unit ID (1).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 (ADU size: MBAP 7 bytes); layout: MODBUS Messaging on TCP/IP Implementation Guide, Section 3.1.3
pub const TCP_HEADER_LENGTH: usize = 7;
/// Maximum PDU length (256 bytes RS485 ADU - 1 unit id - 2 CRC = 253).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1
pub const MAX_PDU_LENGTH: usize = 253;
/// Maximum ADU length on TCP: 253 bytes PDU + 7 bytes MBAP = 260.
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.1 ("TCP MODBUS ADU = 253 bytes + MBAP (7 bytes) = 260 bytes")
pub const MAX_ADU_LENGTH: usize = 260;
/// Default Modbus TCP port (IANA registered port 502; not defined in this PDF, implementation default).
pub const DEFAULT_TCP_PORT: u16 = 502;

// Data sizes.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.2 (Data Encoding)
// BYTES_PER_COIL and BYTES_PER_DISCRETE_INPUT refer to how individual statuses are
// packed, not that each coil/input uses a full byte in a multi-item request/response.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Read Coils Response - "coil status ... packed as one coil per bit")

/// Represents a single status bit; multiple are packed.
pub const BYTES_PER_COIL: usize = 1;
/// Represents a single status bit; multiple are packed.
pub const BYTES_PER_DISCRETE_INPUT: usize = 1;
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Read Holding Registers Response - "Each register data in two bytes")
pub const BYTES_PER_REGISTER: usize = 2;
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Read Input Registers Response)
pub const BYTES_PER_INPUT_REGISTER: usize = 2;

// Modbus limits.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Sections 6.1, 6.2, 6.3, 6.4, 6.11, 6.12, 6.17 (function specific limits)

/// Maximum number of coils in Read Coils/Discrete Inputs (0x07D0). Ref: Section 6.1, 6.2
pub const MAX_COIL_COUNT: u16 = 2000;
/// Maximum number of coils in Write Multiple Coils (0x07B0). Ref: Section 6.11
pub const MAX_WRITE_COIL_COUNT: u16 = 1968;
/// Maximum number of registers in Read requests (0x007D). Ref: Section 6.3, 6.4
pub const MAX_REGISTER_COUNT: u16 = 125;
/// Maximum number of registers in Write Multiple Registers (0x007B). Ref: Section 6.12
pub const MAX_WRITE_REGISTER_COUNT: u16 = 123;
/// Maximum number of registers to read in Read/Write Multiple (0x007D). Ref: Section 6.17
pub const MAX_READ_WRITE_READ_COUNT: u16 = 125;
/// Maximum number of registers to write in Read/Write Multiple (0x0079). Ref: Section 6.17
pub const MAX_READ_WRITE_WRITE_COUNT: u16 = 121;

// Coil values as defined in the Modbus specification.
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5 (Write Single Coil)
//
// "The requested ON/OFF state is specified by a constant in the Coil Value field.
// A value of 0xFF00 requests the coil to be ON.
// A value of 0x0000 requests the coil to be OFF.
// All other values are illegal and will not affect the coil."

/// ON value for coils in register format. Ref: Section 6.5
pub const COIL_ON_U16: u16 = 0xFF00;
/// OFF value for coils in register format. Ref: Section 6.5
pub const COIL_OFF_U16: u16 = 0x0000;

/// Standard identifier for Modbus TCP (protocol id 0).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, MODBUS Messaging on TCP/IP Implementation Guide, Section 3.1.3 (MBAP header, Protocol Identifier), outside the supplied spec
pub const TCP_PROTOCOL_IDENTIFIER: ProtocolId = ProtocolId(0);

/// Bit set in the function code to indicate an exception response (fc + 0x80).
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
pub const EXCEPTION_BIT: u8 = 0x80;

/// Reports whether a raw function code byte represents an exception response.
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
pub fn is_exception(function_code: u8) -> bool {
    (function_code & EXCEPTION_BIT) != 0
}

/// Reports whether a [`FunctionCode`] represents an exception response.
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
pub fn is_function_exception(function_code: FunctionCode) -> bool {
    is_exception(function_code.0)
}

/// Extracts the original function code byte from an exception function code.
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
pub fn get_original_function_code(exception_code: u8) -> u8 {
    exception_code & !EXCEPTION_BIT
}

/// Extracts the original [`FunctionCode`] from an exception function code.
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception Responses)
pub fn get_original_function(exception_code: FunctionCode) -> FunctionCode {
    FunctionCode(get_original_function_code(exception_code.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of common/test/types_test.go

    #[test]
    fn test_type_aliases() {
        let address: Address = Address(100);
        assert_eq!(address.0, 100, "Address conversion failed");

        let quantity: Quantity = Quantity(10);
        assert_eq!(quantity.0, 10, "Quantity conversion failed");

        let coil_value: CoilValue = true;
        assert!(coil_value, "CoilValue conversion failed");

        let discrete_input_value: DiscreteInputValue = false;
        assert!(!discrete_input_value, "DiscreteInputValue conversion failed");

        let register_value: RegisterValue = 12345;
        assert_eq!(register_value, 12345, "RegisterValue conversion failed");

        let input_register_value: InputRegisterValue = 54321;
        assert_eq!(input_register_value, 54321, "InputRegisterValue conversion failed");
    }

    #[test]
    fn test_address_arithmetic() {
        let base_address = Address(100);
        let offset = Address(50);

        // Addition
        assert_eq!(base_address + offset, Address(150));
        // Addition with constant
        assert_eq!(base_address + 25, Address(125));
        // Addition with different type that gets converted
        assert_eq!(base_address + Address(10), Address(110));
    }

    #[test]
    fn test_quantity_arithmetic() {
        let base_quantity = Quantity(100);
        let offset = Quantity(50);

        // Addition
        assert_eq!(base_quantity + offset, Quantity(150));
        // Subtraction
        assert_eq!(base_quantity - offset, Quantity(50));
        // Comparison
        assert!(base_quantity > offset);
    }

    #[test]
    fn test_function_code_string() {
        let cases = [
            (FUNC_READ_COILS, "ReadCoils"),
            (FUNC_READ_DISCRETE_INPUTS, "ReadDiscreteInputs"),
            (FUNC_READ_HOLDING_REGISTERS, "ReadHoldingRegisters"),
            (FUNC_READ_INPUT_REGISTERS, "ReadInputRegisters"),
            (FUNC_WRITE_SINGLE_COIL, "WriteSingleCoil"),
            (FUNC_WRITE_SINGLE_REGISTER, "WriteSingleRegister"),
            (FUNC_READ_EXCEPTION_STATUS, "ReadExceptionStatus"),
            (FUNC_WRITE_MULTIPLE_COILS, "WriteMultipleCoils"),
            (FUNC_WRITE_MULTIPLE_REGISTERS, "WriteMultipleRegisters"),
            (FUNC_READ_WRITE_MULTIPLE_REGISTERS, "ReadWriteMultipleRegisters"),
            (FunctionCode(0x7F), "Unknown(0x7F)"), // Unknown function code (not an exception)
        ];
        for (code, expected) in cases {
            assert_eq!(code.to_string(), expected, "FunctionCode for 0x{:02X}", code.0);
        }
        // Exception response rendering
        assert_eq!(FunctionCode(0x81).to_string(), "Exception(ReadCoils)");
        assert_eq!(FUNC_READ_DEVICE_IDENTIFICATION.to_string(), "ReadDeviceIdentification");
    }

    #[test]
    fn test_exception_code_string() {
        let cases = [
            (EXCEPTION_FUNCTION_CODE_NOT_SUPPORTED, "FunctionCodeNotSupported"),
            (EXCEPTION_DATA_ADDRESS_NOT_AVAILABLE, "DataAddressNotAvailable"),
            (EXCEPTION_INVALID_DATA_VALUE, "InvalidDataValue"),
            (EXCEPTION_SERVER_DEVICE_FAILURE, "ServerDeviceFailure"),
            (EXCEPTION_ACKNOWLEDGE, "Acknowledge"),
            (EXCEPTION_SERVER_DEVICE_BUSY, "ServerDeviceBusy"),
            (EXCEPTION_MEMORY_PARITY_ERROR, "MemoryParityError"),
            (EXCEPTION_GATEWAY_PATH_UNAVAILABLE, "GatewayPathUnavailable"),
            (EXCEPTION_GATEWAY_TARGET_NO_RESPONSE, "GatewayTargetNoResponse"),
            (ExceptionCode(0xFF), "Unknown(0xFF)"), // Unknown exception code
        ];
        for (code, expected) in cases {
            assert_eq!(code.to_string(), expected, "ExceptionCode for 0x{:02X}", code.0);
        }
    }

    #[test]
    fn test_exception_functions() {
        // Normal function code
        let normal_code = FUNC_READ_COILS.0;
        assert!(!is_exception(normal_code));

        // Exception function code
        let exception_code = FUNC_READ_COILS.0 | EXCEPTION_BIT;
        assert!(is_exception(exception_code));

        assert_eq!(get_original_function_code(exception_code), normal_code);

        // IsFunctionException and GetOriginalFunction
        let normal_func_code = FUNC_READ_COILS;
        let exception_func_code = FunctionCode(exception_code);
        assert!(!is_function_exception(normal_func_code));
        assert!(is_function_exception(exception_func_code));
        assert_eq!(get_original_function(exception_func_code), normal_func_code);
    }

    #[test]
    fn test_misc_strings() {
        assert_eq!(CONFORMITY_LEVEL_BASIC.to_string(), "Basic (stream)");
        assert_eq!(CONFORMITY_LEVEL_EXTENDED_INDIVIDUAL.to_string(), "Extended (stream+individual)");
        assert_eq!(ConformityLevel(0x55).to_string(), "Unknown(0x55)");
        assert_eq!(MORE_FOLLOWS_NO.to_string(), "No");
        assert_eq!(MORE_FOLLOWS_YES.to_string(), "Yes");
        assert_eq!(MEI_READ_DEVICE_ID.to_string(), "ReadDeviceIdentification");
        assert_eq!(MeiType(1).to_string(), "UnknownMEIType(0x01)");
        assert_eq!(READ_DEVICE_ID_SPECIFIC.to_string(), "SpecificObject");
        assert_eq!(DeviceIdObjectCode(0x90).to_string(), "ExtendedObject(0x90)");
        assert_eq!(DeviceIdObjectCode(0x10).to_string(), "UnknownObject(0x10)");
        assert_eq!(ExceptionStatus(0).to_string(), "ExceptionStatus(None)");
        assert_eq!(ExceptionStatus(0x0A).to_string(), "ExceptionStatus(Bits: [1 3], Value: 0x0A)");
    }
}
