use esp32s3::i2c::{Bmi270, I2cDevice, ImuMotion, ImuState};
use std::sync::{Arc, Mutex};

#[test]
fn official_demo_data_ready_gate_reads_accelerometer() {
    let state = Arc::new(Mutex::new(ImuState::default()));
    let mut sensor = Bmi270::new(Arc::new(ImuMotion::default()), state.clone());
    sensor.start(false);
    sensor.write(0x1c);
    sensor.start(true);
    let status = u16::from_le_bytes([sensor.read(), sensor.read()]);
    // Bosch BMI2 interrupt masks, also used by CoreS3-UserDemo AppIMUModel::Update.
    assert_ne!(status & 0x2000, 0, "AUX readiness gates the entire official demo update");
    assert_eq!(status & 0xc000, 0xc000, "accelerometer and gyro data ready");
    for (ax, expected) in [(0.0, 0_i16), (9.80665, 16384_i16)] {
        state.lock().unwrap().ax = ax;
        sensor.start(false);
        sensor.write(0x0c);
        sensor.start(true);
        assert_eq!(i16::from_le_bytes([sensor.read(), sensor.read()]), expected);
    }
}

/// A scripted motion replaces the browser stream: the BMI270 quantizes the same handling
/// script the QMI8658 plays (2 g range: 16384 LSB/g, 2000 dps: 16.384 LSB/dps).
#[test]
fn scripted_motion_latches_sensor_output() {
    let motion = Arc::new(ImuMotion::default());
    motion.mode.store(1, std::sync::atomic::Ordering::Relaxed);
    motion
        .cycle
        .store(esp32s3::periph::CPU_HZ, std::sync::atomic::Ordering::Relaxed);
    let mut sensor = Bmi270::new(motion, Arc::default());
    sensor.start(false);
    sensor.write(0x0c);
    sensor.start(true);
    let axis = i16::from_le_bytes([sensor.read(), sensor.read()]);
    let (accel, _) = esp32s3::i2c::motion_script(1.0);
    let quantize = |v: f64| {
        let r = if v < 0.0 { v - 0.5 } else { v + 0.5 };
        r.clamp(-32768.0, 32767.0) as i32 as i16
    };
    assert_eq!(axis, quantize(accel[0] * 16384.0), "accel X follows the script");
}
