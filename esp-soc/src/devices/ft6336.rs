use esp_periph::i2c::I2cDevice;

/// Touch state shared between a board (UI) and its touch controller.
#[derive(Default, Debug, Clone, Copy)]
pub struct TouchState { pub down: bool, pub x: u16, pub y: u16, pub seen: bool, pub release_pending: bool }
impl TouchState {
    pub fn update(&mut self, x: u16, y: u16, down: bool) {
        self.x = x; self.y = y;
        if down { self.down = true; self.seen = false; self.release_pending = false; }
        else if self.seen { self.down = false; }
        else { self.release_pending = true; }
    }
    /// Keep even a short tap readable until the guest has observed the press once.
    pub fn observe(&mut self) {
        if self.release_pending && self.seen { self.down = false; self.release_pending = false; }
        if self.down { self.seen = true; }
    }
}

/// FocalTech FT6336U capacitive touch controller (I2C @ 0x38) used on M5Stack Core2 / CoreS3.
pub struct Ft6336 {
    ptr: u8,
    first: bool,
    touch: std::sync::Arc<std::sync::Mutex<TouchState>>,
    pub reads: u64,
}

impl Ft6336 {
    pub fn new(touch: std::sync::Arc<std::sync::Mutex<TouchState>>) -> Self {
        Ft6336 { ptr: 0, first: true, touch, reads: 0 }
    }

    fn reg(&self, addr: u8) -> u8 {
        let mut touch = self.touch.lock().expect("FT6336 touch state mutex poisoned");
        if addr == 0x02 {
            if touch.release_pending && touch.seen {
                touch.down = false;
                touch.release_pending = false;
            }
            if touch.down {
                touch.seen = true;
            }
        }
        let touch = *touch;
        match addr {
            0x00 => 0, // DEV_MODE: normal
            0x01 => 0, // GEST_ID: no gesture
            0x02 => u8::from(touch.down), // TD_STATUS: 1 touch point
            0x03 => {
                let event = if touch.down { 0x00 } else { 0x40 }; // 00b: press down, 01b: lift up
                event | (((touch.x >> 8) as u8) & 0x0f)
            }
            0x04 => touch.x as u8,
            0x05 => ((touch.y >> 8) as u8) & 0x0f,
            0x06 => touch.y as u8,
            0xa3 => 0x64, // Chip ID
            0xa6 => 0x01, // Firmware version
            0xa8 => 0x11, // FocalTech ID
            _ => 0,
        }
    }
}

impl I2cDevice for Ft6336 {
    fn start(&mut self, read: bool) -> bool { if !read { self.first = true; } true }
    fn write(&mut self, byte: u8) -> bool {
        if self.first { self.ptr = byte; self.first = false; } else { self.ptr = self.ptr.wrapping_add(1); }
        true
    }
    fn read(&mut self) -> u8 {
        let value = self.reg(self.ptr);
        self.ptr = self.ptr.wrapping_add(1);
        self.reads += 1;
        value
    }
}

