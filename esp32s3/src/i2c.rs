//! The I2C bus devices the boards hang on the controller (`esp_periph::i2c::I2c`).
pub use esp_periph::i2c::*;
use std::collections::HashMap;

pub struct Ch32v003 { pub regs: [u8; 8], ptr: u8, first: bool, pub writes: u64 }
impl Default for Ch32v003 { fn default() -> Self { Self::new() } }

impl Ch32v003 {
    pub fn new() -> Self { let mut r = [0u8; 8]; r[2] = 0xff; r[4] = 0xff; Ch32v003 { regs: r, ptr: 0, first: true, writes: 0 } }
}
impl I2cDevice for Ch32v003 {
    fn start(&mut self, read: bool) -> bool { if !read { self.first = true; } true }
    fn write(&mut self, b: u8) -> bool { if self.first { self.ptr = b & 7; self.first = false; } else { self.regs[self.ptr as usize] = b; self.writes += 1; } true }
    fn read(&mut self) -> u8 { self.regs[self.ptr as usize] }
}

/// What the board needs to know about the sensor's configuration (written over SCCB).
#[derive(Default, Debug)]
pub struct SensorState { pub width: u32, pub height: u32, pub format: u8, pub streaming: bool }

/// OV5640 image sensor over SCCB: 16-bit register addresses, auto-increment.
pub struct Ov5640 { pub regs: HashMap<u16, u8>, addr: u16, phase: u8, pub writes: u64, state: std::sync::Arc<std::sync::Mutex<SensorState>> }
impl Ov5640 {
    pub fn new(state: std::sync::Arc<std::sync::Mutex<SensorState>>) -> Self {
        let mut regs = HashMap::new();
        regs.insert(0x300a, 0x56); regs.insert(0x300b, 0x40);   // chip ID 0x5640
        regs.insert(0x3008, 0x02);                              // system control: normal
        regs.insert(0x302a, 0xb0);                              // silicon revision
        Ov5640 { regs, addr: 0, phase: 0, writes: 0, state }
    }
    pub fn get(&self, r: u16) -> u8 { *self.regs.get(&r).unwrap_or(&0) }
    fn sync_state(&self) {
        let mut st = self.state.lock().unwrap();
        st.width = ((self.get(0x3808) as u32 & 0xf) << 8) | self.get(0x3809) as u32;    // DVP output width
        st.height = ((self.get(0x380a) as u32 & 0x7) << 8) | self.get(0x380b) as u32;   // DVP output height
        st.format = self.get(0x4300);
        st.streaming = self.get(0x3008) & 0x40 == 0;
    }
}
impl I2cDevice for Ov5640 {
    fn start(&mut self, read: bool) -> bool { if !read { self.phase = 0; } true }
    fn write(&mut self, b: u8) -> bool {
        match self.phase {
            0 => { self.addr = (b as u16) << 8; self.phase = 1; }
            1 => { self.addr |= b as u16; self.phase = 2; }
            _ => { let v = if self.addr == 0x3008 { b & !0x80 } else { b }; self.regs.insert(self.addr, v); if (0x3808..=0x380b).contains(&self.addr) || self.addr == 0x4300 || self.addr == 0x3008 { self.sync_state(); } self.addr = self.addr.wrapping_add(1); self.writes += 1; }
        }
        true
    }
    fn read(&mut self) -> u8 { let v = self.get(self.addr); self.addr = self.addr.wrapping_add(1); v }
}

/// State of an ST7701S panel controller as seen through its 9-bit init SPI (D/C bit + 8 data bits).
#[derive(Default, Debug)]
pub struct St7701State { pub words: u64, pub last_cmd: u8, pub sleep_out: bool, pub display_on: bool, pub cmds: Vec<u8> }

/// TCA9554 / PCA9554 8-bit IO expander (regs: 0 input, 1 output, 2 polarity, 3 config). On the
/// Waveshare Touch-LCD-4B the panel's init SPI hangs off EXIO0 (CS), EXIO1 (MOSI), EXIO2 (CLK); the
/// device decodes that bit-banged stream into `St7701State`.
pub struct Tca9554 { pub regs: [u8; 4], ptr: u8, first: bool, panel: Option<std::sync::Arc<std::sync::Mutex<St7701State>>>, shift: u16, nbits: u8 }
impl Tca9554 {
    pub fn new(panel: std::sync::Arc<std::sync::Mutex<St7701State>>) -> Self { Tca9554 { regs: [0xff, 0xff, 0x00, 0xff], ptr: 0, first: true, panel: Some(panel), shift: 0, nbits: 0 } }
    /// Disconnected register-RAM initialization stub. External output effects are not modeled.
    pub fn register_ram_stub() -> Self { Tca9554 { regs: [0xff, 0xff, 0x00, 0xff], ptr: 0, first: true, panel: None, shift: 0, nbits: 0 } }
    fn input_port(&self) -> u8 { ((self.regs[1] & !self.regs[3]) | self.regs[3]) ^ self.regs[2] }
    fn output(&mut self, old: u8, new: u8) {
        let Some(panel) = self.panel.as_ref() else { return };
        let cs = new & 1 != 0; let mosi = (new >> 1) & 1; let clk_rise = new & 4 != 0 && old & 4 == 0;
        if cs { self.nbits = 0; self.shift = 0; return; }
        if clk_rise {
            self.shift = (self.shift << 1) | mosi as u16; self.nbits += 1;
            if self.nbits == 9 {
                let dc = self.shift & 0x100 != 0; let b = self.shift as u8; self.nbits = 0; self.shift = 0;
                let mut st = panel.lock().expect("ST7701 panel state mutex poisoned"); st.words += 1;
                if !dc { st.last_cmd = b; st.cmds.push(b); match b { 0x11 => st.sleep_out = true, 0x10 => st.sleep_out = false, 0x29 => st.display_on = true, 0x28 => st.display_on = false, _ => {} } }
            }
        }
    }
}
impl I2cDevice for Tca9554 {
    fn start(&mut self, read: bool) -> bool { if !read { self.first = true; } true }
    fn write(&mut self, b: u8) -> bool {
        if self.first { self.ptr = b & 3; self.first = false; }
        else if self.ptr != 0 { let old = self.regs[1]; self.regs[self.ptr as usize] = b; if self.ptr == 1 { self.output(old, b); } }
        true
    }
    fn read(&mut self) -> u8 { if self.ptr == 0 { self.input_port() } else { self.regs[self.ptr as usize] } }
}

/// Touch state shared between the board (UI) and the GT911 model.
#[derive(Default, Debug, Clone, Copy)]
pub struct TouchState { pub down: bool, pub x: u16, pub y: u16, pub seen: bool, pub release_pending: bool }

/// Goodix GT911 capacitive touch controller: 16-bit register addresses; product ID at 0x8140,
/// config at 0x8047.., status + up to 5 points at 0x814E...
pub struct Gt911 { addr: u16, phase: u8, touch: std::sync::Arc<std::sync::Mutex<TouchState>>, pub reads: u64, w: u16, h: u16 }
impl Gt911 {
    pub fn new(touch: std::sync::Arc<std::sync::Mutex<TouchState>>, w: u16, h: u16) -> Self { Gt911 { addr: 0, phase: 0, touch, reads: 0, w, h } }
    fn reg(&self, a: u16) -> u8 {
        let mut tl = self.touch.lock().unwrap();
        if a == 0x814e {
            // like the real controller's buffer: a touch stays readable until the host has seen it once
            if tl.release_pending && tl.seen { tl.down = false; tl.release_pending = false; }
            if tl.down { tl.seen = true; }
        }
        let t = *tl;
        match a {
            0x8140 => b'9', 0x8141 => b'1', 0x8142 => b'1', 0x8143 => 0, 0x8144 => 0x60, 0x8145 => 0x10,      // "911", firmware 0x1060
            0x8047 => 0x41,                                                                                     // config version
            0x8048 => self.w as u8, 0x8049 => (self.w >> 8) as u8, 0x804a => self.h as u8, 0x804b => (self.h >> 8) as u8,
            0x804c => 5,                                                                                        // touch number
            0x814e => 0x80 | t.down as u8,                                                                      // buffer ready + count
            0x814f => 0, 0x8150 => t.x as u8, 0x8151 => (t.x >> 8) as u8, 0x8152 => t.y as u8, 0x8153 => (t.y >> 8) as u8, 0x8154 => 20, 0x8155 => 0, 0x8156 => 0,
            _ => 0,
        }
    }
}
impl I2cDevice for Gt911 {
    fn start(&mut self, read: bool) -> bool { if !read { self.phase = 0; } true }
    fn write(&mut self, b: u8) -> bool { match self.phase { 0 => { self.addr = (b as u16) << 8; self.phase = 1; } 1 => { self.addr |= b as u16; self.phase = 2; } _ => { self.addr = self.addr.wrapping_add(1); } } true }
    fn read(&mut self) -> u8 { let v = self.reg(self.addr); self.addr = self.addr.wrapping_add(1); self.reads += 1; v }
}

/// Hynitron CST820 touch controller used on the Waveshare Touch AMOLED 1.8 V2.
/// The register report matches the CST816S-compatible driver used by its board support package.
pub struct Cst820 { ptr: u8, first: bool, touch: std::sync::Arc<std::sync::Mutex<TouchState>>, pub reads: u64 }
impl Cst820 {
    pub fn new(touch: std::sync::Arc<std::sync::Mutex<TouchState>>) -> Self { Cst820 { ptr: 0, first: true, touch, reads: 0 } }
    fn reg(&self, addr: u8) -> u8 {
        let mut touch = self.touch.lock().expect("CST820 touch state mutex poisoned");
        if addr == 0x02 {
            if touch.release_pending && touch.seen { touch.down = false; touch.release_pending = false; }
            if touch.down { touch.seen = true; }
        }
        let touch = *touch;
        match addr {
            0x01 => 0,
            0x02 => u8::from(touch.down),
            0x03 => ((touch.x >> 8) as u8) & 0x0f,
            0x04 => touch.x as u8,
            0x05 => ((touch.y >> 8) as u8) & 0x0f,
            0x06 => touch.y as u8,
            0xa7 => 0xb7,
            0xa8 => 0x41,
            0xa9 => 0x02,
            _ => 0,
        }
    }
}
impl I2cDevice for Cst820 {
    fn start(&mut self, read: bool) -> bool { if !read { self.first = true; } true }
    fn write(&mut self, byte: u8) -> bool {
        if self.first { self.ptr = byte; self.first = false; } else { self.ptr = self.ptr.wrapping_add(1); }
        true
    }
    fn read(&mut self) -> u8 { let value = self.reg(self.ptr); self.ptr = self.ptr.wrapping_add(1); self.reads += 1; value }
}

/// FocalTech FT6336U capacitive touch controller (I2C @ 0x38) used on M5Stack CoreS3.
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
            0x02 => if touch.down { 1 } else { 0 }, // TD_STATUS: 1 touch point
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

/// AW9523B 16-channel LED driver / I/O expander (I2C @ 0x58) used on M5Stack CoreS3.
pub struct Aw9523 {
    pub regs: [u8; 256],
    ptr: u8,
    first: bool,
    pub writes: u64,
    pub port1_state: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

impl Aw9523 {
    pub fn new(port1_state: std::sync::Arc<std::sync::atomic::AtomicU8>) -> Self {
        let mut regs = [0u8; 256];
        regs[0x00] = 0xff; // Input Port 0
        regs[0x01] = 0xff; // Input Port 1
        regs[0x02] = 0xff; // Output Port 0
        regs[0x03] = 0xff; // Output Port 1
        regs[0x04] = 0x00; // Config Port 0
        regs[0x05] = 0x00; // Config Port 1
        regs[0x10] = 0x23; // Chip ID: AW9523
        regs[0x11] = 0x10; // Global Control
        regs[0x12] = 0xff; // LED Mode 0
        regs[0x13] = 0xff; // LED Mode 1
        Aw9523 { regs, ptr: 0, first: true, writes: 0, port1_state }
    }
}

impl I2cDevice for Aw9523 {
    fn start(&mut self, read: bool) -> bool { if !read { self.first = true; } true }
    fn write(&mut self, byte: u8) -> bool {
        if self.first {
            self.ptr = byte;
            self.first = false;
        } else {
            self.regs[self.ptr as usize] = byte;
            if self.ptr == 0x03 {
                self.port1_state.store(byte, std::sync::atomic::Ordering::Relaxed);
                self.regs[0x01] = byte;
            } else if self.ptr == 0x02 {
                self.regs[0x00] = byte;
            }
            self.ptr = self.ptr.wrapping_add(1);
            self.writes += 1;
        }
        true
    }
    fn read(&mut self) -> u8 {
        let value = match self.ptr {
            0x00 => self.regs[0x02],
            0x01 => self.regs[0x03],
            _ => self.regs[self.ptr as usize],
        };
        self.ptr = self.ptr.wrapping_add(1);
        value
    }
}

/// IMU state shared between the host/browser motion sensors and the BMI270 model.
#[derive(Debug, Clone, Copy)]
pub struct ImuState {
    pub ax: f32, // m/s^2
    pub ay: f32,
    pub az: f32,
    pub gx: f32, // deg/s
    pub gy: f32,
    pub gz: f32,
}

impl Default for ImuState {
    fn default() -> Self {
        ImuState {
            ax: 0.0,
            ay: 0.0,
            az: 9.80665,
            gx: 0.0,
            gy: 0.0,
            gz: 0.0,
        }
    }
}

/// Bosch BMI270 6-axis IMU (I2C @ 0x69).
pub struct Bmi270 {
    pub regs: [u8; 256],
    ptr: u8,
    first: bool,
    pub imu: std::sync::Arc<std::sync::Mutex<ImuState>>,
    pub writes: u64,
}

impl Bmi270 {
    pub fn new(imu: std::sync::Arc<std::sync::Mutex<ImuState>>) -> Self {
        let mut regs = [0u8; 256];
        regs[0x00] = 0x24; // CHIP_ID
        regs[0x02] = 0x00; // ERR_REG
        regs[0x03] = 0x60; // STATUS (drdy_acc = 1, drdy_gyr = 1)
        regs[0x04] = 0x07; // INT_STATUS_0 (drdy_acc | drdy_gyr | drdy_aux)
        regs[0x05] = 0x07; // INT_STATUS_1
        regs[0x21] = 0x01; // INTERNAL_STATUS (ASIC init message: complete)
        // BMM150 AUX chip ID and dummy mag data
        regs[0x0a] = 0x32; // BMM150 Chip ID
        Bmi270 {
            regs,
            ptr: 0,
            first: true,
            imu,
            writes: 0,
        }
    }

    fn reg(&self, addr: u8) -> u8 {
        match addr {
            0x00 => 0x24, // CHIP_ID
            0x02 => 0x00, // ERR_REG
            0x03 => 0x60, // STATUS
            0x1c => 0x00,
            0x1d => 0xe0, // AUX, gyro and accelerometer ready: bits 13..15 of INT_STATUS
            0x21 => 0x01, // INTERNAL_STATUS (init complete)
            0x0c..=0x17 => {
                let imu = *self.imu.lock().unwrap();
                // 2G range: 1G = 9.80665 m/s^2 = 16384 LSB => lsb = (m/s^2 / 9.80665) * 16384
                let lsb_from_mps2 = |v: f32| -> i16 {
                    ((v / 9.80665) * 16384.0).clamp(-32768.0, 32767.0) as i16
                };
                // 2000 dps range: 16.384 LSB / dps => lsb = dps * 16.384
                let lsb_from_dps = |v: f32| -> i16 {
                    (v * 16.384).clamp(-32768.0, 32767.0) as i16
                };
                let ax = lsb_from_mps2(imu.ax);
                let ay = lsb_from_mps2(imu.ay);
                let az = lsb_from_mps2(imu.az);
                let gx = lsb_from_dps(imu.gx);
                let gy = lsb_from_dps(imu.gy);
                let gz = lsb_from_dps(imu.gz);
                match addr {
                    0x0c => ax as u8,
                    0x0d => (ax >> 8) as u8,
                    0x0e => ay as u8,
                    0x0f => (ay >> 8) as u8,
                    0x10 => az as u8,
                    0x11 => (az >> 8) as u8,
                    0x12 => gx as u8,
                    0x13 => (gx >> 8) as u8,
                    0x14 => gy as u8,
                    0x15 => (gy >> 8) as u8,
                    0x16 => gz as u8,
                    0x17 => (gz >> 8) as u8,
                    _ => 0,
                }
            }
            a => self.regs[a as usize],
        }
    }
}

impl I2cDevice for Bmi270 {
    fn start(&mut self, read: bool) -> bool {
        if !read {
            self.first = true;
        }
        true
    }

    fn write(&mut self, byte: u8) -> bool {
        if self.first {
            self.ptr = byte;
            self.first = false;
        } else {
            self.regs[self.ptr as usize] = byte;
            self.ptr = self.ptr.wrapping_add(1);
            self.writes += 1;
        }
        true
    }

    fn read(&mut self) -> u8 {
        let val = self.reg(self.ptr);
        self.ptr = self.ptr.wrapping_add(1);
        val
    }
}

/// GalaxyCore GC0308 VGA CMOS image sensor (I2C @ 0x21).
pub struct Gc0308 {
    pub regs: [u8; 256],
    page1: [u8; 256],
    page: u8,
    ptr: u8,
    first: bool,
    pub writes: u64,
    pub sensor: std::sync::Arc<std::sync::Mutex<SensorState>>,
}

impl Gc0308 {
    pub fn new(sensor: std::sync::Arc<std::sync::Mutex<SensorState>>) -> Self {
        let mut regs = [0u8; 256];
        regs[0x00] = 0x9b; // CHIP_ID (PID)
        regs[0x24] = 0xa2; // YCbYCr output until firmware selects RGB565.
        Gc0308 {
            regs,
            page1: [0; 256],
            page: 0,
            ptr: 0,
            first: true,
            writes: 0,
            sensor,
        }
    }
}

impl I2cDevice for Gc0308 {
    fn start(&mut self, read: bool) -> bool {
        if !read {
            self.first = true;
        }
        true
    }

    fn write(&mut self, byte: u8) -> bool {
        if self.first {
            self.ptr = byte;
            self.first = false;
        } else {
            if self.ptr == 0xfe {
                if byte & 0x80 != 0 {
                    self.regs.fill(0);
                    self.regs[0x00] = 0x9b;
                    self.regs[0x24] = 0xa2;
                    self.page1.fill(0);
                    self.sensor.lock().unwrap().format = 2;
                }
                self.page = byte & 1;
            } else if self.page == 1 {
                self.page1[self.ptr as usize] = byte;
            } else if self.ptr != 0x00 {
                self.regs[self.ptr as usize] = byte;
                if self.ptr == 0x24 {
                    self.sensor.lock().unwrap().format = byte & 0x0f;
                }
            }
            self.ptr = self.ptr.wrapping_add(1);
            self.writes += 1;
        }
        true
    }

    fn read(&mut self) -> u8 {
        let value = if self.ptr == 0xfe {
            self.page
        } else if self.page == 1 {
            self.page1[self.ptr as usize]
        } else {
            self.regs[self.ptr as usize]
        };
        self.ptr = self.ptr.wrapping_add(1);
        value
    }
}

/// Lite-On LTR-553ALS-WA ambient light and proximity sensor (I2C @ 0x23).
pub struct Ltr553 {
    pub regs: [u8; 256],
    ptr: u8,
    first: bool,
    pub writes: u64,
}

impl Default for Ltr553 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ltr553 {
    pub fn new() -> Self {
        let mut regs = [0u8; 256];
        regs[0x86] = 0x05; // MANUFAC_ID
        regs[0x87] = 0x92; // PART_ID
        regs[0x88] = 0x10; // ALS_DATA_CH1_0
        regs[0x89] = 0x00; // ALS_DATA_CH1_1
        regs[0x8A] = 0x20; // ALS_DATA_CH0_0
        regs[0x8B] = 0x00; // ALS_DATA_CH0_1
        regs[0x8C] = 0x00; // ALS_STATUS
        regs[0x8D] = 0x00; // PS_DATA_0
        regs[0x8E] = 0x01; // PS_DATA_1
        Ltr553 {
            regs,
            ptr: 0,
            first: true,
            writes: 0,
        }
    }
}

impl I2cDevice for Ltr553 {
    fn start(&mut self, read: bool) -> bool {
        if !read {
            self.first = true;
        }
        true
    }

    fn write(&mut self, byte: u8) -> bool {
        if self.first {
            self.ptr = byte;
            self.first = false;
        } else {
            self.regs[self.ptr as usize] = byte;
            self.ptr = self.ptr.wrapping_add(1);
            self.writes += 1;
        }
        true
    }

    fn read(&mut self) -> u8 {
        let val = self.regs[self.ptr as usize];
        self.ptr = self.ptr.wrapping_add(1);
        val
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn read_registers(device: &mut Cst820, first: u8, count: usize) -> Vec<u8> {
        assert!(device.start(false));
        assert!(device.write(first));
        assert!(device.start(true));
        (0..count).map(|_| device.read()).collect()
    }

    #[test]
    fn cst820_reports_board_identity() {
        let mut device = Cst820::new(Default::default());
        assert_eq!(read_registers(&mut device, 0xa7, 3), [0xb7, 0x41, 0x02]);
    }

    #[test]
    fn cst820_reports_touch_coordinates() {
        let touch = std::sync::Arc::new(std::sync::Mutex::new(TouchState { down: true, x: 0x167, y: 0x1bf, ..Default::default() }));
        let mut device = Cst820::new(touch);
        assert_eq!(read_registers(&mut device, 0x02, 5), [1, 0x01, 0x67, 0x01, 0xbf]);
    }

    #[test]
    fn tca9554_register_stub_keeps_written_outputs() {
        let mut device = Tca9554::register_ram_stub();
        assert!(device.start(false));
        assert!(device.write(1));
        assert!(device.write(0xa5));
        assert!(device.start(false));
        assert!(device.write(1));
        assert!(device.start(true));
        assert_eq!(device.read(), 0xa5);
    }

    #[test]
    fn tca9554_input_port_is_read_only_and_reflects_pin_levels() {
        let mut device = Tca9554::register_ram_stub();
        assert!(device.start(false));
        assert!(device.write(0));
        assert!(device.write(0));
        assert_eq!(device.regs[0], 0xff);

        assert!(device.start(false));
        assert!(device.write(3));
        assert!(device.write(0xf0));
        assert!(device.start(false));
        assert!(device.write(1));
        assert!(device.write(0x05));
        assert!(device.start(false));
        assert!(device.write(0));
        assert!(device.start(true));
        assert_eq!(device.read(), 0xf5);
    }

    #[test]
    fn ft6336_reports_id_and_touch() {
        let touch = std::sync::Arc::new(std::sync::Mutex::new(TouchState { down: true, x: 120, y: 180, ..Default::default() }));
        let mut device = Ft6336::new(touch);
        assert!(device.start(false));
        assert!(device.write(0xa3)); // Chip ID
        assert!(device.start(true));
        assert_eq!(device.read(), 0x64);

        assert!(device.start(false));
        assert!(device.write(0x02)); // TD_STATUS
        assert!(device.start(true));
        assert_eq!(device.read(), 1); // 1 point
        assert_eq!(device.read(), 0x00); // P1_XH: 0 | (120 >> 8)
        assert_eq!(device.read(), 120);  // P1_XL: 120
        assert_eq!(device.read(), 0x00); // P1_YH: 0 | (180 >> 8)
        assert_eq!(device.read(), 180);  // P1_YL: 180
    }

    #[test]
    fn aw9523_reports_id_and_updates_port1() {
        let port1 = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let mut device = Aw9523::new(port1.clone());
        assert!(device.start(false));
        assert!(device.write(0x10)); // ID register
        assert!(device.start(true));
        assert_eq!(device.read(), 0x23);

        assert!(device.start(false));
        assert!(device.write(0x03)); // Output Port 1
        assert!(device.write(0b0000_0110)); // Enable LCD reset & backlight
        assert_eq!(port1.load(std::sync::atomic::Ordering::Relaxed), 0b0000_0110);
    }
}
