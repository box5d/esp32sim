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
    board.set_camera_picture(Picture { w: 1, h: 1, rgb: vec![255, 0, 0] });
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
    board.set_camera_picture(Picture { w: 1, h: 1, rgb: vec![0, 255, 0] });
    assert!(board.camera_frame().unwrap().2.chunks_exact(2).all(|pixel| pixel == [0x07, 0xe0]));
}
