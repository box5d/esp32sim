# EX226: classic Wi-Fi on StationLink

Base: PR #208, branch `esp32-classic-4-crypto-bb`, `schematik-engineering/esp32sim@c60e7a647ed1df1167bbb3c34141e6acdc471ab5`, on upstream main `bb2b33d5`.

The classic eFuse MAC words carry the ROM CRC8 of the factory MAC, so IDF's
MAC CRC check passes (`classic_factory_mac_includes_rom_crc8`).

`next_deadline` covers timers and board deadlines but not Wi-Fi due times, the
same as S3/C3/C6: while the CPU idles, a due frame waits for the next timer
deadline (about 1 ms with the FreeRTOS tick). A shared StationLink due-time API
for all four chips is left for a separate change. Descriptor decoding reuses
the checked reader from EX224. No fourth station implementation is added.

## Sources and limits

ESP-IDF v5.5.4 `components/soc/esp32/register/soc/dport_reg.h:1031-1043`
defines the clock reset value and Wi-Fi enable mask; lines 1064 and 1069 define
`DPORT_CORE_RST_EN_REG` bit 2, `DPORT_WIFIMAC_RST`. MAC offsets, PHY
completion and RX metadata are inferred from the Arduino-ESP32 3.3.8 libraries;
no public register-header or hardware validation is claimed for these fields.
PHY calibration completes without RF arithmetic. No RF timing claim, PSRAM
DMA, multi-descriptor Wi-Fi frames or classic firmware networking validation
is claimed by these register-level tests.

An inline DPORT gate guards Wi-Fi service; there is no separate Cargo feature.
AHB aliases normalize before the shared MMIO path, including UART/I2C FIFO
notifications and RNG reads. TX borrows DRAM without copying the frame.
The shared Regi2c helper retains each chip's analog defaults and host selection
and is inlined, so each chip keeps its own map code;
classic adds only its inferred poll-completion fields. Existing chips gain no
fields or per-tick work. New classic fields are appended.

## Verification

Use the full [EX230 command set](../classic-core/README.md) on Rust 1.99.0.
The inherited ROM/firmware hashes are in that receipt. No golden changes.
JIT implementation is unchanged.

[Mutation table](mutations.json) records 27 killed single replacements and the tests
that fail. Run each with `cargo +1.99.0 test -p CRATE --lib TEST`, restoring
the original source between mutations. Tests cover register aliases, TSF,
clock/reset, DMA rejection, RX metadata, shared station pacing and reboot.



## CPU comparison

### Classic hello: measured pre-rebase revisions

Branch `esp32-classic-5-wifi` at
`schematik-engineering/esp32sim@ca069cf2b6a50647bdcc414c9ac53c01e50f08e3`
against parent #208, branch `esp32-classic-4-crypto` at
`schematik-engineering/esp32sim@cb5d1fa6440cedb46e769ab372cb1a2130296b4b`.
These measurements used the old classic clock model. They identify the old
revisions, not the bb2b33d5 rebase or corrected-clock tips.
Rust 1.99.0 release binaries; child user CPU seconds including startup; one warmup
and seven alternating pairs, load below 5 before each attempt and peak below 7.
Console SHA-256 and instruction counts matched across attempts.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Classic hello | 2.5060 (2.3622–2.7359) | 2.4150 (2.3839–2.7382) | -3.63% | 2/7 | 34160592 | 5.28 |

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.562813 | 2.446447 | 4.91/4.91/4.91 | 4.82/4.91/4.91 | accepted |
| 1 | 2 | M→B | 2.689331 | 2.738203 | 4.91/4.92/4.92 | 4.92/4.92/4.92 | accepted |
| 2 | 3 | B→M | 2.735878 | 2.531034 | 5.00/5.00/5.00 | 4.92/5.00/5.00 | accepted |
| 3 | 4 | M→B | 2.408043 | 2.394516 | 4.96/5.28/5.28 | 5.28/5.28/5.28 | accepted |
| 4 | 5 | B→M | 2.428351 | 2.415029 | 4.93/4.93/4.93 | 4.92/4.93/4.93 | accepted |
| 5 | 6 | M→B | 2.362180 | 2.383858 | 4.93/5.02/5.02 | 5.02/5.02/5.02 | accepted |
| 6 | 7 | B→M | 2.506022 | 2.449845 | 4.65/4.65/4.65 | 4.80/4.65/4.80 | accepted |
| 7 | 8 | M→B | 2.552290 | 2.384733 | 4.65/4.60/4.65 | 4.60/4.60/4.60 | accepted |

Raw outputs are not committed; the accepted pair samples are above.

The non-classic workloads were not measured separately. The stack tip, PR #212 (schematik-engineering/esp32sim@bcd4df7e, branch `esp32-classic-host`, which contains this change), was measured against main `2f9443a9` with the method in the classic-host receipt (user CPU, one warmup and seven alternating load-gated pairs, identical instruction counts and console hashes):

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.2784 (3.0442–3.3351) | 3.2612 (3.0412–3.3331) | -0.52% | 2/7 | 4800000000 | 5.27 |
| Pocket Tank | 35.2968 (34.6376–38.3461) | 35.0204 (34.7165–38.7101) | -0.78% | 3/7 | 10073833665 | 4.26 |
| S3 hello | 24.6510 (23.7365–32.0593) | 24.3939 (23.8923–27.6721) | -1.04% | 4/7 | 1789819657 | 4.96 |
| C3 hello | 2.3837 (2.3635–2.5350) | 2.4178 (2.3742–2.5332) | +1.43% | 5/7 | 4800000000 | 3.53 |

That measurement predates the Oct 11 review commits and the rebase onto main `eec7b2c9`. Those commits add no S3/C3/C6 execution-path instructions; the static check below was repeated on the rebased stack tip (#211).

Static check (no timing): release `esp32sim` binaries from main `eec7b2c9` and
the rebased stack tip were disassembled with `llvm-objdump -d` and compared function by
function after normalising addresses, alignment `nop`s, adrp page offsets and
linker-chosen symbol aliases. The S3/C3/C6 execution path is instruction-identical
to main: `Machine::run`, `step_core`, each `SocBus` `tick` and `next_deadline`, S3 `tick_impl`,
`refresh_tick_budget` and GDMA engines, the MMIO dispatchers, `TimerGroup::tick`, and the UART,
TIMG, SHA, LEDC and RMT tick and register paths, and RTC_CNTL. Remaining differences: shared `Gpio::write` (`ubfx` for `lsr`, inherited from EX230); `I2c::write`, whose bounds-check panic calls share one source location in the macro
expansion, so three merge and the END arm moves; `Ledc::output`, the host PWM query, and the `Peripherals::new` constructors; drop glue for `StationLink` and `Tcp`; the analog I2C masters keep main's code
because `Regi2c` is inlined into each chip.

## Privacy

No firmware disassembly, private captures or host information is retained.
Register interpretations remain explicitly inferred where no header exists.


## Rebase qualification on bb2b33d5

Wi-Fi descriptor reads continue using the shared DescriptorWalk adapter. The retained link matches other chips on MAC reset; register clearing is classic-only. Retaining last_rx_desc/last_rx_us can delay the first post-reset frame by the bounded 50 ms RX stall interval. No additional RX policy change is made.

Rust 1.99.0: both Clippy targets pass; empty-HOME CI-policy 906 passed/0 failed/0 ignored; plain 878 passed/0 failed/46 ignored; production WASM build, eight demos and privacy pass. `checks.json` identifies the exercised source revision; subsequent receipt changes are documentation only.
