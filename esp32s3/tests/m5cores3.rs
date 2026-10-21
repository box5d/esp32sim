use esp32s3::board::*;

fn write_cmd(b: &mut M5CoreS3, c: u8, params: &[u8]) {
    b.gpio_changes(&[(PIN_LCD_DC, false)]);
    b.spi_tx(2, &[c]);
    if !params.is_empty() {
        b.gpio_changes(&[(PIN_LCD_DC, true)]);
        b.spi_tx(2, params);
    }
}

#[test]
fn m5cores3_board_registration_and_naming() {
    let board = make_board("m5cores3").expect("m5cores3 board should be registered");
    assert_eq!(board.name(), "m5cores3");

    let board_alias = make_board("cores3").expect("cores3 alias should work");
    assert_eq!(board_alias.name(), "m5cores3");
}

#[test]
fn m5cores3_display_pipeline() {
    let mut b = M5CoreS3::new();
    assert_eq!(b.name(), "m5cores3");

    // Exit sleep, turn on display
    write_cmd(&mut b, 0x11, &[]);
    write_cmd(&mut b, 0x29, &[]);
    assert!(!b.tft.sleeping);
    assert!(b.tft.on);

    // Set address window 0..319, 0..239
    write_cmd(&mut b, 0x2a, &[0x00, 0x00, 0x01, 0x3f]);
    write_cmd(&mut b, 0x2b, &[0x00, 0x00, 0x00, 0xef]);

    // Send RAMWR command
    b.gpio_changes(&[(PIN_LCD_DC, false)]);
    b.spi_tx(2, &[0x2c]);

    // Write 4 pixels of pure green (RGB565: 0x07e0)
    b.gpio_changes(&[(PIN_LCD_DC, true)]);
    let green_pixels = [0x07, 0xe0, 0x07, 0xe0, 0x07, 0xe0, 0x07, 0xe0];
    b.spi_tx(2, &green_pixels);

    assert_eq!(b.tft.pixels_written, 4);
    assert_eq!(b.display_frames(), 1);

    let (w, h, frame, written) = b.display().expect("display should be available");
    assert_eq!(w, 320);
    assert_eq!(h, 240);
    assert_eq!(written, 4);
    assert_eq!(frame.len(), 320 * 240);
    assert_eq!(frame[0], 0x07e0);
    assert_eq!(frame[1], 0x07e0);
    assert_eq!(frame[2], 0x07e0);
    assert_eq!(frame[3], 0x07e0);
    assert_eq!(frame[4], 0x0000); // unwritten pixel remains 0
}

#[test]
fn m5cores3_touch_and_irq() {
    let mut b = M5CoreS3::new();
    let mut devices = b.i2c_devices();
    let ft6336_idx = devices.iter().position(|(_, addr, _)| *addr == 0x38).expect("FT6336 at 0x38");
    let dev = &mut devices[ft6336_idx].2;

    // Check Chip ID
    assert!(dev.start(false));
    assert!(dev.write(0xa3));
    assert!(dev.start(true));
    assert_eq!(dev.read(), 0x64);

    // Initial state: no touch
    assert!(dev.start(false));
    assert!(dev.write(0x02));
    assert!(dev.start(true));
    assert_eq!(dev.read(), 0); // TD_STATUS = 0
    assert_eq!(b.input_levels(), vec![(PIN_TOUCH_INT, true)]); // Active-low IRQ inactive (high)

    // Touch down at (150, 200)
    b.touch(150, 200, true);
    assert_eq!(b.input_levels(), vec![(PIN_TOUCH_INT, false)]); // Active-low IRQ asserted (low)

    assert!(dev.start(false));
    assert!(dev.write(0x02));
    assert!(dev.start(true));
    assert_eq!(dev.read(), 1); // 1 point touch
    assert_eq!(dev.read(), 0x00); // P1_XH: 0 | (150 >> 8)
    assert_eq!(dev.read(), 150);  // P1_XL: 150
    assert_eq!(dev.read(), 0x00); // P1_YH: 0 | (200 >> 8)
    assert_eq!(dev.read(), 200);  // P1_YL: 200

    // Touch release
    b.touch(150, 200, false);
    assert_eq!(b.input_levels(), vec![(PIN_TOUCH_INT, true)]); // IRQ back to high
}

#[test]
fn m5cores3_i2c_peripherals() {
    let mut b = M5CoreS3::new();
    let mut devices = b.i2c_devices();

    // AXP2101 @ 0x34
    let axp = &mut devices.iter_mut().find(|(_, addr, _)| *addr == 0x34).expect("AXP2101").2;
    assert!(axp.start(false));
    assert!(axp.write(0x03)); // Chip ID
    assert!(axp.start(true));
    assert_eq!(axp.read(), 0x4a);

    // AW9523B @ 0x58
    let aw = &mut devices.iter_mut().find(|(_, addr, _)| *addr == 0x58).expect("AW9523B").2;
    assert!(aw.start(false));
    assert!(aw.write(0x10)); // ID register
    assert!(aw.start(true));
    assert_eq!(aw.read(), 0x23);

    // Write backlight and LCD reset on AW9523 Output Port 1
    assert!(aw.start(false));
    assert!(aw.write(0x03));
    assert!(aw.write(0b0000_0110));
    assert_eq!(b.aw9523_p1.load(std::sync::atomic::Ordering::Relaxed), 0b0000_0110);

    // BMI270 @ 0x69
    let imu = &mut devices.iter_mut().find(|(_, addr, _)| *addr == 0x69).expect("BMI270").2;
    assert!(imu.start(false));
    assert!(imu.write(0x00)); // Chip ID
    assert!(imu.start(true));
    assert_eq!(imu.read(), 0x24);

    // Report string sanity
    let report = b.report();
    assert!(report.contains("[m5cores3] LCD:"));
    assert!(report.contains("AW9523 P1="));
}

/// SENS ADC1 one-shot on a CoreS3-shaped machine: Port BGPIO8 (ADC1 channel 7) converts the
/// host-supplied pad voltage through SENS_SAR_MEAS1_CTRL2, the register the IDF 4.4 firmware
/// (Arduino-ESP32 2.x, as shipped on CoreS3) reaches at RTC_CNTL base + 0x800 + 0x0c.
#[test]
fn m5cores3_sens_adc1_oneshot_converts_port_b() {
    use emu_core::Bus;
    use esp_periph::{rtc_cntl::RtcCntl, AnalogSource};
    use esp_soc::SocBus;

    const CTRL: u32 = 0x6000_880c;    // SENS_SAR_MEAS1_CTRL2: DATA [15:0], DONE 16, START 17, EN_PAD [30:19]
    const ATTEN: u32 = 0x6000_8814;   // SENS_SAR_ATTEN1: channel 7 attenuation in bits 15:14
    const EN_PAD7: u32 = 1 << (19 + 7);
    const START: u32 = 1 << 17;

    let mut m = esp32s3::machine([0; 6]);
    m.console.capture = true;
    m.bus.board = make_board("m5cores3").expect("m5cores3 board should be registered");
    m.bus.attach_board_devices();

    // The touch interrupt pad (GPIO21) sits outside the S3 ADC window; the model rejects it.
    assert!(!m.bus.adc_set_raw(PIN_TOUCH_INT, 1));
    assert_eq!(m.bus.adc_observation(8).unwrap().generation, 0);

    m.bus.write32(ATTEN, 3 << 14).unwrap(); // 11 dB, the Arduino analogRead default on Port B
    m.bus.analog_set(8, AnalogSource::Const(1.650));
    m.bus.write32(CTRL, EN_PAD7).unwrap(); // arm EN_PAD with START low
    assert_eq!(m.bus.adc_observation(8).unwrap().generation, 0, "arm alone does not convert");

    m.bus.write32(CTRL, EN_PAD7 | START).unwrap();
    let meas = m.bus.read32(CTRL).unwrap();
    let expect = RtcCntl::s3_adc_code(1.650, 3);
    assert_ne!(meas & (1 << 16), 0, "DONE follows the rising START edge");
    assert_eq!(meas & 0xfff, expect);
    assert_eq!(m.bus.adc_observation(8).unwrap().raw, expect as u16);
    assert_eq!(m.bus.adc_observation(8).unwrap().generation, 1);

    // A host raw count replaces the voltage source for the next one-shot; START must
    // fall first — while it is held high the S3 keeps the last DONE/DATA pair.
    assert!(m.bus.adc_set_raw(8, 3000));
    m.bus.write32(CTRL, EN_PAD7).unwrap();
    m.bus.write32(CTRL, EN_PAD7 | START).unwrap();
    assert_eq!(m.bus.read32(CTRL).unwrap() & 0xfff, 3000);
    assert_eq!(m.bus.adc_observation(8).unwrap().generation, 2);
}
