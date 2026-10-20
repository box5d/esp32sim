//! M5Stack CoreS3 board model:
//!   - SoC: ESP32-S3 (Xtensa LX7 dual core)
//!   - Display: ILI9342C 320x240 on GP-SPI2 (SCLK 36, MOSI 37, D/C 35, CS 3)
//!   - Touch: FT6336U capacitive touch controller on I2C0 @0x38 (SDA 12, SCL 11, INT 21)
//!   - PMU: AXP2101 on I2C0 @0x34
//!   - IO Expander: AW9523B on I2C0 @0x58 (controls LCD reset P1_1, backlight P1_2, bus power)
//!   - RTC: BM8563 on I2C0 @0x51
//!   - IMU: BMI270 on I2C0 @0x69
//!   - Audio Codec: ES7210 on I2C0 @0x40 + I2S
//!   - Audio Amp: AW88298 on I2C0 @0x36 + I2S

use esp_soc::board::{BoardEdge, BoardModel, VirtualCycle};
use esp_soc::devices::DcsPanel;
use esp_soc::picture::{self, Picture};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

pub const PIN_LCD_CS: u8 = 3;
pub const PIN_LCD_DC: u8 = 35;
pub const PIN_LCD_SCLK: u8 = 36;
pub const PIN_LCD_MOSI: u8 = 37;
pub const PIN_TOUCH_INT: u8 = 21;
pub const PIN_I2C_SDA: u8 = 12;
pub const PIN_I2C_SCL: u8 = 11;
pub const PIN_SD_CS: u8 = 4;

pub const LCD_WIDTH: u32 = 320;
pub const LCD_HEIGHT: u32 = 240;

pub struct M5CoreS3 {
    pub tft: DcsPanel,
    pub touch_state: Arc<Mutex<crate::i2c::TouchState>>,
    pub aw9523_p1: Arc<AtomicU8>,
    pub gpio_events: u64,
    pub touch_irq_level: bool,
    pub cycle: VirtualCycle,
    pub last_cmd: u8,
    pub edges: Vec<BoardEdge>,
    pub pending_touch_irq: Option<(VirtualCycle, bool)>,
    pub pending_touch_final: Option<bool>,
    pub imu_state: Arc<Mutex<crate::i2c::ImuState>>,
    pub sensor_state: Arc<Mutex<crate::i2c::SensorState>>,
    pub picture: Option<esp_soc::picture::Picture>,
    pub camera_frame_cache: Option<(u32, u32, Arc<Vec<u8>>)>,
    camera_frame_format: u8,
    pub camera_frames: u64,
}

impl Default for M5CoreS3 {
    fn default() -> Self {
        Self::new()
    }
}

impl M5CoreS3 {
    pub fn new() -> Self {
        M5CoreS3 {
            tft: DcsPanel::new(LCD_WIDTH as usize, LCD_HEIGHT as usize),
            touch_state: Arc::new(Mutex::new(crate::i2c::TouchState::default())),
            aw9523_p1: Arc::new(AtomicU8::new(0xff)),
            gpio_events: 0,
            touch_irq_level: true, // active low, so default high
            cycle: 0,
            last_cmd: 0,
            edges: Vec::new(),
            pending_touch_irq: None,
            pending_touch_final: None,
            imu_state: Arc::new(Mutex::new(crate::i2c::ImuState::default())),
            sensor_state: Arc::new(Mutex::new(crate::i2c::SensorState {
                width: 160,
                height: 120,
                format: 2,
                streaming: true,
            })),
            picture: Some(esp_soc::picture::default_test_pattern(160, 120)),
            camera_frame_cache: None,
            camera_frame_format: 2,
            camera_frames: 0,
        }
    }

    /// Renders the 320x240 frame respecting MADCTL rotation/mirroring flags.
    pub fn frame(&self) -> Vec<u16> {
        let cols = self.tft.cols;
        let rows = self.tft.rows;
        let mut out = vec![0u16; (LCD_WIDTH * LCD_HEIGHT) as usize];
        let p = &self.tft;

        let mv = p.madctl & 0x20 != 0;
        let mx = p.madctl & 0x40 != 0;
        let my = p.madctl & 0x80 != 0;

        for y in 0..(LCD_HEIGHT as usize) {
            for x in 0..(LCD_WIDTH as usize) {
                let (c, r) = if mv {
                    let col = if my { LCD_HEIGHT as usize - 1 - y } else { y };
                    let row = if mx { LCD_WIDTH as usize - 1 - x } else { x };
                    (col, row)
                } else {
                    let col = if mx { LCD_WIDTH as usize - 1 - x } else { x };
                    let row = if my { LCD_HEIGHT as usize - 1 - y } else { y };
                    (col, row)
                };

                let c = c.min(cols - 1);
                let r = r.min(rows - 1);
                let px = p.gram[r * cols + c];
                out[y * (LCD_WIDTH as usize) + x] = px;
            }
        }
        out
    }

    fn queue_touch(&mut self, cycle: VirtualCycle, x: u16, y: u16, down: bool) {
        let mut touch = self
            .touch_state
            .lock()
            .expect("M5CoreS3 touch mutex poisoned");
        touch.x = x.min(LCD_WIDTH as u16 - 1);
        touch.y = y.min(LCD_HEIGHT as u16 - 1);
        if down {
            touch.down = true;
            touch.seen = false;
            touch.release_pending = false;
        } else if touch.seen {
            touch.down = false;
        } else {
            touch.release_pending = true;
        }
        drop(touch);

        // FT6336U INT is active low: down=true -> level=false (LOW), down=false -> level=true (HIGH)
        let level = !down;
        let queued_level = self
            .pending_touch_final
            .or(self.pending_touch_irq.map(|(_, l)| l))
            .unwrap_or(self.touch_irq_level);

        if level != queued_level {
            // Keep LOW asserted for at least 5ms (1,200,000 cycles @ 240MHz) on release to ensure guest OS reads it
            let target_cycle = if level {
                cycle.max(self.cycle).saturating_add(240_000 * 5)
            } else {
                cycle.max(self.cycle).saturating_add(1)
            };
            match self.pending_touch_irq {
                Some((_, first_level)) if level == first_level => {
                    self.pending_touch_final = None;
                }
                Some(_) => {
                    self.pending_touch_final = Some(level);
                }
                None => {
                    self.pending_touch_irq = Some((target_cycle, level));
                }
            }
        }
    }
}

impl BoardModel for M5CoreS3 {
    fn name(&self) -> &'static str {
        "m5cores3"
    }

    fn gpio_changes(&mut self, changes: &[(u8, bool)]) {
        for &(pin, level) in changes {
            self.gpio_events += 1;
            if pin == PIN_LCD_DC {
                self.tft.dc = level;
            }
        }
    }

    fn gpio_events(&self) -> u64 {
        self.gpio_events
    }

    fn spi_transfer(&mut self, host: u8, tx: &[u8], rx_len: usize) -> Vec<u8> {
        if host == 2 {
            for &b in tx {
                if !self.tft.dc {
                    self.last_cmd = b;
                }
                self.tft.byte(b);
            }
        }
        if rx_len == 0 {
            return Vec::new();
        }
        let mut resp = vec![0x00; rx_len];
        match self.last_cmd {
            0x04 | 0xd3 => {
                // ILI9342C: M5GFX checks ((id & 0xFF) == 0xE3)
                // When read via SPI readData(32), byte 0 ends up in id & 0xFF
                let id = [0xe3, 0x00, 0x93, 0x42];
                for (i, &b) in id.iter().enumerate().take(rx_len) {
                    resp[i] = b;
                }
            }
            0x09 => {
                // Read display status
                let st = [0x00, 0x61, 0x00, 0x00];
                for (i, &b) in st.iter().enumerate().take(rx_len) {
                    resp[i] = b;
                }
            }
            0x0a => {
                if !resp.is_empty() {
                    resp[0] = 0x08;
                }
            }
            _ => {}
        }
        resp
    }

    fn spi_tx(&mut self, host: u8, data: &[u8]) {
        if host == 2 {
            for &b in data {
                if !self.tft.dc {
                    self.last_cmd = b;
                }
                self.tft.byte(b);
            }
        }
    }

    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn crate::i2c::I2cDevice>)> {
        use crate::i2c::*;
        let axp_regs = &[
            (0x03, 0x4a), // Chip ID: AXP2101
            (0x00, 0x01), // Power status
            (0x10, 0x01), // PMU status
            (0x12, 0xff), // Power on status
            (0x20, 0xff), // DCDC on
            (0x80, 0x01), // Charging status
        ];
        vec![
            // I2C0 Bus (Wire)
            (0, 0x34, Box::new(Reg8Device::new("axp2101", axp_regs))),
            (0, 0x58, Box::new(Aw9523::new(self.aw9523_p1.clone()))),
            (0, 0x38, Box::new(Ft6336::new(self.touch_state.clone()))),
            (0, 0x21, Box::new(Gc0308::new(self.sensor_state.clone()))),
            (0, 0x23, Box::new(crate::i2c::Ltr553::new())),
            (
                0,
                0x51,
                Box::new(Reg8Device::new(
                    "bm8563",
                    &[(0x02, 0x00), (0x03, 0x00), (0x04, 0x12)],
                )),
            ),
            (0, 0x69, Box::new(Bmi270::new(self.imu_state.clone()))),
            (
                0,
                0x40,
                Box::new(Reg8Device::new("es7210", &[(0x3d, 0x72), (0x3e, 0x10)])),
            ),
            (
                0,
                0x36,
                Box::new(Reg8Device::new("aw88298", &[(0x00, 0x18)])),
            ),
            (
                0,
                0x0c,
                Box::new(Reg8Device::new("ak09918", &[(0x00, 0x48), (0x01, 0x0c)])),
            ),
            (
                0,
                0x10,
                Box::new(Reg8Device::new("bmm150", &[(0x40, 0x32)])),
            ),
            (
                0,
                0x68,
                Box::new(Reg8Device::new("bm8563_alt", &[(0x00, 0x00)])),
            ),
            // I2C1 Bus (Wire1 - for peripherals mapped to secondary bus)
            (1, 0x34, Box::new(Reg8Device::new("axp2101", axp_regs))),
            (1, 0x58, Box::new(Aw9523::new(self.aw9523_p1.clone()))),
            (1, 0x38, Box::new(Ft6336::new(self.touch_state.clone()))),
            (1, 0x21, Box::new(Gc0308::new(self.sensor_state.clone()))),
            (1, 0x23, Box::new(crate::i2c::Ltr553::new())),
            (
                1,
                0x51,
                Box::new(Reg8Device::new(
                    "bm8563",
                    &[(0x02, 0x00), (0x03, 0x00), (0x04, 0x12)],
                )),
            ),
            (1, 0x69, Box::new(Bmi270::new(self.imu_state.clone()))),
            (
                1,
                0x40,
                Box::new(Reg8Device::new("es7210", &[(0x3d, 0x72), (0x3e, 0x10)])),
            ),
            (
                1,
                0x36,
                Box::new(Reg8Device::new("aw88298", &[(0x00, 0x18)])),
            ),
            (
                1,
                0x0c,
                Box::new(Reg8Device::new("ak09918", &[(0x00, 0x48), (0x01, 0x0c)])),
            ),
            (
                1,
                0x10,
                Box::new(Reg8Device::new("bmm150", &[(0x40, 0x32)])),
            ),
            (
                1,
                0x68,
                Box::new(Reg8Device::new("bm8563_alt", &[(0x00, 0x00)])),
            ),
        ]
    }

    fn display(&self) -> Option<(u32, u32, Vec<u16>, u64)> {
        Some((LCD_WIDTH, LCD_HEIGHT, self.frame(), self.tft.pixels_written))
    }

    fn display_version(&self) -> u64 {
        self.tft.pixels_written
    }

    fn display_frames(&self) -> u64 {
        self.tft.frames
    }

    fn display_quiet_push(&self) -> bool {
        true
    }

    fn gram(&self) -> Option<(Vec<u16>, usize, usize)> {
        Some((self.tft.gram.clone(), self.tft.cols, self.tft.rows))
    }

    fn input_levels(&self) -> Vec<(u8, bool)> {
        vec![(PIN_TOUCH_INT, self.touch_irq_level)]
    }

    fn touch(&mut self, x: u16, y: u16, down: bool) {
        self.queue_touch(self.cycle, x, y, down);
        if self.cycle == 0 {
            self.touch_irq_level = !down;
            self.pending_touch_irq = None;
        }
    }

    fn touch_at(&mut self, cycle: VirtualCycle, x: u16, y: u16, down: bool) {
        self.queue_touch(cycle, x, y, down);
    }

    fn next_deadline(&self) -> Option<VirtualCycle> {
        self.pending_touch_irq.map(|(touch, _)| touch)
    }

    fn advance_to(&mut self, cycle: VirtualCycle) {
        assert!(
            cycle >= self.cycle,
            "board time moved backwards from {} to {}",
            self.cycle,
            cycle
        );
        while self
            .next_deadline()
            .is_some_and(|deadline| deadline <= cycle)
        {
            let deadline = self
                .next_deadline()
                .expect("due board transition must have a deadline");
            if self
                .pending_touch_irq
                .is_some_and(|(touch_cycle, _)| touch_cycle == deadline)
            {
                let (_, level) = self
                    .pending_touch_irq
                    .take()
                    .expect("due touch interrupt must remain pending");
                self.touch_irq_level = level;
                self.edges.push(BoardEdge {
                    cycle: deadline,
                    pin: PIN_TOUCH_INT,
                    level,
                });
                if self
                    .pending_touch_final
                    .is_some_and(|final_level| final_level != level)
                {
                    let final_level = self
                        .pending_touch_final
                        .take()
                        .expect("different final touch level must remain pending");
                    let next_delay = if final_level { 240_000 * 5 } else { 1 };
                    self.pending_touch_irq =
                        Some((deadline.saturating_add(next_delay), final_level));
                } else {
                    self.pending_touch_final = None;
                }
            }
        }
        self.cycle = cycle;
    }

    fn take_edges(&mut self) -> Vec<BoardEdge> {
        std::mem::take(&mut self.edges)
    }

    fn set_imu_state(&mut self, ax: f32, ay: f32, az: f32, gx: f32, gy: f32, gz: f32) {
        let mut imu = self.imu_state.lock().unwrap();
        imu.ax = ax;
        imu.ay = ay;
        imu.az = az;
        imu.gx = gx;
        imu.gy = gy;
        imu.gz = gz;
    }

    fn set_camera_picture(&mut self, p: Picture) {
        self.picture = Some(p);
        self.camera_frame_cache = None;
    }

    fn camera_preview(&self, w: u32, h: u32) -> Option<Vec<u8>> {
        let p = self.picture.as_ref()?;
        let mut out = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            let sy = (y as u64 * p.h as u64 / h as u64) as usize;
            for x in 0..w {
                let sx = (x as u64 * p.w as u64 / w as u64) as usize;
                let o = (sy * p.w as usize + sx) * 3;
                out.extend_from_slice(&p.rgb[o..o + 3]);
            }
        }
        Some(out)
    }

    fn camera_frame(&mut self) -> Option<(u32, u32, Arc<Vec<u8>>)> {
        let (w, h, format) = {
            let s = self.sensor_state.lock().unwrap();
            (
                if s.width > 0 { s.width } else { 160 },
                if s.height > 0 { s.height } else { 120 },
                s.format,
            )
        };
        let stale = match &self.camera_frame_cache {
            Some((fw, fh, _)) => *fw != w || *fh != h || self.camera_frame_format != format,
            None => true,
        };
        if stale {
            let default_pic;
            let p = match self.picture.as_ref() {
                Some(pic) => pic,
                None => {
                    default_pic = picture::default_test_pattern(w, h);
                    &default_pic
                }
            };
            // GC0308 OUTPUT_FMT (page 0, register 0x24) selects the DVP format.
            // The CoreS3 demo requests RGB565 and sends the framebuffer directly
            // to the LCD; returning YUYV here corrupts the phone camera image.
            let pixels = if format == 6 {
                picture::to_rgb565(p, w, h)
            } else {
                picture::to_yuyv(p, w, h)
            };
            self.camera_frame_cache = Some((w, h, Arc::new(pixels)));
            self.camera_frame_format = format;
        }
        self.camera_frames += 1;
        self.camera_frame_cache.clone()
    }

    fn report(&self) -> String {
        let t = &self.tft;
        let p1 = self.aw9523_p1.load(Ordering::Relaxed);
        let lcd_reset = (p1 & (1 << 1)) != 0;
        let backlight = (p1 & (1 << 2)) != 0;
        format!(
            "[m5cores3] LCD: {} frames, {} pixels, madctl={:#x} inv={} on={} bbox={:?}\n[m5cores3] AW9523 P1={:#04x} (reset={}, bl={}), touch_irq={}",
            t.frames, t.pixels_written, t.madctl, t.inverted, t.on, t.bbox(), p1, lcd_reset, backlight, self.touch_irq_level
        )
    }
}
