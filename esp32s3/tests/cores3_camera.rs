use esp32s3::board::M5CoreS3;
use esp_soc::{board::BoardModel, picture::Picture};

#[test]
fn phone_picture_uses_the_format_selected_over_sccb() {
    let mut board = M5CoreS3::new();
    let mut devices = board.i2c_devices();
    let sensor = &mut devices.iter_mut().find(|(bus, addr, _)| *bus == 0 && *addr == 0x21).unwrap().2;
    let mut write = |reg, value| {
        assert!(sensor.start(false));
        assert!(sensor.write(reg));
        assert!(sensor.write(value));
    };
    board.set_camera_picture(Picture { w: 1, h: 1, rgb: vec![255, 0, 0] }).unwrap();
    let (_, _, yuv) = board.camera_frame().unwrap();
    // GC0308 OUTPUT_FMT on page 0: RGB565, as configured by CoreS3 UserDemo.
    write(0xfe, 0);
    write(0x24, 0xa6);
    let (w, h, rgb) = board.camera_frame().unwrap();
    assert_eq!((w, h), (160, 120));
    assert!(rgb.chunks_exact(2).all(|pixel| pixel == [0xf8, 0x00]));
    assert_ne!(rgb, yuv);

    // A same-address write in page 1 must not change the active format.
    write(0xfe, 1);
    write(0x24, 0xa2);
    assert_eq!(board.camera_frame().unwrap().2, rgb);
    write(0xfe, 0);
    write(0x24, 0xa2);
    assert_eq!(board.camera_frame().unwrap().2, yuv);

    write(0x24, 0xa6);
    board.set_camera_picture(Picture { w: 1, h: 1, rgb: vec![0, 255, 0] }).unwrap();
    assert!(board.camera_frame().unwrap().2.chunks_exact(2).all(|pixel| pixel == [0x07, 0xe0]));
}

const IRAM: u32 = 0x4037_0000;
const SPIN: [u8; 3] = [0x06, 0xff, 0xff];                             // j .   (objdump: ffff06)
const CAM_DESC: u32 = 0x3fc9_0000;                                    // GDMA IN chain in DRAM
const CAM_FRAME: u32 = 0x3fc9_1000;                                   // sensor frame landing buffer
const CAM_LINES: u32 = 120;                                           // QVGA = 160x120 RGB565
const CAM_LINE: u32 = 320;                                            // 160 px * 2 bytes
const CAM_PERIOD: u64 = 20_480;                                       // 320 quanta of 64 cycles

#[test]
fn cores3_gc0308_streams_chunked_rgb565_lines_through_the_dma_pipeline() {
    use emu_core::Bus;
    use esp_soc::{board::BoardModel, Stop};

    let mut m = esp32s3::machine([0; 6]);
    m.console.capture = true;
    m.quantum = 64;

    // SCCB: CoreS3 UserDemo configures the GC0308 OUTPUT_FMT (page 0, register 0x24)
    // to 0xa6 - RGB565. The snapshot the DMA pipeline sees is read from the board itself.
    let mut board = M5CoreS3::new();
    let mut devices = board.i2c_devices();
    let sensor = &mut devices.iter_mut().find(|(bus, addr, _)| *bus == 0 && *addr == 0x21).unwrap().2;
    for (reg, value) in [(0xfe, 0u8), (0x24, 0xa6)] {
        assert!(sensor.start(false));
        assert!(sensor.write(reg));
        assert!(sensor.write(value));
    }
    drop(devices);
    let expected = board.camera_frame().expect("GC0308 frame after the SCCB format switch");
    assert_eq!(expected.0, 160);
    assert_eq!(expected.1, 120);
    let expected_bytes = expected.2;

    m.bus.board = Box::new(board);

    // Phase 1 - idle VSYNC. The M5Stack cam_task arms capture after CAM_VSYNC_INT, so the
    // upstream pipeline must publish the GC0308 frame and raise the interrupt before CAM_START.
    let cam = &mut m.bus.periph.lcd_cam;
    cam.set_clock_enabled(true);
    cam.frame_cycles = CAM_PERIOD;
    cam.write(0x04, 1 << 29);            // sensor side of CAM_CTRL (BIT_ORDER/VS_EOF_EN stay off)
    cam.write(0x64, 1 << 2);             // CAM_VSYNC_INT enable
    esp_soc::SocBus::load_bytes(&mut m.bus, IRAM, &SPIN).unwrap();
    m.cores[0].pc = IRAM;
    m.cores[0].ps = 0;
    m.max_cycles = m.bus.cycles + CAM_PERIOD;
    assert!(matches!(m.run(u64::MAX), Stop::Halted));
    m.bus.flush_ticks(); // halt stops with deferred ticks: deliver them like a guest read would
    {
        let cam = &m.bus.periph.lcd_cam;
        assert_ne!(cam.int_raw & (1 << 2), 0, "GC0308 idle VSYNC reaches CAM_VSYNC_INT");
        assert!(cam.cam_frame().is_some(), "the pipeline holds a frame for the armed capture");
    }

    // Phase 2 - chunked DMA. The legacy CoreS3 fork tracked CAM_SUC_EOF chunks with `cam_pos`
    // counters; upstream v0.3.0 drives the same ongoing-shape through GDMA IN descriptors:
    // one 320-byte RGB565 line per descriptor, REC_DATA_BYTELEN = line-1, VS_EOF_EN off,
    // so every line closes with its own SUC_EOF and the frame reassembles in order.
    for line in 0..CAM_LINES {
        let d = CAM_DESC + line * 12;
        let next = if line + 1 < CAM_LINES { CAM_DESC + (line + 1) * 12 } else { 0 };
        m.bus.write32(d, (1 << 31) | CAM_LINE).unwrap();
        m.bus.write32(d + 4, CAM_FRAME + line * CAM_LINE).unwrap();
        m.bus.write32(d + 8, next).unwrap();
    }
    m.bus.periph.gdma.write(0x48, 5);    // GDMA IN channel 0 <- LCD_CAM camera (trigger 5)
    m.bus.periph.gdma.write(0x20, (CAM_DESC & 0xfffff) | (1 << 22)); // INLINK START
    let cam = &mut m.bus.periph.lcd_cam;
    cam.write(0x70, u32::MAX);           // clear the phase-1 raw bits
    cam.write(0x08, (1 << 29) | (CAM_LINE - 1)); // CAM_START + REC_DATA_BYTELEN = one line

    m.max_cycles = m.bus.cycles + 4 * CAM_PERIOD;
    assert!(matches!(m.run(u64::MAX), Stop::Halted));
    m.bus.flush_ticks();

    let cam = &m.bus.periph.lcd_cam;
    assert_ne!(cam.int_raw & (1 << 2), 0, "VSYNC accompanies the streamed frames");
    assert!(cam.frames >= 2, "{} VSYNC frames for idle + capture phases", cam.frames);
    // The test does not re-arm descriptor chain per frame (firmware does that in its
    // SUC_EOF callback), so periods after the first streamed frame legitimately drop.
    let in0 = &m.bus.periph.gdma.inp[0];
    assert_eq!(in0.int_raw & 3, 3, "every chunk closed with IN_DONE and IN_SUC_EOF");
    assert_eq!(in0.eof_desc, CAM_DESC + (CAM_LINES - 1) * 12, "the final line owns the last SUC_EOF");
    let got: Vec<u8> = (0..CAM_LINES * CAM_LINE).map(|i| m.bus.read8(CAM_FRAME + i).unwrap()).collect();
    assert_eq!(got.as_slice(), &expected_bytes[..], "chunked DMA reassembles the GC0308 RGB565 frame");
}
