# EX216: I2C addressing and transfer time

Base: upstream main `2b992d2d2fcf19a83c250a2ba8f9f1d708f991cc` (#201/#202/#203/#196 merged).
S3, C3 and C6 share one controller. Device callbacks can change the primary
address, match aliases and accept write-only general calls. A single `start_address(configured, address, read)` callback matches and accepts
the address after physical pin filtering. Every matching device receives the
address and data phases. ACK is the OR of replies; read
data is their wired AND. END retains selection; STOP, route changes and device
removal update it.

Transfers advance at byte/ACK deadlines derived from the programmed bus clock.
Board time advances before peripheral callbacks. The existing optional dispatch
owns activation, ticks and deadlines; S3/C6 interrupt sources also use it, while
C3 I2C interrupts keep main's pin-source scan. Inactive controllers return
no clock and leave the active list. C3 uses its existing active pin-service path;
S3/C6 advance the board before the peripheral tick and deliver its inputs after it,
under main's existing `board_edges` gate (`drain_board_inputs` is main's
`deliver_board_inputs` without the advance). No CPU-loop fields were added; C3 pin activation uses one inlined predicate.

Ordering consequence: because the board advances before peripheral ticks, an
input edge that an RMT or LEDC callback makes due within the same tick is
delivered one tick later than on main. Existing goldens other than panel-sid
(below) are unchanged by this.

Related experiments: EX201 and EX206 established physical pin matching and
timestamped board callbacks but left I2C instantaneous; EX209 added removal.
EX216 adds address fanout and elapsed bus time to those existing paths.
Overlaps: #207 (classic stack) also rewrites the `I2c` core, making it
const-generic over the command-slot count and rewriting `run()`. Agreed merge
order: #199 lands first; #207 is rebased onto it (and onto #205) and takes this
I2C core. #208–#212 are stacked on #207 and carry the same change.

## Register sources and model limits

Public ESP-IDF headers, checked at v5.5.4, matching the panel firmware, and v4.4.7:

- [S3 i2c_reg.h, v5.5.4](https://github.com/espressif/esp-idf/blob/v5.5.4/components/soc/esp32s3/register/soc/i2c_reg.h):
  lines 16-26 low period; 72-78 TRANS_START; 111-117 FSM_RST; 173-179 BUS_BUSY;
  932-1006 high, wait-high, start and stop periods; 1043-1074 clock fields.
  C3 and C6 headers in the corresponding chip directories have the same
  9-bit period, 7-bit wait-high, reset and busy fields.
- [S3 i2c_ll.h, v5.5.4](https://github.com/espressif/esp-idf/blob/v5.5.4/components/hal/esp32s3/include/hal/i2c_ll.h):
  lines 175-201 program low/setup/hold minus one and literal high/wait-high;
  lines 205-235 define fractional A as denominator and B as numerator.
  The register header's A/B prose is reversed; the model follows the HAL.
  C6's equivalent bus configuration is at lines 173-200.
- [C6 pcr_reg.h, v5.5.4](https://github.com/espressif/esp-idf/blob/v5.5.4/components/soc/esp32c6/register/soc/pcr_reg.h#L240-L278):
  NUM at bits 12-19, A at 0-5, B at 6-11, source at 20.
  C6 uses PCR rather than the S3/C3 controller clock register.
- [S3 clk_tree_defs.h, v5.5.4](https://github.com/espressif/esp-idf/blob/v5.5.4/components/soc/esp32s3/include/soc/clk_tree_defs.h#L39):
  nominal RC_FAST 17.5 MHz. C3 uses the same value at line 39, C6 at line 47.
  The model uses a 40 MHz crystal and the existing 80 MHz APB clock.
- [S3 i2c_ll.h, v4.4.7](https://github.com/espressif/esp-idf/blob/v4.4.7/components/hal/esp32s3/include/hal/i2c_ll.h#L160-L171)
  and C3 lines 164-175 use the same bus-period convention. Arduino-ESP32 2.x
  uses IDF 4.4; 3.x uses IDF 5.x. [IDF v4.4.7 S3 soc.h, lines 243-246](https://github.com/espressif/esp-idf/blob/v4.4.7/components/soc/esp32s3/include/soc/soc.h#L243-L246) gives APB 80 MHz, RTC 20 MHz and XTAL 40 MHz;
  the model uses the IDF 5.x nominal value and does not calibrate oscillators.

The sum of programmed start/stop periods, nine clocks per byte including ACK,
one APB tick for END/empty commands, reset cancellation and command completion
semantics are inferred. No hardware timing claim. The register-driven fixtures
need no firmware SDK. Existing panel firmware exercises the production driver.

No SDA/SCL electrical waveform, arbitration, clock stretching, timeout engine,
clock gating or slave mode is modeled. Callbacks see the enclosing bus tick;
a caller consuming several deadlines in one tick sees the tick's final time.
Machine scheduler granularity still applies. C6 samples PCR on controller MMIO access; clock reconfiguration during
an active transfer is not validated. PCA9685 is a test fixture for aliases, not a
new PWM device model. No classic ESP32 or physical sensor validation is claimed.

## Verification

Both strict Clippy checks, the release binary build, eight production WASM demos,
WASM section/timing/BLE ABI checks and evidence privacy checks pass.

Commands run from the repository root with Rust 1.99.0; the default toolchain
was not changed. Final check results are in `checks.json`; source, firmware and public-header hashes
are in `inputs.json`.

```sh
tools/fetch-demo-assets.sh --no-linux
cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
cargo +1.99.0 test --release --workspace
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
node tools/check-evidence-privacy.mjs
python3 docs/evidence/i2c-addressing-timing/mutate.py
```

Release workspace: **795 passed**, zero ignored under CI policy; plain tests:
**769 passed, 44 ignored**. Both runs use an empty HOME, with only
the ROM input for CI policy and no firmware-related variables for plain tests. Build-tool locations are
supplied separately so changing HOME does not hide the Rust installation.
Temporary test output stays inside the worktree. Reproduce the clean environment
with pre-existing Rust tool/cache directories, then run both policies:

```sh
mkdir -p target/empty-home target/test-tmp
env -i PATH="$PATH" RUSTUP_HOME="$HOME/.rustup" CARGO_HOME="$HOME/.cargo" \
  HOME="$PWD/target/empty-home" TMPDIR="$PWD/target/test-tmp" \
  ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" \
  cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
env -i PATH="$PATH" RUSTUP_HOME="$HOME/.rustup" CARGO_HOME="$HOME/.cargo" \
  HOME="$PWD/target/empty-home" TMPDIR="$PWD/target/test-tmp" \
  cargo +1.99.0 test --release --workspace
```

The mutation script restores each edited source in a `finally` block and
requires a failing test result, not a compiler error. Run it without concurrent
source edits or Rust builds. `mutations.json` is the mutation-to-killing-test
table, **44/44 killed**. Sixteen register-driven tests cover current-address replacement/removal, alias enable/disable,
general-call filtering, ACK aggregation, read collision, byte deadlines,
fractional clocks, reset, repeated starts, command exhaustion, optional dispatch
and board callback order on all three chips. The S3/C6 board-order mutations
restore main's advance-after-tick delivery. The last five mutations run the
`esp-periph` `devices` and `esp32c3` library tests listed under Idle path.

## Intentional golden changes

Only `panel-sid.console.txt`, `panel-sid.insns` and `panel-sid.wav.sha256`
change. I2C expander initialization now consumes bus time. Panel main startup
moves from 725 to 1147 ms; instructions from 396469561 to 398637113.
The count includes a final scheduling round cut short at the seven-second
cycle ceiling (#197); console, report and WAV do not depend on that cut.
The fixed seven-second audio window changes accordingly. Console content other
than timestamps is identical. `panel-sid.report.txt` additionally pins stop
interrupt totals and per-source counts. Other existing goldens remain exact.

Old WAV SHA-256:
`89880538c0dc82e11f74bfdd7a4e98e02cc793cfd806bdb78c08e6c9b1378dfe`.
New WAV SHA-256:
`37b4961b189338e0977f283e2485080f5f8078380fac956f969283cc20ca2171`.

## Pocket Tank correctness

[Console comparison, transfer trace, instantaneous control and hardware-model limits](pocket-tank.md):
finite I2C time shifts initialization by 1–2 ms and changes the seeded fish
workload. No new NACK or transfer failure; restoring synchronous completion
reproduces main's console and instruction/interrupt totals exactly. The
AMOLED board transfers are pinned by a firmware-independent register test.

## Idle path

While no transfer is active, I2C does no per-tick or per-deadline-query work
on any chip. Changes reachable on that path, relative to main:

| Path | Change | Idle cost |
| --- | --- | --- |
| Tick delivery (`device_set!` `tick`) | I2C is an `optional` entry | None: existing empty-list branch; optional devices tick only from `active_optional`. |
| Deadline query (`device_set!` `cycles_until_deadline`) | Optional entries leave the static scan; one empty-list branch calls a non-inlined scan over `active_optional` | I2C is not polled. On C3 the static scan no longer reads `BLE_LC` state, replaced by the list-length load; S3/C6 add that load and branch. |
| C3 interrupt sources | I2C's optional entry lists no source | `OPTIONAL_SOURCES` and generated `source_status()` equal main; `pin_irqs_enabled`/`refresh_pin_lines()` still own `I2C_EXT0`. |
| C3 tick (`SocBus::tick`) | `pins_active()` includes `i2c.is_active()` | None while `pins_active` is false: `tick_with_pins()` is skipped as on main. |
| S3/C6 tick (`tick_impl`/`devices`) | The board advance moves ahead of `periph.tick` under the same `board_edges` test; edges and releases drain after it | Same calls as main: one `advance_to` and one edge/release drain per tick, only when `board_edges`; one extra test of the cached flag. |
| C3 MMIO write (`periph_write`) | `Peripherals::write32` returns its existing I2C/RMT/SPI2 block test; the bus refreshes `pins_active` on it | No new address test; SPI2 writes also refresh `pins_active` (idempotent). |
| Controller layout (`I2c`) | `remaining: Option<u64>` (`None` while idle) takes main's `cur: Option<usize>` slot; the command list and C6 clock override are boxed; step indices are `u8` in tail padding | None: `I2c` keeps main's 184 bytes and its largest niche at offset 0, so S3 `Peripherals`/`SocBus` field offsets equal main's (`offset_of!` on every field). C3/C6 embed the same type. |
| S3 interrupt sources (`source_status`) | I2C0/I2C1 bits come from the optional-source cache | Two fewer controller register tests per scan. |
| S3 MMIO write (`periph_write_inner`) | I2C0/I2C1 write arms refresh the optional list | No new test on other blocks; register allocation differs from main. |

Deadline equivalence: an optional device is listed whenever `clock()` is
`Some`; `refresh_optional` runs after its writes and ticks. I2C `has_deadline()`,
`next_deadline()` and `clock()` all follow `remaining`. C3 `BLE_LC` is enabled
iff `clock()` is `Some`; `enable_ble_full` refreshes it and nothing disables
it. LEDC and MCPWM implement no deadline. Deadline values, divider and
rounding are unchanged.

Tests: `optional_devices_are_not_polled_until_configured_and_keep_stopped_irqs`
counts `has_deadline()`/`next_deadline()` calls before configuration and after
stop, and checks an active APB deadline (7 ticks, divider 3: 18 CPU cycles).
`pin_clock_participation_tracks_i2c_transfers` checks that TRANS_START enters
and completion leaves C3 pin service, with a 6-cycle STOP deadline.
`wifi_and_pin_sources_survive_interrupt_refresh` rejects C3 pin-source bits in
the optional cache. `controller_leaves_optional_dispatch_after_completion_and_reset`
checks I2C deadline methods before, during and after a transfer and after reset.

### Rebase onto current main

Current base: upstream main `2b992d2d` (#203 and #196 merged). Rust 1.99.0
AArch64 release binaries, separate target directories,
`llvm-objdump -d --demangle --no-show-raw-insn` and `asmcmp.py` normalization.
[Current structured comparison](rebase-2b.json) records selected emitted functions
and every probed layout field. The PR's Rust diff against this main is byte-identical
to its Rust diff against `a175e357`; no further implementation change was made.

C3 retains the `write32` boolean / `pins_active()` update followed by the SHA DMA
pending hook. Optional deadlines use `active_optional` only; the gated pin-source
scan remains I2C's single IRQ owner. C6 SPI/I2S work precedes board advance and
peripheral ticks. All 185 Peripherals/SocBus size/offset records match main
across S3/C3/C6; `I2c` remains 184 bytes on both revisions.

All compared emitted Machine run/step bodies match. C3 BLE_LC's standalone
`next_deadline` is absent in the branch; active-only use is inlined into optional
dispatch. Selected differences are listed below, not claimed instruction-identical.
They cover I2C activation, ticking and active-list deadlines, board ordering,
cached IRQ sources and C6 PCR clock sampling, plus register/block placement.
This is static evidence, not CPU timing.

| Chip / function | Main instructions | Branch instructions |
| --- | ---: | ---: |
| S3 `periph_write_inner` | 1663 | 1742 |
| S3 `refresh_tick_budget` | 278 | 282 |
| S3 `tick_impl` | 3743 | 3742 |
| S3 `Peripherals::tick_optional` | 312 | 394 |
| S3 `Peripherals::source_status` | 263 | 252 |
| C3 `next_deadline` | 200 | 194 |
| C3 `periph_write` | 1705 | 1734 |
| C3 `pin_deadline` | 211 | 205 |
| C3 `tick_with_pins` | 521 | 542 |
| C3 `Peripherals::tick_optional` | 324 | 365 |
| C6 `tick` | 2036 | 2042 |
| C6 `next_deadline` | 290 | 298 |
| C6 `periph_write` | 1747 | 1799 |
| C6 `Peripherals::tick_optional` | 227 | 274 |
| C6 `Peripherals::read32` | 677 | 696 |

### Historical rebase onto #201/#202

Release binaries built with Rust 1.99.0 on arm64 macOS were compared against
`ff64902a` using `llvm-objdump -d --demangle --no-show-raw-insn` and the
address-normalizing `asmcmp.py` comparator. [Structured results](rebase-validation.json)
retain normalized instruction counts, field offsets and the single-run Pocket Tank check.

- S3 `run_unmodeled`, `run_modeled`, `step_core`, bus `tick` and `periph_read`
  are identical. `tick_impl`: 3667 → 3673 instructions; `refresh_tick_budget`:
  278 → 282; `periph_write_inner`: 1663 → 1742.
- C3 `run`, `run_unmodeled`, `step_core`, bus `tick`, `pending_work` and
  `periph_read` are identical. `next_deadline`: 200 → 194 (the active optional
  list replaces the static BLE_LC deadline poll); `periph_write`: 1660 → 1691.
- C6 `run`, `run_unmodeled`, `step_core`, `pending_work` and `periph_read`
  are identical. Bus `tick`: 2036 → 2042; `next_deadline`: 290 → 298;
  `periph_write`: 1660 → 1741. `devices` is inlined into `tick`; S3 `run`
  has no separate emitted symbol, so its emitted CLI caller and run bodies were checked.
- S3 `tick_impl` and C6 `tick` retain exactly the same call multisets as main
  (164 and 88 call sites respectively), with board advance moved before tick
  and one extra `board_edges` test. Register allocation and stack slots also
  change; these functions are not byte-identical. Deadline/write differences
  are the optional-list and I2C write activation changes described above.
- `offset_of!` dumps match field-for-field: all 72 S3 `SocBus`/`Peripherals`
  fields, both struct sizes and `I2c` size (75 entries total). `I2c` remains
  184 bytes, `Peripherals` 9360, `SocBus` 12408; `i2c` and `periph` remain at
  offset zero, preserving the niche-driven enclosing layout.
- C6 `pending_work` keeps SPI and I2S RX service before board advance;
  C3 MMIO keeps main's combined SPI_EXEC/RX_CONF handling, RX-state update
  and work refresh while returning the existing pin-block classification.
  Main's PCM routing, ADC streams and shared GDMA walker are retained.

The instantaneous-I2C control is the rebased source plus only
`self.advance(u64::MAX)` after `self.schedule()` in `I2c::run()`. One run
per arm, 30 emulated seconds on `waveshare-amoled18-v2`, matches main
`ff64902a`: **10073833665 instructions** (cores 4446180034 / 5627653631),
1006391 exceptions, 95179 interrupts and console SHA-256
`c3776029dd98a1cd1ad148aa3cfdbd6639a49e465157214c2f1ca34ac5e2bbd5`.
This is a correctness check, not a CPU timing measurement.

## CPU comparison

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 2f9443a9 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `schematik-engineering/esp32sim@b1c67907` (branch `i2c-addressing-timing`, before its rebases) against main `2f9443a9`. These are historical CPU samples, not measurements of the `2b992d2d` rebase. The Pocket Tank table likewise retains its earlier main `eec7b2c9` and fork control revision. The rebase onto merged #203 is checked by release disassembly and field-offset dumps; the instantaneous-control correctness check is separate from CPU timing. No new CPU benchmark was run. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| S3 hello | 24.0373 (23.6903–25.0699) | 24.1383 (23.7769–25.3374) | +0.42% | 5/7 | 1789819657 | 4.62 |
| C3 hello | 2.3552 (2.3254–2.5007) | 2.3553 (2.3160–2.3796) | +0.00% | 2/7 | 4800000000 | 3.10 |
| C6 hello | 3.0831 (3.0423–3.1483) | 3.0920 (3.0332–3.1513) | +0.29% | 4/7 | 4800000000 | 2.83 |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 23.941341 | 23.846167 | 3.64/3.10/3.64 | 4.62/3.64/4.62 | accepted |
| 1 | 2 | M→B | 23.690272 | 23.821893 | 3.10/2.45/3.10 | 2.45/1.95/2.45 | accepted |
| 2 | 3 | B→M | 23.822947 | 23.863793 | 1.70/1.46/1.70 | 1.95/1.70/1.95 | accepted |
| 3 | 4 | M→B | 24.240985 | 24.534307 | 1.46/1.44/1.48 | 1.44/1.65/1.68 | accepted |
| 4 | 5 | B→M | 23.853731 | 23.776909 | 1.88/1.65/1.88 | 1.65/1.88/1.95 | accepted |
| 5 | 6 | M→B | 24.037307 | 24.812378 | 1.65/1.72/1.72 | 1.72/2.38/2.38 | accepted |
| 6 | 7 | B→M | 25.008786 | 24.138289 | 2.04/2.11/2.12 | 2.38/2.04/2.38 | accepted |
| 7 | 8 | M→B | 25.069922 | 25.337400 | 2.11/2.35/2.35 | 2.35/2.67/2.67 | accepted |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.478101 | 2.548989 | 3.10/3.10/3.10 | 2.67/3.10/3.10 | accepted |
| 1 | 2 | M→B | 2.409126 | 2.332956 | 3.10/3.09/3.10 | 3.09/3.09/3.09 | accepted |
| 2 | 3 | B→M | 2.349123 | 2.316006 | 3.00/3.00/3.00 | 3.09/3.00/3.09 | accepted |
| 3 | 4 | M→B | 2.325419 | 2.372879 | 3.00/2.84/3.00 | 2.84/2.84/2.84 | accepted |
| 4 | 5 | B→M | 2.500651 | 2.373659 | 2.93/2.93/2.93 | 2.84/2.93/2.93 | accepted |
| 5 | 6 | M→B | 2.382596 | 2.379636 | 2.93/2.94/2.94 | 2.94/2.94/2.94 | accepted |
| 6 | 7 | B→M | 2.329875 | 2.355290 | 2.94/2.86/2.94 | 2.94/2.94/2.94 | accepted |
| 7 | 8 | M→B | 2.355221 | 2.351089 | 2.86/2.86/2.86 | 2.86/2.71/2.86 | accepted |

### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.171948 | 3.062761 | 2.74/2.74/2.74 | 2.71/2.74/2.74 | accepted |
| 1 | 2 | M→B | 3.101561 | 3.151329 | 2.74/2.60/2.74 | 2.60/2.60/2.60 | accepted |
| 2 | 3 | B→M | 3.048916 | 3.071167 | 2.63/2.82/2.82 | 2.63/2.63/2.63 | accepted |
| 3 | 4 | M→B | 3.148267 | 3.092029 | 2.82/2.82/2.82 | 2.82/2.83/2.83 | accepted |
| 4 | 5 | B→M | 3.042312 | 3.100649 | 2.77/2.77/2.77 | 2.83/2.77/2.83 | accepted |
| 5 | 6 | M→B | 3.087219 | 3.047839 | 2.77/2.62/2.77 | 2.62/2.62/2.62 | accepted |
| 6 | 7 | B→M | 3.079720 | 3.033229 | 2.57/2.45/2.57 | 2.62/2.57/2.62 | accepted |
| 7 | 8 | M→B | 3.083089 | 3.104397 | 2.45/2.45/2.45 | 2.45/2.41/2.45 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are not committed.

### Pocket Tank

Measured against main `eec7b2c9` with the same method as the hello table above (user CPU, one warmup and seven alternating load-gated pairs), the control arm built from `schematik-engineering/esp32sim@7b404a99` (branch `i2c-addressing-timing-instant-control`). Instruction counts and console hashes are identical in every pair:

| Workload | Main median (range) | Control median (range) | Change | Control slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Pocket Tank | 35.5447 (34.9420–37.9908) | 35.2735 (34.9134–35.5341) | -0.76% | 2/7 | 10073833665 | 5.05 |

Why a control: on this branch, finite I2C transfer time
changes the game's time-derived seed, so the PR's Pocket Tank run executes
different guest work from main and a direct timing pair is not an emulator
overhead comparison ([pocket-tank.md](pocket-tank.md)).

The comparison arm is an instantaneous-I2C control build: this branch's code
with one change, `I2c::run()` calling `self.advance(u64::MAX)` right after
`self.schedule()`, so every command list completes at TRANS_START with zero bus
time, as on main. Everything else stays as on this branch: address fanout,
aliases, general call, optional dispatch, the S3/C3/C6 tick order (board
before peripherals) and the controller layout. A transfer never stays active,
so I2C never joins the optional deadline list. Single correctness runs of the
control (30 emulated seconds, `waveshare-amoled18-v2`) reproduce main
`eec7b2c9`'s console SHA-256 and instruction count exactly; see
[pocket-tank.md](pocket-tank.md). The historical timing result is the table above;
no new timing is claimed for the rebase.

## Evidence scope

The receipt retains public input hashes, commands, assertions and results.
Raw compiler/test output and downloaded headers are omitted. No personal paths,
host identities, device identifiers or private firmware are retained.
