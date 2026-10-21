# EX224: classic ESP32 peripheral adapters

Base: upstream main `bb2b33d5` (#199/#205/#206 merged).

The shared I2C engine is specialized by command count. Existing chips retain
exactly eight slots and no runtime layout field; classic has sixteen slots
and the old opcode mapping. Main's pin-address device matching is preserved.
Both slot counts expand one macro body into separate non-generic impls, so
S3/C3/C6 still compile one copy of each I2C method; a generic impl would be
instantiated in every calling crate and inlined differently there.
Classic uses #201's `esp_periph::gdma::DescriptorWalk::read_with` with a
memory-only word reader and finite chain walks with cycle rejection. The shared
walker owns descriptor decoding, word-specific faults and visit budgets. No
second `dma_descriptor_reader!` macro or `esp_periph::dma` module remains.
S3's SPI, camera and crypto walks retain main's implementation. DMA errors do
not publish classic TX completion or send malformed transfers to the board.

No existing chip gains fields or per-tick work. Classic peripheral clocks and
transaction timings are inferred. Hardware fade, RMT receive and bit-level
SPI timing are not modeled. RMT emits bits through the shared decoder and
BoardModel path; streams beyond 4096 bits are bounded and not published as
complete frames. There is no second pulse observer, FIFO mode or speculative
RX ownership error model. Continuous output retains only the latest lap.

Classic LEDC maps HS/LS banks onto two `Ledc<true>` groups: S3's eight-channel
map with C6-width timer fields, and a configuration write that cancels a
pending duty update. As with I2C, both variants expand one macro body. The
inline source hook selects APB, REF_TICK or RC_FAST per classic timer;
existing chips pass their single source clock. SPI receive words use the
shared `fill_spi_w!` macro and RMT bits the shared `ws2812_bit!` decoder;
S3/C3/C6 expand both to their previous inline code. I2C uses I2c<16> directly,
with blocked-line timeout handled at START. Pin routes use the shared router
over a linear copy of the classic pad registers in which the GPIO-matrix
function carries the S3/C3/C6 number; existing chips' pin tables are unchanged. UART1/UART2 native pads use IO_MUX
function 4 (`io_mux_reg.h:181,189,242,250`: `FUNC_SD_DATA2_U1RXD`,
`FUNC_SD_DATA3_U1TXD`, `FUNC_GPIO16_U2RXD`, `FUNC_GPIO17_U2TXD`), as selected by
`uart_set_pin` through `uart_pins.h`; UART0 keeps function 0. A host-device test
covers Arduino `Serial2` on GPIO16/17 and the absence of routes on reset pads.
The inactive-board gate has an assertion test that rejects tick/MMIO callbacks.

The shared RMT engine generalization is not included: classic tests require
half-symbol deadlines, an idle tick between continuous laps, and wrapping a
borrowed allocation across word 511. The existing shared engine consumes whole
symbols and rejects allocations past the end of RAM. Unifying these contracts
would expand this part into a timing change for S3/C3/C6; retaining the classic
engine preserves both contracts without adding branches to their tick paths.

## Sources

ESP-IDF v5.5.4 `components/soc/esp32/register/soc/`:
`spi_reg.h` lines 361-372, 1285-1334 and 1414-1468 define transfer, link and
interrupt bits. `rmt_reg.h` lines 79-138 define idle, clock and continuous
controls. `ledc_reg.h` lines 1464-1472 and 1666-1674 define timer resolution.
`i2c_reg.h` defines sixteen command slots; `components/hal/esp32/include/hal/i2c_ll.h`
defines their old opcode mapping. `clk_tree_defs.h:40` supplies the nominal
8.5 MHz RC_FAST clock; the port corrects the prototype's 8 MHz value. Code comments cite these sources. The
firmware boot regression uses the IDF 5.5.4 fixture and hashes from EX230.
No hardware or IDF 4.4 validation is claimed.

## Checks

Use the full EX230 command set on this branch, including both Clippy targets,
empty-HOME workspace runs with ROM-only inputs and with no firmware variables,
all eight production WASM scenarios, and evidence privacy. Classic hello goldens match upstream main byte-for-byte; no regeneration is needed. No JIT implementation changes.

[Mutation table](mutations.json) records 32 mutations killed by assertion
failures. Apply each single replacement and run
`cargo +1.99.0 test -p CRATE --lib TEST`, or `--test TARGET TEST` for
rows naming an integration target, then restore it. The LEDC shadow test
isolates channel updates from timer updates; neither missing latch gate is
hidden by the other one.



## CPU comparison

Not measured separately. The stack tip, PR #212 (schematik-engineering/esp32sim@bcd4df7e, branch `esp32-classic-host`, which contains this change), was measured against main `2f9443a9` with the method in the classic-host receipt (user CPU, one warmup and seven alternating load-gated pairs, identical instruction counts and console hashes):

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
expansion, so three merge and the END arm moves; `Ledc::output`, the host PWM query, and the `Peripherals::new` constructors.

## Privacy and limits

Only source expressions, test names and outcomes are retained. No private
firmware, captures or machine identifiers are included.


## Rebase qualification on bb2b33d5

The timed modern I2C engine retains #199 runtime addressing, multicast and byte deadlines; the sixteen-slot classic specialization retains synchronous transactions and its classic opcodes. #205 RMT DMA and gather paths remain intact; the inline WS2812 decoder is shared. Classic timer, LEDC and RMT stepping uses the corrected APB accumulator, and board deadlines convert bus time back to CPU clocks. Tests that assumed PLL timing now explicitly select PLL. Classic hello goldens are identical to main, including the 11.043-second reset; no peripheral addition changes that fixture.

Separate commit `schematik-engineering/esp32sim@3c6d2135` (Address classic core follow-up notes) drops the no-op UART tick, retains RC_FAST APB:CPU 1:1 with a mutation-sensitive test, and adds the clock-model note in docs/esp32.md. RC_FAST frequency is still approximated as 40 MHz. The 32 peripheral mutations are assertion-killed; the slot-budget mutation now targets the scheduled engine boundary.

Release comparison against bb2b33d5: 118 selected S3/C3/C6 hot functions identical, one different, none missing. I2C tick loses six instructions exclusively in bounds-panic tails because macro expansion merges their source locations; the idle entry and normal scheduled byte path are identical. This is not a CPU measurement.

Rust 1.99.0: both Clippy targets pass; empty-HOME CI-policy 885 passed/0 failed/0 ignored; plain 857 passed/0 failed/46 ignored; production WASM build, eight demos and privacy pass. `checks.json` identifies the exercised source revision; subsequent receipt changes are documentation only.
