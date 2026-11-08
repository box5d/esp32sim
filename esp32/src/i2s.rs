//! Classic I2S RX register adapter and native descriptor DMA.
//! ESP-IDF v5.5.4 components/soc/esp32/register/soc/i2s_reg.h: CONF bits 21-128,
//! interrupt _S defines 142-184, RXEOF_NUM 676-682, CONF_CHAN 692-695, INLINK_ADDR 763-768,
//! LC_CONF 856-868, clocks 1340-1390. include/soc/gpio_sig_map.h:69-72,299,309-312,343
//! supplies the GPIO matrix signals; dport_reg.h:947,965 defines the clock gates.
//! Samples are packed at DMA boundaries; no bit-level clock model is claimed.
use esp_periph::{Device, RegRam, WriteEffect};
use esp_periph::i2s::{I2s, PcmSources};
use xtensa_lx7::bus::{Bus, Fault};
use crate::bus::{SocBus, dma_range};
#[cfg(test)]
use crate::bus::{DRAM_HIGH, DRAM_LOW};
use crate::periph::CPU_HZ;
use crate::periph::ClassicGpio;

/// I2S_CONF_REG reset: RX/TX_MSB_RIGHT (bits 17/16) and RX/TX_RIGHT_FIRST (bits 9/8) set.
const CONF_RESET: u32 = (1 << 17) | (1 << 16) | (1 << 9) | (1 << 8);
/// I2S_LC_CONF_REG reset: OUT_EOF_MODE (bit 8) set, CHECK_OWNER (bit 12) clear.
const LC_CONF_RESET: u32 = 1 << 8;
/// LC_CONF CHECK_OWNER, at the bit position the shared channel's CONF1 uses too.
const CHECK_OWNER: u32 = 1 << 12;

// ESP-IDF v5.5.5 i2s_reg.h: CONF TX_START bit 4, OUT_LINK 0x30,
// EOF descriptor 0x38, LC_CONF auto-writeback bit 6, CONF2 LCD_EN bit 0.
// sens_reg.h: SAR_DAC_CTRL1 (0x98) DIG_FORCE bit 22 selects I2S-to-DAC.
// This adapter implements DMA-boundary mono DAC output, not a pin-level serializer.
#[derive(Default)]
pub(crate) struct ClassicDacTx {
    running: bool, desc: u32, rate: u32, phase: u64, credit: usize, wait_bytes: usize,
}

pub struct ClassicI2s {
    pub inner: I2s,
    ram: RegRam,
    dma: esp_periph::gdma::GdmaInCh,
    pub(crate) clock_enabled: bool,
    pub(crate) tx: ClassicDacTx,
    #[cfg(test)]
    pub(crate) step_calls: u64,
}
impl Default for ClassicI2s { fn default() -> Self { Self::new() } }
impl ClassicI2s {
    pub fn new() -> Self {
        let mut ram = RegRam::new();
        ram.write(8, CONF_RESET);
        ram.write(0x24, 64);
        ram.write(0x60, LC_CONF_RESET);
        Self { inner: I2s::new(CPU_HZ), ram, dma: esp_periph::gdma::GdmaInCh { conf1: LC_CONF_RESET & CHECK_OWNER, ..Default::default() }, clock_enabled: false, tx: ClassicDacTx::default(),
            #[cfg(test)] step_calls: 0 }
    }
    /// Writes go through `Device::write`, as the other chips' dispatch does, so the shared
    /// inherent writer keeps its single caller and S3/C3/C6 code generation is unchanged.
    fn sync(&mut self) {
        let clock = self.ram.read(0xac);
        let (a, b, n) = ((clock >> 14) & 63, (clock >> 8) & 63, clock & 255);
        let bck = self.ram.read(0xb0) & 63;
        let div = n as f64 + if a != 0 { b as f64 / a as f64 } else { 0.0 };
        self.tx.rate = if self.ram.read(0xa8) & 1 != 0 && clock & (1 << 21) == 0 && bck != 0 && div >= 2.0 {
            (160_000_000.0 / div / bck as f64).round() as u32
        } else { 0 };

        let conf = self.ram.read(8);
        let clock = self.ram.read(0xac);
        let sample = self.ram.read(0xb0);
        let bits = (sample >> 18) & 63;
        let bck = (sample >> 6) & 63;
        let (a, b) = ((clock >> 14) & 63, (clock >> 8) & 63);
        let div = if b == 0 || a < b { 0 } else { b | ((a % b) << 9) | ((a / b - 1) << 18) };
        let supported = clock & (1 << 21) == 0 && bck != 0 && [16, 24, 32].contains(&bits);
        Device::write(&mut self.inner, 0x30, (clock & 255) | (2 << 27) | if supported { 1 << 26 } else { 0 });
        Device::write(&mut self.inner, 0x38, div);
        Device::write(&mut self.inner, 0x28, (bck.saturating_sub(1) << 7) | (bits.saturating_sub(1) << 13) | ((if bits <= 16 { 15 } else { 31 }) << 18));
        // RX_CHAN_MOD 1/2 selects one slot and RX_MSB_RIGHT (CONF bit 17) decides which:
        // IDF v5.5.4 hal/esp32/include/hal/i2s_ll.h:859-875 writes RIGHT as msb_right ? 1 : 2
        // and LEFT as msb_right ? 2 : 1. The shared TDM mask has left in bit 0, right in bit 1.
        let channel = (self.ram.read(0x2c) >> 3) & 3;
        let msb_right = conf & (1 << 17) != 0;
        let mask = match channel { 1 | 2 => if (channel == 1) == msb_right { 2 } else { 1 }, _ => 3 };
        Device::write(&mut self.inner, 0x50, (1 << 16) | mask);
        // Shared RX_CONF bit 0 RX_RESET <- CONF bit 1; bit 1 RX_FIFO_RESET <- CONF bit 3 (CONF
        // bit 2 is TX_FIFO_RESET); bit 2 RX_START <- bit 5; bit 3 RX_SLAVE_MOD <- bit 7.
        // Classic keeps a 24-bit sample in a 32-bit word (i2s_ll.h:676-680 sets RX_FIFO_MOD 2;
        // esp_driver_i2s/i2s_common.c:447-448 allocates 4 bytes), so 24-bit RX sets RX_24_FILL_EN.
        let fill24 = if bits == 24 { 1 << 16 } else { 0 };
        Device::write(&mut self.inner, 0x20, ((conf >> 1) & 1) | ((conf >> 2) & 2) | ((conf >> 3) & 4) | ((conf & (1 << 7)) >> 4) | fill24);
    }
    #[inline]
    pub(crate) fn data(&mut self, cycles: u64, now: u64, port: usize, gpio: &ClassicGpio, sources: Option<&mut PcmSources>) -> Vec<u8> {
        let (data, clock) = [(155, [27, 28]), (181, [164, 165])][port];
        let signals = esp_periph::i2s::RxSignals { data, clock, input_select_bit: 7, output_mask: 0x3ff };
        self.inner.receive(cycles, now, false, &gpio.gpio, signals, sources)
    }
    pub(crate) fn dac_active(&self) -> bool {
        self.clock_enabled && self.tx.running && self.tx.rate != 0 && self.ram.read(8) & (1 << 4) != 0
    }
    /// Map the shared channel's GDMA-layout interrupt bits onto classic INT_RAW: IN_DONE 0,
    /// IN_SUC_EOF 1, IN_DSCR_ERR 3 and IN_DSCR_EMPTY 4 become bits 8, 9, 13 and 15.
    #[inline]
    pub(crate) fn raise(&mut self, gdma: u32, eof_desc: u32) {
        if gdma & (1 << 1) != 0 { self.ram.write(0x3c, eof_desc); }
        self.inner.int_raw |= ((gdma & 3) << 8) | ((gdma & (1 << 3)) << 10) | ((gdma & (1 << 4)) << 11);
    }
}
impl Device for ClassicI2s {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0xc | 0x10 | 0x14 => self.inner.read(off),
            // IDF v5.5.4 i2s_reg.h:105-128: all four reset bits are plain R/W.
            8 => self.ram.read(off),
            0xbc => u32::from(!self.dac_active()),
            _ => self.ram.read(off),
        }
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        self.ram.write(off, value);
        match off {
            0x14 | 0x18 => { Device::write(&mut self.inner, off, value); }
            0x30 => {
                if value & (1 << 28) != 0 { self.tx.running = false; }
                if value & (3 << 29) != 0 {
                    self.tx.desc = 0x3ff0_0000 | (value & 0xfffff);
                    self.tx.running = true; self.tx.phase = 0; self.tx.credit = 0; self.tx.wait_bytes = 4;
                }
            }
            0x34 => {
                if value & (1 << 28) != 0 { self.dma.running = false; }
                if value & (3 << 29) != 0 { self.dma.desc = 0x3ff0_0000 | (value & 0xfffff); self.dma.buf_pos = 0; self.dma.rx_eof_pos = 0; self.dma.running = true; }
            }
            0x60 => self.dma.conf1 = value & CHECK_OWNER,
            8 | 0x2c | 0xa8 | 0xac | 0xb0 => {
                if off == 8 && value & 5 != 0 { self.tx.phase = 0; self.tx.credit = 0; self.tx.wait_bytes = 4; }
                self.sync();
            }
            _ => {}
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 { self.inner.irq() as u64 }
}

/// Classic I2S DMA as the shared receive channel sees it: internal DRAM only. ESP-IDF v5.5.4
/// components/soc/esp32/include/soc/soc.h:203-205 (SOC_DMA_LOW/HIGH) bounds DMA-capable
/// memory to 0x3FFA_E000..0x4000_0000. Descriptor reads, buffer writes and descriptor
/// writeback anywhere else (registers, ROM, IRAM, RTC memory, flash) fault without access.
struct DramDma<'a>(&'a mut SocBus);
impl DramDma<'_> {
    fn reaches(addr: u32, len: u32) -> bool {
        dma_range(addr, len as usize).is_some()
    }
}
impl Bus for DramDma<'_> {
    fn read8(&mut self, _: u32) -> Result<u8, Fault> { Err(Fault::Prohibited) }
    fn read16(&mut self, _: u32) -> Result<u16, Fault> { Err(Fault::Prohibited) }
    fn read32(&mut self, addr: u32) -> Result<u32, Fault> {
        if !Self::reaches(addr, 4) { return Err(Fault::Prohibited); }
        self.0.dma_read_word(addr).ok_or(Fault::Unmapped)
    }
    fn write8(&mut self, addr: u32, v: u8) -> Result<(), Fault> {
        if Self::reaches(addr, 1) && self.0.dma_write_byte(addr, v) { Ok(()) } else { Err(Fault::Prohibited) }
    }
    fn write16(&mut self, _: u32, _: u16) -> Result<(), Fault> { Err(Fault::Prohibited) }
    fn write32(&mut self, addr: u32, v: u32) -> Result<(), Fault> {
        if Self::reaches(addr, 4) && self.0.dma_write_word(addr, v) { Ok(()) } else { Err(Fault::Prohibited) }
    }
    fn fetch(&mut self, _: u32) -> Result<[u8; 4], Fault> { Err(Fault::Prohibited) }
    fn note_code_page(&mut self, _: u32) {}
}

impl SocBus {
    #[inline]
    fn dac_step(&mut self, cycles: u64) {
        if !self.periph.i2s[0].dac_active() { return; }
        let tx = &mut self.periph.i2s[0].tx;
        tx.phase += cycles.saturating_mul(tx.rate as u64);
        tx.credit = tx.credit.saturating_add((tx.phase / CPU_HZ) as usize * 2);
        tx.phase %= CPU_HZ;
        if tx.credit < tx.wait_bytes { return; }
        let mut walk = esp_periph::gdma::DescriptorWalk::new(64);
        loop {
            let (addr, credit) = { let tx = &self.periph.i2s[0].tx; (tx.desc, tx.credit) };
            if credit < 4 { break; }
            let descriptor = walk.read(&mut DramDma(self), addr);
            let Ok((control, d)) = descriptor else { self.dac_fault(); break; };
            let check_owner = self.periph.i2s[0].ram.read(0x60) & CHECK_OWNER != 0;
            if d.length == 0 || d.length > d.size || d.length % 4 != 0 || d.buf % 4 != 0 || (check_owner && !d.owner_dma) {
                self.dac_fault(); break;
            }
            self.periph.i2s[0].tx.wait_bytes = d.length as usize;
            if credit < d.length as usize { break; }
            if !DramDma::reaches(d.buf, d.length) { self.dac_fault(); break; }
            let powered = self.periph.adc.rtc_io.read(0x84) & ((1 << 18) | (1 << 10)) == (1 << 18) | (1 << 10);
            let enabled = powered && self.periph.adc.read_sens(0x98) & (1 << 22) != 0;
            for offset in (0..d.length).step_by(4) {
                let Some(word) = self.dma_read_word(d.buf + offset) else { self.dac_fault(); return; };
                // ESP32 mono FIFO sends the high halfword first; DAC consumes its top 8 bits.
                for byte in [(word >> 24) as u8, (word >> 8) as u8] {
                    self.periph.i2s[0].inner.pcm.push(if enabled { (byte as i16 - 128) << 8 } else { 0 });
                }
            }
            let wrback = self.periph.i2s[0].ram.read(0x60) & (1 << 6) != 0;
            if wrback && !self.dma_write_word(addr, control & !(1 << 31)) { self.dac_fault(); break; }
            let i2s = &mut self.periph.i2s[0];
            i2s.inner.sample_rate = i2s.tx.rate;
            i2s.inner.frames_out += u64::from(d.length / 2);
            i2s.inner.int_raw |= 1 << 11;
            if d.eof { i2s.inner.int_raw |= 1 << 12; i2s.ram.write(0x38, addr); }
            i2s.ram.write(0x54, addr);
            i2s.tx.credit -= d.length as usize; i2s.tx.desc = d.next; i2s.tx.wait_bytes = 4;
            self.irq_dirty = true;
            if d.next == 0 { self.periph.i2s[0].tx.running = false; break; }
        }
    }
    fn dac_fault(&mut self) {
        self.periph.i2s[0].tx.running = false;
        self.periph.i2s[0].inner.int_raw |= 1 << 14;
        self.irq_dirty = true;
    }
    pub(crate) fn i2s_step(&mut self, cycles: u64) {
        self.dac_step(cycles);
        for port in 0..2 {
            if !self.periph.i2s[port].clock_enabled || !self.periph.i2s[port].dma.running { continue; }
            #[cfg(test)] { self.periph.i2s[port].step_calls += 1; }
            // Pin-routed sources live on controller 0, as on the other chips.
            let mut bank = self.periph.i2s[0].inner.take_pcm_bank();
            let data = self.periph.i2s[port].data(cycles, self.cycles, port, &self.periph.gpio, bank.as_deref_mut());
            self.periph.i2s[0].inner.restore_pcm_bank(bank);
            let mut dma = self.periph.i2s[port].dma;
            let mut irq = false;
            // IDF v5.5.4 hal/esp32/include/hal/i2s_ll.h:639-643 counts RXEOF_NUM in words.
            if let Some(eof_bytes) = self.periph.i2s[port].ram.read(0x24).checked_mul(4) {
                dma.receive(&mut DramDma(self), &data, Some(eof_bytes), false, |a| !DramDma::reaches(a, 1), &mut irq);
            } else {
                dma.fail_receive(&mut irq);
            }
            self.periph.i2s[port].raise(dma.int_raw, dma.eof_desc);
            dma.int_raw = 0;
            self.periph.i2s[port].dma = dma;
            self.periph.i2s[port].inner.recycle_rx_buffer(data);
            self.irq_dirty |= irq;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classic_i2s_clock_reset_and_pending_gates() {
        for (port, bit) in [4, 21].into_iter().enumerate() {
            let mut b = SocBus::new(4 << 20, [0; 6]);
            b.periph.i2s[port].dma.running = true;
            b.i2s_step(64);
            assert_eq!(b.periph.i2s[port].step_calls, 0);
            b.write32(0x3ff0_00c0, 1 << bit).unwrap();
            b.write32(0x3ff0_00c4, 1 << bit).unwrap();
            b.i2s_step(64);
            assert_eq!(b.periph.i2s[port].step_calls, 0);
            b.write32(0x3ff0_00c4, 0).unwrap();
            b.i2s_step(64);
            assert_eq!(b.periph.i2s[port].step_calls, 1);
            b.write32(0x3ff0_00c0, 0).unwrap();
            b.i2s_step(64);
            assert_eq!(b.periph.i2s[port].step_calls, 1);
            b.periph.i2s[port].dma.running = false;
            b.write32(0x3ff0_00c0, 1 << bit).unwrap();
            b.i2s_step(64);
            assert_eq!(b.periph.i2s[port].step_calls, 1);
        }
    }
    #[test]
    fn classic_i2s_dma_reach_reads_and_writes() {
        for address in [DRAM_LOW, DRAM_HIGH - 4, crate::bus::ROM_DRAM_LOW,
            DRAM_LOW - 4, DRAM_HIGH - 3, 0x4008_0000, 0x3ff8_0000, 0x5000_0000, u32::MAX] {
            let allowed = address == DRAM_LOW || address == DRAM_HIGH - 4;
            let mut b = SocBus::new(0, [0; 6]);
            let mut dma = DramDma(&mut b);
            assert_eq!(dma.read32(address).is_ok(), allowed, "read {address:#x}");
            assert_eq!(dma.write32(address, 42).is_ok(), allowed, "write {address:#x}");
        }
    }

    /// 16-bit stereo at 7843 Hz (PLL160 / 25.5 / BCK 25 / 32): one frame takes 30,601 CPU cycles.
    fn receiver(bits: u32, conf: u32) -> ClassicI2s {
        let mut i = ClassicI2s::new();
        i.write(0xac, 25 | (1 << 8) | (2 << 14));
        i.write(0xb0, (25 << 6) | (bits << 18));
        i.write(8, conf);
        i
    }
    #[test]
    fn classic_fractional_clock_width_and_slave_selection() {
        let mut i = receiver(16, 0);
        assert_eq!(i.inner.rx_rate(), Some(7843));
        i.write(0xb0, (25 << 6) | (24 << 18));
        assert_eq!(i.inner.rx_rate(), Some(3922));
        i.write(0xac, 25 | (1 << 21));
        assert_eq!(i.inner.rx_rate(), None);
        i.write(0xac, 25);
        i.write(0xb0, 16 << 18);
        assert_eq!(i.inner.rx_rate(), None);
        i.write(0xb0, (25 << 6) | (8 << 18));
        assert_eq!(i.inner.rx_rate(), None);
        i.write(0xac, 25 | (1 << 8) | (2 << 14));
        i.write(0xb0, (25 << 6) | (16 << 18));
        i.write(8, (1 << 5) | (1 << 7));
        assert!(i.inner.rx_data(40_000, false, |_| [1, 2]).is_empty());
        i.write(8, 1 << 5);
        assert_eq!(i.inner.rx_data(40_000, false, |_| [1, 2]), [1, 0, 2, 0]);
    }

    /// IDF's slot encoding (i2s_ll.h:859-875) for both RX_MSB_RIGHT values; [left, right] = [1, 2].
    #[test]
    fn rx_chan_mod_selects_slot_by_msb_right() {
        assert_eq!(ClassicI2s::new().read(8), 0x0003_0300); // i2s_reg.h:21-125 defaults
        for (msb_right, left_mod, right_mod) in [(1 << 17, 2, 1), (0, 1, 2)] {
            let mut i = receiver(16, (1 << 5) | msb_right);
            i.write(0x2c, left_mod << 3);
            assert_eq!(i.inner.rx_data(40_000, false, |_| [1, 2]), [1, 0], "left, msb_right {msb_right:#x}");
            i.write(0x2c, right_mod << 3);
            assert_eq!(i.inner.rx_data(40_000, false, |_| [1, 2]), [2, 0], "right, msb_right {msb_right:#x}");
            i.write(0x2c, 0);
            assert_eq!(i.inner.rx_data(40_000, false, |_| [1, 2]), [1, 0, 2, 0]);
        }
    }

    /// CONF bit 3 is RX_FIFO_RESET and bit 2 TX_FIFO_RESET (i2s_reg.h:105-125): only the RX
    /// resets (bits 1 and 3) discard the receiver's fractional frame time.
    #[test]
    fn rx_fifo_reset_is_conf_bit_3_not_tx_bit_2() {
        for (reset, restarts) in [(1 << 3, true), (1 << 1, true), (1 << 2, false), (1 << 0, false)] {
            let mut i = receiver(16, 1 << 5);
            assert!(i.inner.rx_data(30_000, false, |_| [1, 2]).is_empty());
            i.write(8, (1 << 5) | reset);
            assert_eq!(i.read(8), (1 << 5) | reset, "reset bit {reset:#x} remains readable");
            i.write(8, 1 << 5);
            assert_eq!(i.inner.rx_data(1_000, false, |_| [1, 2]).is_empty(), restarts, "reset bit {reset:#x}");
        }
    }

    /// Classic keeps 24-bit samples in 32-bit words, as IDF sizes them; 16/32-bit are unchanged.
    #[test]
    fn classic_24_bit_samples_fill_words() {
        for (bits, cycles, frame) in [(24, 80_000, vec![0, 0, 1, 0, 0, 0, 0xfe, 0xff]), (32, 80_000, vec![0, 0, 1, 0, 0, 0, 0xfe, 0xff]), (16, 40_000, vec![1, 0, 0xfe, 0xff])] {
            let mut i = receiver(bits, 1 << 5);
            assert_eq!(i.inner.rx_data(cycles, false, |_| [1, -2]), frame, "{bits}-bit");
        }
    }
}
