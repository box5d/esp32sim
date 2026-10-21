use esp_soc::SocBus as _;
use emu_core::bus::Bus as _;

#[test]
fn clock_fraction_and_lact_follow_frequency_changes() {
    let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    bus.write32(0x3ff4_8070, 1 << 27).unwrap();
    bus.write32(0x3ff0_003c, 1).unwrap();
    bus.tick(1);
    assert_eq!(bus.cycles, 1);
    bus.tick(1);
    assert_eq!(bus.cycles, 3, "160 MHz half ticks must not be discarded per call");
    bus.write32(0x3ff0_003c, 2).unwrap();
    bus.tick(2);
    assert_eq!(bus.cycles, 5);
    bus.write32(0x3ff4_8070, 0).unwrap();
    bus.tick(2);
    assert_eq!(bus.cycles, 17, "XTAL selection overrides PLL divider");

    for (period, cycles) in [(0, 80_000), (1, 160_000), (2, 240_000)] {
        let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
        bus.write32(0x3ff4_8070, 1 << 27).unwrap();
        bus.write32(0x3ff0_003c, period).unwrap();
        bus.write32(0x3ff5_f070, (1 << 31) | (1 << 30) | (40 << 13)).unwrap();
        bus.tick(cycles);
        bus.write32(0x3ff5_f080, 1).unwrap();
        assert_eq!(bus.read32(0x3ff5_f078).unwrap(), 2_000, "LACT/esp_timer has advanced 1 ms");
        assert_eq!(bus.cycles, 240_000);
    }
}

#[test]
fn xtal_apb_lact_prescaler_counts_real_milliseconds() {
    let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    for (source, cpu_cycles, divider) in [(0, 40_000, 20), (1, 240_000, 40), (0, 40_000, 20)] {
        bus.write32(0x3ff4_8070, source << 27).unwrap();
        bus.write32(0x3ff0_003c, 2).unwrap();
        bus.write32(0x3ff5_f070, (1 << 31) | (1 << 30) | (divider << 13)).unwrap();
        bus.write32(0x3ff5_f080, 1).unwrap();
        let before = bus.read32(0x3ff5_f078).unwrap();
        bus.tick(cpu_cycles);
        bus.write32(0x3ff5_f080, 1).unwrap();
        assert_eq!(bus.read32(0x3ff5_f078).unwrap() - before, 2_000, "source={source}");
    }
}

#[test]
fn clock_conversion_bounds_stop_and_peripheral_deadlines() {
    for code in [&[0x06, 0xff, 0xff][..], &[0x00, 0x70, 0x00, 0x06, 0xff, 0xff][..]] {
        let mut m = esp32::machine([0; 6], 4 << 20);
        m.bus.load_bytes(0x4008_0000, code).unwrap();
        m.cores[0].pc = 0x4008_0000;
        m.cores[0].ps = 0;
        m.quantum = 1;
        m.bus.write32(0x3ff0_003c, 0).unwrap();
        m.bus.write32(0x3ff4_8070, 1 << 27).unwrap();
        m.max_cycles = 9;
        assert!(matches!(m.run(100), esp32::Stop::Halted));
        assert_eq!((m.bus.cycles, m.cores[0].ccount), (9, 3));
    }

    let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    bus.write32(0x3ff0_003c, 1).unwrap();
    bus.write32(0x3ff4_8070, 1 << 27).unwrap();
    bus.write32(0x3ff5_f000, (1 << 31) | (1 << 30) | (1 << 11) | (1 << 10) | (2 << 13)).unwrap();
    bus.write32(0x3ff5_f010, 10).unwrap();
    bus.write32(0x3ff5_f098, 1).unwrap();
    // The shared device clock returns a conservative deadline (before the alarm).
    let reference = u64::from(bus.periph.cycles_until_timer());
    assert_eq!(bus.next_deadline(), Some((reference * 2).div_ceil(3)));
    bus.tick(40);
    assert_ne!(bus.read32(0x3ff5_f09c).unwrap() & 1, 0);
}

#[test]
fn selected_cpu_clock_scales_instruction_and_idle_time() {
    for (source, period, master_cycles) in [(0, 2, 1200), (1, 0, 600), (1, 1, 300), (1, 2, 200)] {
        for code in [&[0x06, 0xff, 0xff][..], &[0x00, 0x70, 0x00, 0x06, 0xff, 0xff][..]] {
            let mut m = esp32::machine([0; 6], 4 << 20);
            m.bus.load_bytes(0x4008_0000, code).unwrap();
            m.cores[0].pc = 0x4008_0000;
            m.cores[0].ps = 0;
            m.quantum = 2;
            m.bus.write32(0x3ff0_003c, period).unwrap();
            m.bus.write32(0x3ff4_8070, source << 27).unwrap();
            assert!(matches!(m.run(200), esp32::Stop::MaxInsns));
            assert_eq!(m.cores[0].ccount, 200);
            assert_eq!(m.bus.cycles, master_cycles, "source={source}, period={period}");
        }
    }
}

#[test]
fn wakeup_state_cannot_alias_reset_cause() {
    let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    bus.set_reset_cause(12);
    assert_eq!(bus.read32(0x3ff4_8038).unwrap(), 12 << 11);
    bus.write32(0x3ff4_8038, u32::MAX).unwrap();
    assert_eq!(bus.read32(0x3ff4_8034).unwrap(), 12 | (12 << 6));
    assert_eq!(bus.read32(0x3ff4_8038).unwrap(), 0xfff << 11);
}

#[test]
fn gpio23_is_visible_but_unbonded_or_invalid_pins_are_hidden() {
    let bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    assert!(bus.gpio_state(23).is_some());
    for pin in [20, 24, 28, 29, 30, 31, 40, 255] { assert!(bus.gpio_state(pin).is_none()); }
}

#[test]
fn reset_disables_classic_rwdt_without_clobbering_stage3() {
    let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    bus.write32(0x3ff4_80a4, 0x50d8_3aa1).unwrap();
    bus.write32(0x3ff4_8098, 0x1234).unwrap();
    bus.write32(0x3ff4_808c, (1 << 31) | (3 << 28)).unwrap();
    bus.request_reset(12);
    bus.reboot([0; 6]);
    assert_eq!(bus.read32(0x3ff4_808c).unwrap(), 0);
    assert_eq!(bus.read32(0x3ff4_8098).unwrap(), 0x1234);
}

#[test]
fn reset_keeps_both_flash_jedec_ids() {
    let mut bus = esp32::bus::SocBus::new(16 << 20, [0; 6]);
    bus.set_flash_size(16 << 20);
    let ids = [bus.periph.spi0.0.jedec, bus.periph.spi1.0.jedec];
    assert_eq!(ids[0][2], 24);
    for _ in 0..3 {
        bus.request_reset(12);
        bus.reboot([0; 6]);
        assert_eq!([bus.periph.spi0.0.jedec, bus.periph.spi1.0.jedec], ids);
    }
}

/// IDF v5.5.4 soc/esp32/register/soc/spi_reg.h: CTRL2 is +0x14 and CLOCK +0x18 on the classic SPI1, where the
/// shared SPI_MEM layout keeps CLOCK (+0x14) and USER (+0x18); a write to the classic CLOCK must not land in USER.
#[test]
fn spi1_ctrl2_and_clock_do_not_alias_the_shared_user_and_clock_slots() {
    let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    bus.write32(0x3ff4_2014, 0x0000_1111).unwrap();
    bus.write32(0x3ff4_2018, 0x0000_2222).unwrap();
    bus.write32(0x3ff4_201c, 0xa000_0000).unwrap();
    assert_eq!(bus.read32(0x3ff4_2014).unwrap(), 0x0000_1111, "CTRL2");
    assert_eq!(bus.read32(0x3ff4_2018).unwrap(), 0x0000_2222, "CLOCK");
    assert_eq!(bus.read32(0x3ff4_201c).unwrap(), 0xa000_0000, "USER");
}
