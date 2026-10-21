use esp32c6::periph::{OptionalPeripherals, Peripherals};

#[test]
fn optional_devices_preserve_the_existing_i2s_indirection() {
    assert_eq!(std::mem::offset_of!(OptionalPeripherals, i2s0), 0);
}

#[test]
fn cold_devices_do_not_grow_the_native_hot_bus() {
    use std::mem::{size_of, size_of_val};

    // Guard the actual cold field, not a snapshot of unrelated hot-bus fields.
    // Inlining OptionalPeripherals must fail this on every architecture, including
    // both Linux CI targets. Adding other Peripherals/SocBus fields is harmless.
    let periph = Peripherals::new([0; 6]);
    assert_eq!(size_of_val(&periph.optional), size_of::<usize>());
    assert_eq!(size_of::<Option<Box<OptionalPeripherals>>>(), size_of::<usize>());
    assert!(size_of::<OptionalPeripherals>() > size_of_val(&periph.optional));
}
