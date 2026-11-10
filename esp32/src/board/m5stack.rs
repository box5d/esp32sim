//! M5Stack Basic / Fire and Core2 (AXP192 revision) board wiring.
//! Sources: docs.m5stack.com/en/core/{basic,Fire,Core2}; M5GFX 0.2.35 Core2 reset/backlight registers.
use esp_periph::i2c::I2cDevice;
use esp_soc::board::{BoardEdge, BoardModel, SpiPins, VirtualCycle};
use esp_soc::devices::{DcsPanel, ft6336::{Ft6336, TouchState}, i2c_bitbang::{I2cBitBang, SharedI2cDevice}};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Model { Basic, Fire, Core2 }

// Register-level AXP192 power control; battery/charging analog behavior is not modeled.
struct Axp192 {
    regs: Arc<Mutex<[u8; 256]>>, reset: Arc<AtomicBool>, ptr: u8, first: bool,
}
impl I2cDevice for Axp192 {
    fn pins(&self) -> Option<(u8, u8)> { Some((21, 22)) }
    fn start(&mut self, read: bool) -> bool { if !read { self.first = true; } true }
    fn write(&mut self, byte: u8) -> bool {
        if self.first { self.ptr = byte; self.first = false; }
        else {
            let mut regs = self.regs.lock().unwrap();
            if self.ptr != 3 { regs[self.ptr as usize] = byte; }
            if self.ptr == 0x96 && byte & 2 == 0 { self.reset.store(true, Ordering::Relaxed); }
            self.ptr = self.ptr.wrapping_add(1);
        }
        true
    }
    fn read(&mut self) -> u8 {
        let byte = self.regs.lock().unwrap()[self.ptr as usize];
        self.ptr = self.ptr.wrapping_add(1); byte
    }
}

struct BoardI2c(SharedI2cDevice);
impl I2cDevice for BoardI2c {
    fn pins(&self) -> Option<(u8, u8)> { Some((21, 22)) }
    fn start(&mut self, read: bool) -> bool { self.0.start(read) }
    fn write(&mut self, byte: u8) -> bool { self.0.write(byte) }
    fn read(&mut self) -> u8 { self.0.read() }
    fn stop(&mut self) { self.0.stop(); }
}

pub struct M5Stack {
    model: Model, panel: DcsPanel, cs: bool, reset: bool, backlight: bool, command: u8, read_index: u8,
    pmu: Arc<Mutex<[u8; 256]>>, pmu_reset: Arc<AtomicBool>,
    touch: Arc<Mutex<TouchState>>, software_i2c: I2cBitBang,
    edges: Vec<BoardEdge>, release_sda: bool, cycle: VirtualCycle, touch_irq_high: bool,
}
impl M5Stack {
    pub fn new(model: Model) -> Self {
        let mut regs = [0; 256]; regs[3] = 3;
        Self { model, panel: DcsPanel::new(320, 240), cs: true, reset: true, backlight: false, command: 0, read_index: 0,
            pmu: Arc::new(Mutex::new(regs)), pmu_reset: Arc::new(AtomicBool::new(false)),
            touch: Arc::new(Mutex::new(TouchState::default())), software_i2c: I2cBitBang::new(),
            edges: Vec::new(), release_sda: false, cycle: 0, touch_irq_high: true }
    }
    fn core2(&self) -> bool { self.model == Model::Core2 }
    fn powered(&self) -> bool {
        if self.core2() { let regs = self.pmu.lock().unwrap(); regs[0x12] & 4 != 0 && regs[0x96] & 2 != 0 && regs[0x27] & 0x7f != 0 }
        else { self.reset && self.backlight }
    }
}
impl BoardModel for M5Stack {
    fn name(&self) -> &'static str { match self.model { Model::Basic => "m5basic", Model::Fire => "m5fire", Model::Core2 => "m5core2" } }
    fn gpio_changes(&mut self, changes: &[(u8, bool)]) {
        let (cs, dc) = if self.core2() { (5, 15) } else { (14, 27) };
        for &(pin, level) in changes {
            if pin == cs { self.cs = level; }
            if pin == dc { self.panel.dc = level; }
            if !self.core2() {
                if pin == 33 { self.reset = level; if !level { self.panel.reset(); } }
                if pin == 32 { self.backlight = level; }
            }
        }
    }
    fn gpio_output_at(&mut self, cycle: VirtualCycle, changes: &[(u8, bool)], enabled: u64, output: u64) {
        self.cycle = cycle; self.gpio_changes(changes);
        if self.core2() {
            let released = |pin: u8| enabled & !output & (1u64 << pin) == 0u64;
            if let Some(low) = self.software_i2c.update(released(22), released(21)) {
                self.release_sda = !low;
                if low { self.edges.push(BoardEdge { cycle, pin: 21, level: false }); }
            }
        }
    }
    fn released_inputs(&mut self) -> Vec<u8> { if std::mem::take(&mut self.release_sda) { vec![21] } else { Vec::new() } }
    fn take_edges(&mut self) -> Vec<BoardEdge> { std::mem::take(&mut self.edges) }
    fn uses_spi_pins(&self) -> bool { true }
    fn spi_transfer_pins(&mut self, _host: u8, pins: SpiPins, tx: &[u8], rx_len: usize) -> Vec<u8> {
        let mut rx = vec![0xff; rx_len];
        if self.cs || pins.sclk & (1 << 18) == 0 || pins.mosi & (1 << 23) == 0 { return rx; }
        if self.pmu_reset.swap(false, Ordering::Relaxed) { self.panel.reset(); }
        for &byte in tx {
            if !self.panel.dc { self.command = byte; }
            else if self.command == 0xd9 { self.read_index = byte & 15; }
            self.panel.byte(byte);
        }
        // M5GFX 0.2.35 probe requires the ILI9342C register set as well as RDDID.
        let data: &[u8] = match self.command {
            0x04 => &[0xe3, 0, 0x93, 0x42],
            // C-variant ID4 bytes with the M5GFX one-bit dummy phase.
            0xd3 if self.read_index == 2 => &[0x26],
            0xd3 if self.read_index == 3 => &[0x84],
            0xd3 => &[0, 0x93, 0x42],
            0x09 => &[0, 0x61, 0, 0], 0x0a => &[0x08],
            _ => &[],
        };
        rx.fill(0); for (dst, src) in rx.iter_mut().zip(data) { *dst = *src; } rx
    }
    fn input_levels(&self) -> Vec<(u8, bool)> {
        // Only the touch IRQ is a genuine board-driven input. SDA/SCL must NOT be
        // driven here: a static external drive overrides the guest's own output in
        // the resolved input level, so a guest that verifies its clock/data lines
        // by reading them back (LGFX software I2C) would see SCL stuck high.
        // Undriven pins already resolve high via the passive pull default.
        if self.core2() { vec![(39, true)] } else { vec![(39, true), (38, true), (37, true)] }
    }
    fn named_pin(&self, name: &str) -> Option<u8> {
        if self.core2() { return None; }
        match name { "a" | "btn1" => Some(39), "b" | "btn2" => Some(38), "c" | "btn3" => Some(37), _ => None }
    }
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn I2cDevice>)> {
        if !self.core2() { return Vec::new(); }
        let devices: [(u8, Box<dyn I2cDevice>); 2] = [
            (0x34, Box::new(Axp192 { regs: self.pmu.clone(), reset: self.pmu_reset.clone(), ptr: 0, first: true })),
            (0x38, Box::new(Ft6336::new(self.touch.clone()))),
        ];
        self.software_i2c = I2cBitBang::new();
        let mut out = Vec::new();
        for (addr, device) in devices {
            let device = SharedI2cDevice::new(device);
            self.software_i2c.attach(addr, device.clone());
            // Both controllers can route to the board's GPIO21/22 I2C wires.
            for bus in 0..2 { out.push((bus, addr, Box::new(BoardI2c(device.clone())) as Box<dyn I2cDevice>)); }
        }
        out
    }
    fn touch(&mut self, x: u16, y: u16, down: bool) {
        if self.core2() {
            self.touch.lock().unwrap().update(x.min(319), y.min(279), down);
            self.advance_to(self.cycle);
        }
    }
    fn advance_to(&mut self, cycle: VirtualCycle) {
        self.cycle = cycle;
        if self.core2() {
            let high = !self.touch.lock().unwrap().down;
            if high != self.touch_irq_high {
                self.touch_irq_high = high;
                self.edges.push(BoardEdge { cycle, pin: 39, level: high });
            }
        }
    }
    fn display(&self) -> Option<(u32, u32, Vec<u16>, u64)> {
        let p = &self.panel;
        let mut pixels = p.gram.clone();
        if !self.powered() || !p.on || p.sleeping { pixels.fill(0); }
        else if !p.inverted { for pixel in &mut pixels { *pixel = !*pixel; } }
        Some((p.cols as u32, p.rows as u32, pixels, self.display_version()))
    }
    fn display_version(&self) -> u64 { self.panel.pixels_written.saturating_mul(2) + u64::from(self.powered()) }
    fn display_frames(&self) -> u64 { self.panel.frames }
    fn gram(&self) -> Option<(Vec<u16>, usize, usize)> { Some((self.panel.gram.clone(), self.panel.cols, self.panel.rows)) }
    fn report(&self) -> String { format!("[emu] {}: ILI9342C {} pixels, powered={}", self.name(), self.panel.pixels_written, self.powered()) }
}
