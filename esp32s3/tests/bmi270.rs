use esp32s3::i2c::{Bmi270, I2cDevice, ImuState};
use std::sync::{Arc, Mutex};

#[test]
fn official_demo_data_ready_gate_reads_accelerometer() {
    let state = Arc::new(Mutex::new(ImuState::default()));
    let mut sensor = Bmi270::new(state.clone());
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
