//! Single-controller, 7-bit GPIO I2C: NXP UM10204 rev.7, sections 3.1.3–3.1.6.
//! No clock stretching or arbitration; targets share their state with hardware I2C.
use esp_periph::i2c::I2cDevice;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
pub struct SharedI2cDevice(Rc<RefCell<Box<dyn I2cDevice>>>);

impl SharedI2cDevice {
    pub fn new(device: Box<dyn I2cDevice>) -> Self {
        Self(Rc::new(RefCell::new(device)))
    }
}

impl I2cDevice for SharedI2cDevice {
    fn address(&self, configured: u8) -> u8 {
        self.0.borrow().address(configured)
    }
    fn pins(&self) -> Option<(u8, u8)> {
        self.0.borrow().pins()
    }
    fn start_address(&mut self, configured: u8, address: u8, read: bool) -> bool {
        self.0.borrow_mut().start_address(configured, address, read)
    }
    fn start(&mut self, read: bool) -> bool {
        self.0.borrow_mut().start(read)
    }
    fn write(&mut self, byte: u8) -> bool {
        self.0.borrow_mut().write(byte)
    }
    fn read(&mut self) -> u8 {
        self.0.borrow_mut().read()
    }
    fn stop(&mut self) {
        self.0.borrow_mut().stop();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Address,
    Write,
    Ack,
    Read,
    MasterAck,
    Wait,
}

pub struct I2cBitBang {
    devices: Vec<(u8, SharedI2cDevice)>,
    selected: Option<usize>,
    phase: Phase,
    after_ack: Phase,
    clock: bool,
    data: bool,
    low: bool,
    byte: u8,
    bits: u8,
    ack: bool,
    ack_seen: bool,
}

impl Default for I2cBitBang {
    fn default() -> Self {
        Self::new()
    }
}

impl I2cBitBang {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            selected: None,
            phase: Phase::Idle,
            after_ack: Phase::Idle,
            clock: true,
            data: true,
            low: false,
            byte: 0,
            bits: 0,
            ack: false,
            ack_seen: false,
        }
    }

    pub fn attach(&mut self, address: u8, device: SharedI2cDevice) {
        self.devices.push((address, device));
    }

    fn send_byte(&mut self) {
        self.byte = self.selected.map_or(0xff, |i| self.devices[i].1.read());
        self.bits = 0;
        self.phase = Phase::Read;
        self.low = self.byte & 0x80 == 0;
    }

    /// Master levels after output-enable/open-drain resolution. A response is either
    /// pull SDA low (true) or release it (false), never actively drive it high.
    pub fn update(&mut self, clock: bool, master_data: bool) -> Option<bool> {
        let old_low = self.low;
        let data = master_data && !self.low;
        if clock && self.clock && data != self.data {
            self.low = false;
            self.selected = None;
            self.bits = 0;
            self.byte = 0;
            if data {
                self.phase = Phase::Idle;
                for (_, device) in &mut self.devices {
                    device.stop();
                }
            } else {
                self.phase = Phase::Address;
            }
        } else if clock && !self.clock {
            match self.phase {
                Phase::Address | Phase::Write => {
                    self.byte = (self.byte << 1) | u8::from(data);
                    self.bits += 1;
                    if self.bits == 8 {
                        let read = self.phase == Phase::Address && self.byte & 1 != 0;
                        self.ack = if self.phase == Phase::Address {
                            self.selected =
                                self.devices.iter_mut().position(|(address, device)| {
                                    device.start_address(*address, self.byte >> 1, read)
                                });
                            self.selected.is_some()
                        } else {
                            self.selected
                                .is_some_and(|i| self.devices[i].1.write(self.byte))
                        };
                        self.after_ack = if !self.ack {
                            Phase::Wait
                        } else if read {
                            Phase::Read
                        } else {
                            Phase::Write
                        };
                        self.phase = Phase::Ack;
                        self.ack_seen = false;
                    }
                }
                Phase::Ack => self.ack_seen = true,
                Phase::Read => self.bits += 1,
                Phase::MasterAck => {
                    self.ack = !data;
                    self.ack_seen = true;
                }
                _ => {}
            }
        } else if !clock && self.clock {
            match self.phase {
                Phase::Ack if !self.ack_seen => self.low = self.ack,
                Phase::Ack => {
                    self.low = false;
                    self.phase = self.after_ack;
                    self.byte = 0;
                    self.bits = 0;
                    if self.phase == Phase::Read {
                        self.send_byte();
                    }
                }
                Phase::Read if self.bits < 8 => self.low = self.byte & (0x80 >> self.bits) == 0,
                Phase::Read => {
                    self.low = false;
                    self.phase = Phase::MasterAck;
                    self.ack_seen = false;
                }
                Phase::MasterAck if self.ack_seen => {
                    if self.ack {
                        self.send_byte();
                    } else {
                        self.phase = Phase::Wait;
                    }
                }
                _ => {}
            }
        }
        self.clock = clock;
        self.data = master_data && !self.low;
        (self.low != old_low).then_some(self.low)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Registers {
        data: [u8; 256],
        pointer: u8,
        first: bool,
    }
    impl I2cDevice for Registers {
        fn start(&mut self, read: bool) -> bool {
            if !read {
                self.first = true;
            }
            true
        }
        fn write(&mut self, byte: u8) -> bool {
            if self.first {
                self.pointer = byte;
                self.first = false;
            } else {
                self.data[self.pointer as usize] = byte;
                self.pointer = self.pointer.wrapping_add(1);
            }
            true
        }
        fn read(&mut self) -> u8 {
            let value = self.data[self.pointer as usize];
            self.pointer = self.pointer.wrapping_add(1);
            value
        }
    }

    fn line(bus: &mut I2cBitBang, clock: bool, data: bool) -> bool {
        bus.update(clock, data);
        data && !bus.low
    }
    fn start(bus: &mut I2cBitBang) {
        line(bus, false, true);
        line(bus, true, true);
        line(bus, true, false);
    }
    fn stop(bus: &mut I2cBitBang) {
        line(bus, false, false);
        line(bus, true, false);
        line(bus, true, true);
    }
    fn write(bus: &mut I2cBitBang, byte: u8) -> bool {
        for bit in (0..8).rev() {
            let high = byte & (1 << bit) != 0;
            line(bus, false, high);
            line(bus, true, high);
        }
        line(bus, false, true);
        let ack = !line(bus, true, true);
        line(bus, false, true);
        ack
    }
    fn read(bus: &mut I2cBitBang, ack: bool) -> u8 {
        let mut value = 0;
        for _ in 0..8 {
            value = (value << 1) | u8::from(line(bus, true, true));
            line(bus, false, true);
        }
        line(bus, false, !ack);
        line(bus, true, !ack);
        line(bus, false, !ack);
        value
    }

    #[test]
    fn repeated_start_read_and_shared_hardware_state() {
        let mut device = SharedI2cDevice::new(Box::new(Registers {
            data: [0; 256],
            pointer: 0,
            first: true,
        }));
        let mut bus = I2cBitBang::new();
        bus.attach(0x34, device.clone());
        start(&mut bus);
        assert!(write(&mut bus, 0x68));
        assert!(write(&mut bus, 7));
        assert!(write(&mut bus, 0xa5));
        assert!(write(&mut bus, 0x3c));
        stop(&mut bus);
        assert!(device.start(false));
        assert!(device.write(7));
        assert!(device.start(true));
        assert_eq!(device.read(), 0xa5);
        start(&mut bus);
        assert!(write(&mut bus, 0x68));
        assert!(write(&mut bus, 7));
        start(&mut bus);
        assert!(write(&mut bus, 0x69));
        assert_eq!(read(&mut bus, true), 0xa5);
        assert_eq!(read(&mut bus, false), 0x3c);
        assert!(!bus.low);
        stop(&mut bus);
        assert_eq!(bus.phase, Phase::Idle);
    }

    #[test]
    fn missing_address_nacks_and_stop_aborts_partial_byte() {
        let mut bus = I2cBitBang::new();
        start(&mut bus);
        assert!(!write(&mut bus, 0x68));
        assert!(!bus.low);
        stop(&mut bus);
        start(&mut bus);
        line(&mut bus, false, false);
        line(&mut bus, true, false);
        stop(&mut bus);
        assert!(!bus.low);
        assert_eq!(bus.phase, Phase::Idle);
        start(&mut bus);
        assert!(!write(&mut bus, 0x69));
    }
}
