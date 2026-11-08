//! Classic ESP32 timer-group layout: T0/T1 and LACT.
//! ESP-IDF v5.5.4 components/soc/esp32/register/soc/timer_group_reg.h:
//! T0 edge/level bits 12/11 (42-53), LACT enable/direction/reload/divider (397-420),
//! alarm enable bit 10 (433-438). Timing and reset completion are inferred.
use emu_core::ClockDomain;
use esp_periph::{Device, TimerGroup, WriteEffect};
use esp_periph::timg::Timer;

pub struct ClassicTimer {
    pub timer: TimerGroup,
    lact: Timer,
    lact_rtc: u32,
}

impl ClassicTimer {
    pub fn new(_group: usize) -> Self {
        Self { timer: TimerGroup::new(), lact: Timer::default(), lact_rtc: 0 }
    }

    fn irq_bits(&self) -> u64 {
        let pending = self.timer.int_raw & self.timer.int_ena;
        let mut bits = 0;
        for n in 0..2 {
            if pending & (1 << n) != 0 {
                let config = self.timer.t[n].config;
                if config & (1 << 11) != 0 {
                    bits |= 1 << n;
                }
                if config & (1 << 12) != 0 {
                    bits |= 1 << (4 + n);
                }
            }
        }
        if pending & (1 << 3) != 0 {
            if self.lact.config & (1 << 11) != 0 {
                bits |= 1 << 3;
            }
            if self.lact.config & (1 << 12) != 0 {
                bits |= 1 << 7;
            }
        }
        bits
    }
}

impl Device for ClassicTimer {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x00..=0x47 => self.timer.t[(off / 0x24) as usize].read(off % 0x24),
            0x70 => self.lact.config,
            0x74 => self.lact_rtc,
            0x78..=0x94 => self.lact.read(off - 0x74),
            0x98 => self.timer.int_ena,
            0x9c => self.timer.int_raw,
            0xa0 => self.timer.int_raw & self.timer.int_ena,
            _ => self.timer.read(off),
        }
    }

    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0x00..=0x47 => self.timer.t[(off / 0x24) as usize].write(off % 0x24, value, u64::MAX),
            0x70 => self.lact.config = value,
            0x74 => self.lact_rtc = value,
            0x78..=0x94 => self.lact.write(off - 0x74, value, u64::MAX),
            0x98 => self.timer.int_ena = value,
            0xa4 => self.timer.int_raw &= !value,
            _ => self.timer.write(off, value),
        }
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 {
        self.irq_bits()
    }

    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::Apb)
    }

    fn tick(&mut self, ticks: u64) {
        let TimerGroup { t, int_raw, .. } = &mut self.timer;
        for (i, timer) in t.iter_mut().enumerate() {
            timer.step(ticks, u64::MAX, || *int_raw |= 1 << i);
        }
        self.lact.step(ticks, u64::MAX, || *int_raw |= 1 << 3);
    }

    fn has_deadline(&self) -> bool {
        true
    }

    fn next_deadline(&self) -> Option<u64> {
        // Timer::deadline saturates; the 64-bit counters can overflow TimerGroup's 54-bit product.
        self.timer.t.iter().chain([&self.lact]).filter_map(Timer::deadline).min()
    }
}

/// Classic ESP32 FRC_TIMER block (page 0x47): two legacy free-running counters.
/// ESP-IDF v3.3.5 components/soc/esp32/include/soc/frc_timer_reg.h: timer i at
/// i*0x20 + LOAD 0x0 / COUNT 0x4 / CTRL 0x8 / INT 0xC / ALARM 0x10 (FRC2 only).
/// CTRL: INT_STATUS bit 8 (RO), ENABLE bit 7, AUTOLOAD bit 6, PRESCALER bits
/// 3..1 (field 0/2/4 = /1, /16, /256), LEVEL_INT bit 0. FRC1 is 23-bit with its
/// alarm fixed at the max count; FRC2 is 32-bit with a programmable ALARM.
/// Pre-IDF4 startup code (the PHY RF calibration in the 2019 factory images)
/// polls FRC2 COUNT, so the counters must genuinely advance on APB ticks.
/// No interrupt source is wired: known guests poll COUNT or INT_STATUS.
pub struct ClassicFrc {
    t: [FrcTimer; 2],
}

#[derive(Default, Clone, Copy)]
struct FrcTimer {
    load: u32,
    count: u32,
    ctrl: u32,
    alarm: u32,
    int_status: bool,
    holding: bool,
    prescale_acc: u64,
}

impl FrcTimer {
    fn mask(index: usize) -> u32 {
        if index == 0 { 0x007f_ffff } else { u32::MAX }
    }
    fn target(&self, index: usize) -> u32 {
        if index == 0 { Self::mask(0) } else { self.alarm }
    }
    fn divider(&self) -> u64 {
        let field = (self.ctrl >> 1) & 7;
        1u64 << (2 * field)
    }
    fn enabled(&self) -> bool {
        self.ctrl & (1 << 7) != 0
    }
    fn autoload(&self) -> bool {
        self.ctrl & (1 << 6) != 0
    }
    /// Counter steps from the current count up to (and including) the target,
    /// wrapping at the counter width.
    fn gap(&self, index: usize) -> u64 {
        let target = self.target(index) as u64;
        let count = self.count as u64;
        if count <= target { target - count } else { (Self::mask(index) as u64 - count) + target + 1 }
    }
    fn step(&mut self, index: usize, apb_ticks: u64) {
        if !self.enabled() || self.holding {
            return;
        }
        let div = self.divider();
        self.prescale_acc += apb_ticks;
        let mut steps = self.prescale_acc / div;
        self.prescale_acc %= div;
        let mask = Self::mask(index);
        while steps > 0 {
            let gap = self.gap(index);
            if gap == 0 {
                // Sitting on the target (landed with AUTOLOAD): reload, unless
                // the reload value is the target itself.
                if !self.autoload() {
                    self.holding = true;
                    break;
                }
                self.count = self.load & mask;
                if self.gap(index) == 0 {
                    break;
                }
                continue;
            }
            if steps <= gap {
                self.count = ((self.count as u64 + steps) & mask as u64) as u32;
                if steps == gap {
                    self.int_status = true;
                    if !self.autoload() {
                        self.holding = true;
                    }
                }
                break;
            }
            steps -= gap;
            self.int_status = true;
            if self.autoload() {
                self.count = self.load & mask;
                if self.gap(index) == 0 {
                    break;
                }
            } else {
                self.count = self.target(index);
                self.holding = true;
                break;
            }
        }
    }
    fn steps_to_alarm(&self, index: usize) -> Option<u64> {
        if !self.enabled() || self.holding {
            return None;
        }
        Some(self.gap(index).saturating_mul(self.divider()).saturating_sub(self.prescale_acc))
    }
}

impl ClassicFrc {
    pub fn new() -> Self {
        Self { t: [FrcTimer::default(); 2] }
    }
}

impl Device for ClassicFrc {
    fn read(&mut self, off: u32) -> u32 {
        let index = (off / 0x20) as usize;
        let t = &self.t[index];
        match off % 0x20 {
            0x0 => t.load,
            0x4 => t.count,
            0x8 => t.ctrl | ((t.int_status as u32) << 8),
            0x10 if index == 1 => t.alarm,
            _ => 0,
        }
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        let index = (off / 0x20) as usize;
        let t = &mut self.t[index];
        match off % 0x20 {
            0x0 => t.load = v & FrcTimer::mask(index),
            0x8 => {
                let was_enabled = t.enabled();
                t.ctrl = v & !(1 << 8);
                if t.enabled() && !was_enabled {
                    t.count = t.load;
                    t.int_status = false;
                    t.holding = false;
                    t.prescale_acc = 0;
                }
            }
            0xC => {
                if v & 1 != 0 {
                    t.int_status = false;
                }
            }
            0x10 if index == 1 => t.alarm = v,
            _ => {}
        }
        WriteEffect::NONE
    }

    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::Apb)
    }

    fn tick(&mut self, apb_ticks: u64) {
        for i in 0..2 {
            self.t[i].step(i, apb_ticks);
        }
    }

    fn has_deadline(&self) -> bool {
        true
    }

    fn next_deadline(&self) -> Option<u64> {
        (0..2).filter_map(|i| self.t[i].steps_to_alarm(i)).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_classic_timers_keep_full_width_and_share_alarm_semantics() {
        for (config, load, trigger, latch, count, alarm) in [
            (0, 0x18, 0x20, 0xc, 4, 0x10),
            (0x24, 0x3c, 0x44, 0x30, 0x28, 0x34),
            (0x70, 0x8c, 0x94, 0x80, 0x78, 0x84),
        ] {
            let mut t = ClassicTimer::new(0);
            t.write(load, u32::MAX - 1);
            t.write(load + 4, 0x80000000);
            t.write(trigger, 1);
            t.write(alarm, 1);
            t.write(alarm + 4, 0x80000001);
            t.write(config, (1 << 31) | (1 << 30) | (1 << 10) | (2 << 13));
            assert_eq!(t.next_deadline(), Some(6));
            t.tick(6);
            t.write(latch, 1);
            assert_eq!(t.read(count), 1);
            assert_eq!(t.read(count + 4), 0x80000001);
            assert_ne!(t.read(0x9c), 0);
            assert_eq!(t.next_deadline(), None);
        }
    }

    #[test]
    fn frc2_counts_at_apb_rate_and_alarms() {
        let mut frc = ClassicFrc::new();
        // FRC2: LOAD=0, ALARM=1000, CTRL = ENABLE | LEVEL_INT (prescaler /1),
        // INT clear — the exact sequence the 2019 factory images issue.
        frc.write(0x30, 1000);
        frc.write(0x20, 0);
        frc.write(0x28, 0x81);
        frc.write(0x2c, 1);
        frc.tick(400);
        assert_eq!(frc.read(0x24), 400);
        assert_eq!(frc.read(0x28) & (1 << 8), 0);
        frc.tick(600);
        assert_eq!(frc.read(0x24), 1000);
        assert_ne!(frc.read(0x28) & (1 << 8), 0, "INT_STATUS must latch at the alarm");
        frc.tick(5000);
        assert_eq!(frc.read(0x24), 1000, "without AUTOLOAD the count holds at the alarm");
        frc.write(0x2c, 1);
        assert_eq!(frc.read(0x28) & (1 << 8), 0, "INT_CLR clears the status");
    }

    #[test]
    fn frc_autoload_and_prescaler() {
        let mut frc = ClassicFrc::new();
        frc.write(0x30, 100);
        frc.write(0x20, 10);
        frc.write(0x28, (1 << 7) | (1 << 6) | (2 << 1)); // ENABLE | AUTOLOAD | /16
        frc.tick(16 * 95);
        // From LOAD=10, 90 steps reach the alarm (reload to 10), then 5 more.
        assert_eq!(frc.read(0x24), 15);
        assert_ne!(frc.read(0x28) & (1 << 8), 0);
    }

    #[test]
    fn frc1_is_23_bit_with_alarm_at_max() {
        let mut frc = ClassicFrc::new();
        frc.write(0x00, 0x007f_fffe);
        frc.write(0x08, 1 << 7);
        frc.tick(1);
        assert_eq!(frc.read(0x04), 0x007f_ffff);
        assert_ne!(frc.read(0x08) & (1 << 8), 0);
    }

    #[test]
    fn far_alarms_saturate_instead_of_wrapping() {
        // Divider field 0 means 65536; 2^64 - 1 steps times that overflows u64.
        let mut t = ClassicTimer::new(0);
        for base in [0x00, 0x24, 0x74] {
            let config = if base == 0x74 { 0x70 } else { base };
            t.write(base + 0x10, u32::MAX);
            t.write(base + 0x14, u32::MAX);
            t.write(config, (1 << 31) | (1 << 30) | (1 << 10));
            assert_eq!(t.next_deadline(), Some(u64::MAX), "timer at {base:#x}");
            t.write(config, 0);
        }
    }
}
