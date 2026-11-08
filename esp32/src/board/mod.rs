//! Boards wired for the classic ESP32, independent of S3 pin assignments.
//!
//! - `TtgoTDisplay` — the LilyGO TTGO T-Display: an ST7789 135×240 on VSPI, two buttons
//! - `NoBoard` — a bare module (`none`, `bare`, `esp32dev`): nothing on the pins, console only
//! - `esp32dev-loopback` — the SPI loopback fixture in `crate::spi`
pub mod tdisplay;
pub mod m5stack;

pub use tdisplay::*;

pub fn make_board(name: &str) -> Option<esp_soc::Board> {
    match name {
        "none" | "bare" | "esp32dev" => Some(Box::new(esp_soc::NoBoard)),
        "ttgo-tdisplay" | "tdisplay" => Some(Box::new(TtgoTDisplay::new())),
        "m5basic" | "m5stack-basic" => Some(Box::new(m5stack::M5Stack::new(m5stack::Model::Basic))),
        "m5fire" | "m5stack-fire" => Some(Box::new(m5stack::M5Stack::new(m5stack::Model::Fire))),
        "m5core2" | "m5stack-core2" => Some(Box::new(m5stack::M5Stack::new(m5stack::Model::Core2))),
        _ => crate::spi::make_board(name),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn classic_boards_reject_s3_names() {
        for name in ["none", "bare", "esp32dev", "ttgo-tdisplay", "tdisplay"] { assert!(super::make_board(name).is_some()); }
        for name in ["waveshare-amoled18-v2", "waveshare-lcd4b", "atech14", "waveshare-c6-lcd147"] { assert!(super::make_board(name).is_none()); }
    }
}
