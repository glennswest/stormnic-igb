//! `decode` (memory decode and bus mastering) against a fake PCI function
//! whose attributes drive the command register the way EDK2's PciIo does.
//! The point (#7, finding ec68cd8a67bf): when the NIC would not quiesce,
//! cleanup must leave bus mastering off even if the function had it on
//! before Start.
#[path = "../src/decode.rs"]
#[allow(dead_code)]
mod decode;
use core::cell::Cell;
use decode::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Refused;

/// A function: attributes mirror into the command register's MSE/BME.
struct Fake {
    attributes: Cell<u64>,
    command: Cell<u16>,
    /// `Enable` answers UNSUPPORTED (AMI Aptio 4).
    enable_fails: bool,
    /// `Set` turns on every bit it was given *and* BUS_MASTER (a firmware
    /// that does not honour a cleared bit).
    set_forces_bus_master: bool,
    /// `Disable` answers UNSUPPORTED.
    disable_fails: bool,
    /// Config writes are refused.
    config_write_fails: bool,
}

/// The command register bits an attribute mask stands for.
fn bits(a: u64) -> u16 {
    let mut c = 0;
    if a & ATTRIBUTE_MEMORY != 0 {
        c |= COMMAND_MEMORY;
    }
    if a & ATTRIBUTE_BUS_MASTER != 0 {
        c |= COMMAND_BUS_MASTER;
    }
    c
}

impl Fake {
    fn new(attributes: u64) -> Self {
        let f = Fake {
            attributes: Cell::new(0),
            command: Cell::new(0),
            enable_fails: false,
            set_forces_bus_master: false,
            disable_fails: false,
            config_write_fails: false,
        };
        f.put(attributes);
        f
    }
    /// `Set`: the attributes, and MSE/BME in the command register, become `a`.
    fn put(&self, a: u64) {
        self.attributes.set(a);
        self.command.set((self.command.get() & !(COMMAND_MEMORY | COMMAND_BUS_MASTER)) | bits(a));
    }
    fn bme(&self) -> bool {
        self.command.get() & COMMAND_BUS_MASTER != 0
    }
    fn mse(&self) -> bool {
        self.command.get() & COMMAND_MEMORY != 0
    }
}

impl Pci for Fake {
    type Status = Refused;
    fn get(&self) -> Result<u64, Refused> {
        Ok(self.attributes.get())
    }
    fn supported(&self) -> Result<u64, Refused> {
        Ok(ATTRIBUTE_MEMORY | ATTRIBUTE_BUS_MASTER)
    }
    fn enable(&self, a: u64) -> Result<(), Refused> {
        if self.enable_fails {
            return Err(Refused);
        }
        self.attributes.set(self.attributes.get() | a);
        self.command.set(self.command.get() | bits(a));
        Ok(())
    }
    fn disable(&self, a: u64) -> Result<(), Refused> {
        if self.disable_fails {
            return Err(Refused);
        }
        self.attributes.set(self.attributes.get() & !a);
        self.command.set(self.command.get() & !bits(a));
        Ok(())
    }
    fn set(&self, a: u64) -> Result<(), Refused> {
        self.put(if self.set_forces_bus_master { a | ATTRIBUTE_BUS_MASTER } else { a });
        Ok(())
    }
    fn config_read_u16(&self, offset: u32) -> Result<u16, Refused> {
        assert_eq!(offset, COMMAND);
        Ok(self.command.get())
    }
    fn config_write_u16(&self, offset: u32, value: u16) -> Result<(), Refused> {
        assert_eq!(offset, COMMAND);
        if self.config_write_fails {
            return Err(Refused);
        }
        self.command.set(value);
        Ok(())
    }
}

const BOTH: u64 = ATTRIBUTE_MEMORY | ATTRIBUTE_BUS_MASTER;

#[test]
fn enable_sets_both_and_release_puts_the_original_back() {
    let f = Fake::new(0);
    let e = enable(&f).unwrap();
    assert!(f.mse() && f.bme());
    release(&f, &e).unwrap();
    assert!(!f.mse() && !f.bme());
    assert_eq!(f.attributes.get(), 0);
}

#[test]
fn release_restores_a_bus_master_the_function_had_before() {
    // The quiesced path: putting back what the firmware had is right.
    let f = Fake::new(BOTH);
    let e = enable(&f).unwrap();
    release(&f, &e).unwrap();
    assert!(f.bme(), "release puts the original BUS_MASTER back");
}

#[test]
fn abandon_leaves_bus_master_off_when_the_original_had_it_on() {
    // ec68cd8a67bf: the NIC would not quiesce, the original attributes had
    // BUS_MASTER; the old cleanup (stop BME, then release) turned it back on.
    let f = Fake::new(BOTH);
    let e = enable(&f).unwrap();
    assert!(abandon(&f, &e).is_ok());
    assert!(!f.bme(), "bus mastering must stay off");
    assert!(f.mse(), "memory decode goes back to the original");
    assert_eq!(f.attributes.get(), ATTRIBUTE_MEMORY);
}

#[test]
fn abandon_after_stop_bus_master_keeps_it_off() {
    // The order binding's keep_dma used to run, but with abandon.
    let f = Fake::new(BOTH);
    let e = enable(&f).unwrap();
    stop_bus_master(&f).unwrap();
    assert!(!f.bme());
    abandon(&f, &e).unwrap();
    assert!(!f.bme());
}

#[test]
fn abandon_clears_bus_master_set_by_a_firmware_that_ignores_the_mask() {
    let mut f = Fake::new(BOTH);
    f.set_forces_bus_master = true;
    let e = enable(&f).unwrap();
    f.disable_fails = true;
    assert!(abandon(&f, &e).is_ok());
    assert!(!f.bme(), "the final config write clears what Set put back");
}

#[test]
fn abandon_after_a_config_write_enable_keeps_bus_master_off() {
    // Aptio 4: Enable refused, MSE/BME set by config write.
    let mut f = Fake::new(0);
    f.command.set(COMMAND_MEMORY | 0x0400);
    f.enable_fails = true;
    let e = enable(&f).unwrap();
    assert_eq!(e.by_attributes, 0);
    assert_eq!(e.command_before, Some(COMMAND_MEMORY | 0x0400));
    abandon(&f, &e).unwrap();
    assert!(!f.bme());
    assert_eq!(f.command.get(), COMMAND_MEMORY | 0x0400, "MSE and other bits as before");
}

#[test]
fn abandon_reports_when_bus_master_cannot_be_cleared() {
    let mut f = Fake::new(BOTH);
    f.set_forces_bus_master = true;
    let e = enable(&f).unwrap();
    f.disable_fails = true;
    f.config_write_fails = true;
    assert_eq!(abandon(&f, &e), Err(Some(Refused)));
}
