//! In-memory [`DataStore`]. Port of `gomodbus/server/memory_store.go`.
//!
//! Implements the four-table MODBUS data model.
//! Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (MODBUS Data model)

use std::collections::HashMap;
use std::sync::RwLock;

use crate::common::*;

// Implementation detail (not spec): the four tables guarded by one RwLock.
#[derive(Default)]
struct Tables {
    // Coils (read-write 1-bit outputs) - FC 0x01 (read) and 0x05/0x0F (write)
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (Coils, Table 1)
    coils: HashMap<Address, CoilValue>,
    // Discrete Inputs (read-only 1-bit inputs) - FC 0x02 (read)
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (Discrete Input, Table 1)
    discrete_inputs: HashMap<Address, DiscreteInputValue>,
    // Holding Registers (read-write 16-bit) - FC 0x03 (read) and 0x06/0x10 (write)
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (Holding Registers, Table 1)
    holding_registers: HashMap<Address, RegisterValue>,
    // Input Registers (read-only 16-bit) - FC 0x04 (read)
    // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (Input Registers, Table 1)
    input_registers: HashMap<Address, InputRegisterValue>,
}

/// MemoryStore implements [`DataStore`] with in-memory storage.
/// Provides storage for all four Modbus data types as defined in the specification.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 4.3 (MODBUS Data model)
///
/// Unset addresses read as `false` / `0`. Access is protected by a `RwLock`
/// (implementation detail, not spec), so the store is shareable via `Arc`.
#[derive(Default)]
pub struct MemoryStore {
    tables: RwLock<Tables>,
}

// Implementation detail (not spec): poisoned locks are recovered, as Go mutexes cannot poison.
fn read_lock(l: &RwLock<Tables>) -> std::sync::RwLockReadGuard<'_, Tables> {
    l.read().unwrap_or_else(|e| e.into_inner())
}
fn write_lock(l: &RwLock<Tables>) -> std::sync::RwLockWriteGuard<'_, Tables> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

impl MemoryStore {
    /// Creates a new memory-based data store (Go `NewMemoryStore`).
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    /// Gets a single coil value (`None` if never set).
    /// Ref: Section 4.3 (Coils)
    pub fn get_coil(&self, address: Address) -> Option<CoilValue> {
        read_lock(&self.tables).coils.get(&address).copied()
    }

    /// Sets a single coil value.
    /// Ref: Section 4.3 (Coils)
    pub fn set_coil(&self, address: Address, value: CoilValue) {
        write_lock(&self.tables).coils.insert(address, value);
    }

    /// Gets a single discrete input value (`None` if never set).
    /// Ref: Section 4.3 (Discrete Inputs)
    pub fn get_discrete_input(&self, address: Address) -> Option<DiscreteInputValue> {
        read_lock(&self.tables).discrete_inputs.get(&address).copied()
    }

    /// Sets a single discrete input value.
    /// Ref: Section 4.3 (Discrete Inputs)
    pub fn set_discrete_input(&self, address: Address, value: DiscreteInputValue) {
        write_lock(&self.tables).discrete_inputs.insert(address, value);
    }

    /// Gets a single holding register value (`None` if never set).
    /// Ref: Section 4.3 (Holding Registers)
    pub fn get_holding_register(&self, address: Address) -> Option<RegisterValue> {
        read_lock(&self.tables).holding_registers.get(&address).copied()
    }

    /// Sets a single holding register value.
    /// Ref: Section 4.3 (Holding Registers)
    pub fn set_holding_register(&self, address: Address, value: RegisterValue) {
        write_lock(&self.tables).holding_registers.insert(address, value);
    }

    /// Gets a single input register value (`None` if never set).
    /// Ref: Section 4.3 (Input Registers)
    pub fn get_input_register(&self, address: Address) -> Option<InputRegisterValue> {
        read_lock(&self.tables).input_registers.get(&address).copied()
    }

    /// Sets a single input register value.
    /// Ref: Section 4.3 (Input Registers)
    pub fn set_input_register(&self, address: Address, value: InputRegisterValue) {
        write_lock(&self.tables).input_registers.insert(address, value);
    }

    /// Returns a string representation of the memory store's content
    /// (Go `DumpRegisters`). Unlike Go (random map order) entries are sorted by address.
    /// Implementation detail, not spec.
    pub fn dump_registers(&self) -> String {
        let t = read_lock(&self.tables);
        let mut result = String::from("Memory Store Content:\n");

        if !t.coils.is_empty() {
            result.push_str("Coils:\n");
            for (addr, val) in sorted(&t.coils) {
                result.push_str(&format!("  {}: {}\n", addr.0, val));
            }
        }
        if !t.discrete_inputs.is_empty() {
            result.push_str("Discrete Inputs:\n");
            for (addr, val) in sorted(&t.discrete_inputs) {
                result.push_str(&format!("  {}: {}\n", addr.0, val));
            }
        }
        if !t.holding_registers.is_empty() {
            result.push_str("Holding Registers:\n");
            for (addr, val) in sorted(&t.holding_registers) {
                result.push_str(&format!("  {}: {} (0x{:04X})\n", addr.0, val, val));
            }
        }
        if !t.input_registers.is_empty() {
            result.push_str("Input Registers:\n");
            for (addr, val) in sorted(&t.input_registers) {
                result.push_str(&format!("  {}: {} (0x{:04X})\n", addr.0, val, val));
            }
        }
        result
    }
}

// Implementation detail (not spec): entries ordered by address for deterministic dumps.
fn sorted<V: Copy>(m: &HashMap<Address, V>) -> Vec<(Address, V)> {
    let mut v: Vec<(Address, V)> = m.iter().map(|(a, v)| (*a, *v)).collect();
    v.sort_by_key(|(a, _)| *a);
    v
}

// Ensures `address + count` stays within the 16-bit address space (0x0000-0xFFFF).
// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 7 (Exception code 02: ILLEGAL DATA ADDRESS -
// starting address and quantity together must not exceed the addressable range)
fn check_range(address: Address, count: usize) -> Result<()> {
    if address.0 as usize + count > 0x1_0000 {
        return Err(Error::InvalidAddress);
    }
    Ok(())
}

// Reads `quantity` consecutive entries starting at `address`; the range must not wrap
// the address space. Missing entries read as the default value (false / 0).
fn read_range<V: Copy + Default>(m: &HashMap<Address, V>, address: Address, quantity: Quantity) -> Result<Vec<V>> {
    check_range(address, quantity.0 as usize)?;
    Ok((0..quantity.0)
        .map(|i| m.get(&(address + Address(i))).copied().unwrap_or_default())
        .collect())
}

impl DataStore for MemoryStore {
    /// Reads coil values (data access for FC 0x01).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Read Coils)
    fn read_coils(&self, _ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<CoilValue>> {
        // Validate quantity within Modbus limits (1-2000 coils)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.1 (Quantity of Coils)
        if quantity.0 == 0 || quantity.0 > MAX_COIL_COUNT {
            return Err(Error::InvalidQuantity);
        }
        read_range(&read_lock(&self.tables).coils, address, quantity)
    }

    /// Reads discrete input values (data access for FC 0x02).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (Read Discrete Inputs)
    fn read_discrete_inputs(&self, _ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<DiscreteInputValue>> {
        // Validate quantity within Modbus limits (1-2000 inputs)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.2 (Quantity of Inputs)
        if quantity.0 == 0 || quantity.0 > MAX_COIL_COUNT {
            return Err(Error::InvalidQuantity);
        }
        read_range(&read_lock(&self.tables).discrete_inputs, address, quantity)
    }

    /// Reads holding register values (data access for FC 0x03).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Read Holding Registers)
    fn read_holding_registers(&self, _ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<RegisterValue>> {
        // Validate quantity within Modbus limits (1-125 registers)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.3 (Quantity of Registers)
        if quantity.0 == 0 || quantity.0 > MAX_REGISTER_COUNT {
            return Err(Error::InvalidQuantity);
        }
        read_range(&read_lock(&self.tables).holding_registers, address, quantity)
    }

    /// Reads input register values (data access for FC 0x04).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Read Input Registers)
    fn read_input_registers(&self, _ctx: &Context, address: Address, quantity: Quantity) -> Result<Vec<InputRegisterValue>> {
        // Validate quantity within Modbus limits (1-125 registers)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.4 (Quantity of Input Registers)
        if quantity.0 == 0 || quantity.0 > MAX_REGISTER_COUNT {
            return Err(Error::InvalidQuantity);
        }
        read_range(&read_lock(&self.tables).input_registers, address, quantity)
    }

    /// Writes a single coil (data access for FC 0x05).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.5 (Write Single Coil)
    fn write_single_coil(&self, _ctx: &Context, address: Address, value: CoilValue) -> Result<()> {
        write_lock(&self.tables).coils.insert(address, value);
        Ok(())
    }

    /// Writes a single register (data access for FC 0x06).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.6 (Write Single Register)
    fn write_single_register(&self, _ctx: &Context, address: Address, value: RegisterValue) -> Result<()> {
        write_lock(&self.tables).holding_registers.insert(address, value);
        Ok(())
    }

    /// Writes multiple coils (data access for FC 0x0F).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (Write Multiple Coils)
    fn write_multiple_coils(&self, _ctx: &Context, address: Address, values: &[CoilValue]) -> Result<()> {
        // Validate quantity within Modbus write limits (1-1968 coils)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.11 (Quantity of Outputs)
        if values.is_empty() || values.len() > MAX_WRITE_COIL_COUNT as usize {
            return Err(Error::InvalidQuantity);
        }
        check_range(address, values.len())?;
        let mut t = write_lock(&self.tables);
        for (i, value) in values.iter().enumerate() {
            t.coils.insert(address + Address(i as u16), *value);
        }
        Ok(())
    }

    /// Writes multiple registers (data access for FC 0x10).
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (Write Multiple Registers)
    fn write_multiple_registers(&self, _ctx: &Context, address: Address, values: &[RegisterValue]) -> Result<()> {
        // Validate quantity within Modbus limits (1-123 registers)
        // Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.12 (Quantity of Registers)
        if values.is_empty() || values.len() > MAX_WRITE_REGISTER_COUNT as usize {
            return Err(Error::InvalidQuantity);
        }
        check_range(address, values.len())?;
        let mut t = write_lock(&self.tables);
        for (i, value) in values.iter().enumerate() {
            t.holding_registers.insert(address + Address(i as u16), *value);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_range_does_not_wrap() {
        let store = MemoryStore::new();
        let ctx = Context::background();
        assert_eq!(store.read_holding_registers(&ctx, Address(0xFFFC), Quantity(10)), Err(Error::InvalidAddress));
        assert!(store.read_holding_registers(&ctx, Address(0xFFFC), Quantity(4)).is_ok());
        assert_eq!(store.write_multiple_registers(&ctx, Address(0xFFFF), &[1, 2]), Err(Error::InvalidAddress));
        assert_eq!(store.write_multiple_coils(&ctx, Address(0xFFFF), &[true, true]), Err(Error::InvalidAddress));
    }

    fn ctx() -> Context {
        Context::background()
    }

    #[test]
    fn read_coils() {
        let store = MemoryStore::new();
        store.set_coil(Address(100), true);
        store.set_coil(Address(101), false);
        store.set_coil(Address(102), true);

        let values = store.read_coils(&ctx(), Address(100), Quantity(3)).unwrap();
        assert_eq!(values, vec![true, false, true]);

        // unset values default to false
        let values = store.read_coils(&ctx(), Address(200), Quantity(2)).unwrap();
        assert_eq!(values, vec![false, false]);

        assert!(store.read_coils(&ctx(), Address(100), Quantity(0)).is_err());
        assert!(store.read_coils(&ctx(), Address(100), Quantity(MAX_COIL_COUNT + 1)).is_err());
    }

    #[test]
    fn read_discrete_inputs() {
        let store = MemoryStore::new();
        store.set_discrete_input(Address(100), true);
        store.set_discrete_input(Address(101), true);
        store.set_discrete_input(Address(102), false);

        let values = store.read_discrete_inputs(&ctx(), Address(100), Quantity(3)).unwrap();
        assert_eq!(values, vec![true, true, false]);
    }

    #[test]
    fn read_holding_registers() {
        let store = MemoryStore::new();
        store.set_holding_register(Address(100), 0x1234);
        store.set_holding_register(Address(101), 0x5678);

        let values = store.read_holding_registers(&ctx(), Address(100), Quantity(2)).unwrap();
        assert_eq!(values, vec![0x1234, 0x5678]);

        let values = store.read_holding_registers(&ctx(), Address(200), Quantity(2)).unwrap();
        assert_eq!(values, vec![0, 0]);

        assert!(store.read_holding_registers(&ctx(), Address(100), Quantity(0)).is_err());
        assert!(store.read_holding_registers(&ctx(), Address(100), Quantity(MAX_REGISTER_COUNT + 1)).is_err());
    }

    #[test]
    fn read_input_registers_and_errors() {
        let store = MemoryStore::new();
        store.set_input_register(Address(5), 0xBEEF);
        assert_eq!(store.read_input_registers(&ctx(), Address(5), Quantity(1)).unwrap(), vec![0xBEEF]);
        assert_eq!(
            store.read_input_registers(&ctx(), Address(5), Quantity(0)).unwrap_err(),
            Error::InvalidQuantity
        );
    }

    #[test]
    fn write_single_coil() {
        let store = MemoryStore::new();
        store.write_single_coil(&ctx(), Address(100), true).unwrap();
        assert_eq!(store.get_coil(Address(100)), Some(true));

        store.write_single_coil(&ctx(), Address(100), false).unwrap();
        assert_eq!(store.get_coil(Address(100)), Some(false));
    }

    #[test]
    fn write_multiple_coils() {
        let store = MemoryStore::new();
        let values = [true, false, true];
        store.write_multiple_coils(&ctx(), Address(100), &values).unwrap();
        for (i, expected) in values.iter().enumerate() {
            assert_eq!(store.get_coil(Address(100) + Address(i as u16)), Some(*expected));
        }

        assert!(store.write_multiple_coils(&ctx(), Address(100), &[]).is_err());
        let too_many = vec![false; MAX_COIL_COUNT as usize + 1];
        assert!(store.write_multiple_coils(&ctx(), Address(100), &too_many).is_err());
    }

    #[test]
    fn write_multiple_registers() {
        let store = MemoryStore::new();
        let values = [0x1234, 0x5678];
        store.write_multiple_registers(&ctx(), Address(100), &values).unwrap();
        for (i, expected) in values.iter().enumerate() {
            assert_eq!(store.get_holding_register(Address(100) + Address(i as u16)), Some(*expected));
        }
        assert!(store.write_multiple_registers(&ctx(), Address(0), &[]).is_err());
        assert!(store.write_multiple_registers(&ctx(), Address(0), &[0u16; 124]).is_err());
    }

    #[test]
    fn dump_registers() {
        let store = MemoryStore::new();
        store.set_coil(Address(100), true);
        store.set_discrete_input(Address(200), true);
        store.set_holding_register(Address(300), 0x1234);
        store.set_input_register(Address(400), 0x5678);

        let dump = store.dump_registers();
        assert!(!dump.is_empty());
        for expected in [
            "Coils:",
            "100: true",
            "Discrete Inputs:",
            "200: true",
            "Holding Registers:",
            "300: 4660 (0x1234)",
            "Input Registers:",
            "400: 22136 (0x5678)",
        ] {
            assert!(dump.contains(expected), "dump missing '{}': {}", expected, dump);
        }
    }
}
