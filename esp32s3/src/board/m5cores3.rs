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
//!   - Camera: GC0308 on I2C0 @0x21, RGB565 or YUYV over the LCD_CAM DVP port

use esp_soc::board::{BoardEdge, BoardModel, VirtualCycle};
use esp_soc::devices::{
    i2c_bitbang::{I2cBitBang, SharedI2cDevice},
    DcsPanel,
};
use esp_soc::picture::{self, Picture, PixelFormat};
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

/// Camera sensor frame the guest demo requests before the host picks a picture: an SMB Bartholomäus
/// colour-bar pattern that makes geometry and channel order visible (RGB565 or YUYV alike).
fn test_pattern(w: u32, h: u32) -> Picture {
    let colors = [
        [255, 255, 255], // White
        [255, 255, 0],   // Yellow
        [0, 255, 255],   // Cyan
        [0, 255, 0],     // Green
        [255, 0, 255],   // Magenta
        [255, 0, 0],     // Red
        [0, 0, 255],     // Blue
        [30, 30, 30],    // Dark Gray
    ];
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let bar = (x * 8 / w) as usize;
            let c = colors[bar.min(7)];
            let idx = ((y * w + x) * 3) as usize;
            rgb[idx] = c[0];
            rgb[idx + 1] = c[1];
            rgb[idx + 2] = c[2];
        }
    }
    Picture { w, h, rgb }
}

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
    /// The QMI8658-style scripted motion (0 = follow the browser stream).
    pub motion: Arc<crate::i2c::ImuMotion>,
    pub imu_state: Arc<Mutex<crate::i2c::ImuState>>,
    pub sensor_state: Arc<Mutex<crate::i2c::SensorState>>,
    pub picture: Option<picture::Picture>,
    pub camera_frame_cache: Option<(u32, u32, Arc<Vec<u8>>)>,
    camera_frame_format: u8,
    software_i2c: I2cBitBang,
    release_sda: bool,
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
            motion: Arc::new(crate::i2c::ImuMotion::default()),
            imu_state: Arc::new(Mutex::new(crate::i2c::ImuState::default())),
            sensor_state: Arc::new(Mutex::new(crate::i2c::SensorState {
                width: 160,
                height: 120,
                format: 2,
                streaming: true,
                window: None,
                jpeg: false,
            })),
            picture: Some(test_pattern(160, 120)),
            camera_frame_cache: None,
            camera_frame_format: 2,
            software_i2c: I2cBitBang::new(),
            release_sda: false,
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

    fn gpio_output_at(
        &mut self,
        cycle: VirtualCycle,
        changes: &[(u8, bool)],
        enabled: u64,
        output: u64,
    ) {
        self.gpio_changes(changes);
        let released = |pin| enabled & !output & (1u64 << pin) == 0;
        if let Some(low) = self
            .software_i2c
            .update(released(PIN_I2C_SCL), released(PIN_I2C_SDA))
        {
            self.release_sda = !low;
            if low {
                self.edges.push(BoardEdge {
                    cycle,
                    pin: PIN_I2C_SDA,
                    level: false,
                });
            }
        }
    }

    fn released_inputs(&mut self) -> Vec<u8> {
        if std::mem::take(&mut self.release_sda) {
            vec![PIN_I2C_SDA]
        } else {
            Vec::new()
        }
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
                // rx_len > 0 here: the rx_len == 0 case returned above.
                resp[0] = 0x08;
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
        let core_buses: [(u8, &str); 12] = [
            (0x34, "axp2101"), // PMU
            (0x58, "aw9523"),  // IO expander
            (0x38, "ft6336"),  // touch controller
            (0x21, "gc0308"),  // camera sensor
            (0x23, "ltr553"),  // ambient light / proximity sensor
            (0x51, "bm8563"),  // RTC
            (0x69, "bmi270"),  // IMU
            (0x40, "es7210"),  // audio codec
            (0x36, "aw88298"), // audio amp
            (0x0c, "ak09918"), // magnetometer (secondary device)
            (0x10, "bmm150"),  // magnetometer (BMI270 AUX device)
            (0x68, "bm8563_alt"),
        ];
        let mut list = Vec::with_capacity(24);
        self.software_i2c = I2cBitBang::new();
        self.release_sda = true;
        for &(addr, name) in &core_buses {
            let device: Box<dyn crate::i2c::I2cDevice> = match name {
                "aw9523" => Box::new(Aw9523::new(self.aw9523_p1.clone())),
                "ft6336" => Box::new(Ft6336::new(self.touch_state.clone())),
                "gc0308" => Box::new(Gc0308::new(self.sensor_state.clone())),
                "ltr553" => Box::new(Ltr553::new()),
                "bmi270" => Box::new(Bmi270::new(self.motion.clone(), self.imu_state.clone())),
                "axp2101" => Box::new(Reg8Device::new("axp2101", axp_regs)),
                "bm8563" => Box::new(Reg8Device::new(
                    "bm8563",
                    &[(0x02, 0x00), (0x03, 0x00), (0x04, 0x12)],
                )),
                "es7210" => Box::new(Reg8Device::new("es7210", &[(0x3d, 0x72), (0x3e, 0x10)])),
                "aw88298" => Box::new(Reg8Device::new("aw88298", &[(0x00, 0x18)])),
                "ak09918" => Box::new(Reg8Device::new("ak09918", &[(0x00, 0x48), (0x01, 0x0c)])),
                "bmm150" => Box::new(Reg8Device::new("bmm150", &[(0x40, 0x32)])),
                "bm8563_alt" => Box::new(Reg8Device::new("bm8563_alt", &[(0x00, 0x00)])),
                _ => unreachable!("CoreS3 I2C device table covers every name"),
            };
            let device = SharedI2cDevice::new(device);
            self.software_i2c.attach(addr, device.clone());
            // Both hardware controllers and GPIO I2C address the same physical chips.
            for bus in [0u8, 1u8] {
                list.push((bus, addr, Box::new(device.clone()) as Box<dyn I2cDevice>));
            }
        }
        list
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
        use std::sync::atomic::Ordering::Relaxed;
        // The IMU's scripted motion reads guest time from the clock the board keeps.
        self.motion.cycle.store(cycle, Relaxed);
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

    fn set_imu_motion(&mut self, mode: u32) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        self.motion.mode.store(mode, Relaxed);
        let mut state = self.imu_state.lock().unwrap();
        match mode {
            0 => {
                // Back to the browser-driven stream: a still default until a message arrives.
                *state = crate::i2c::ImuState::default();
                true
            }
            _ => true,
        }
    }

    fn imu_motion(&self) -> u32 {
        use std::sync::atomic::Ordering::Relaxed;
        self.motion.mode.load(Relaxed)
    }

    /// Live sensor values from the browser (m/s^2 and dps) flow into the BMI270 model.
    fn set_imu_state(&mut self, ax: f32, ay: f32, az: f32, gx: f32, gy: f32, gz: f32) {
        let mut imu = self.imu_state.lock().unwrap();
        imu.ax = ax;
        imu.ay = ay;
        imu.az = az;
        imu.gx = gx;
        imu.gy = gy;
        imu.gz = gz;
    }

    fn set_camera_picture(&mut self, p: Picture) -> Result<(), &'static str> {
        if !p.valid() {
            return Err("invalid camera picture or frame exceeds 8 MiB");
        }
        self.picture = Some(p);
        self.camera_frame_cache = None;
        Ok(())
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
                    default_pic = test_pattern(w, h);
                    &default_pic
                }
            };
            // GC0308 OUTPUT_FMT (page 0, register 0x24) selects the DVP format. The
            // CoreS3 demo requests RGB565 (0x6) and sends the framebuffer directly
            // to the LCD; other values give the bus YUYV as esp32-camera would.
            let pixels = if format == 6 {
                picture::sensor_frame(p, w, h, PixelFormat::Rgb565)
                    .unwrap_or_else(|| picture::to_yuyv(p, w, h))
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

#[cfg(test)]
mod software_i2c_tests {
    use super::*;
    use esp_periph::Gpio;

    fn drive(board: &mut M5CoreS3, gpio: &mut Gpio, clock: bool, data: bool) {
        let enabled = (u64::from(!clock) << PIN_I2C_SCL) | (u64::from(!data) << PIN_I2C_SDA);
        gpio.write(0x20, enabled as u32);
        board.gpio_output_at(0, &[], enabled, 0);
        esp_soc::gpio::drain_board_inputs(board, gpio, &mut None, 0);
    }

    fn write(board: &mut M5CoreS3, gpio: &mut Gpio, byte: u8) -> bool {
        for bit in (0..8).rev() {
            let high = byte & (1 << bit) != 0;
            drive(board, gpio, false, high);
            drive(board, gpio, true, high);
        }
        drive(board, gpio, false, true);
        drive(board, gpio, true, true);
        let ack = !gpio.level(PIN_I2C_SDA);
        drive(board, gpio, false, true);
        ack
    }

    #[test]
    fn open_drain_gpio_writes_share_the_hardware_i2c_device() {
        let mut board = M5CoreS3::new();
        let mut devices = board.i2c_devices();
        let mut gpio = Gpio::new();
        drive(&mut board, &mut gpio, true, true);
        drive(&mut board, &mut gpio, true, false);
        assert!(write(&mut board, &mut gpio, 0x58 << 1));
        assert!(write(&mut board, &mut gpio, 0x03));
        assert!(write(&mut board, &mut gpio, 0x37));
        drive(&mut board, &mut gpio, false, false);
        drive(&mut board, &mut gpio, true, false);
        drive(&mut board, &mut gpio, true, true);
        assert!(gpio.level(PIN_I2C_SDA));
        assert_eq!(board.aw9523_p1.load(Ordering::Relaxed), 0x37);
        for (_, address, device) in &mut devices {
            if *address == 0x58 {
                assert!(device.start(false));
                assert!(device.write(0x03));
                assert!(device.start(true));
                assert_eq!(device.read(), 0x37);
            }
        }
    }
}
