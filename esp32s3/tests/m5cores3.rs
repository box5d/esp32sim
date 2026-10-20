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
