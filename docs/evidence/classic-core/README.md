# EX230: classic ESP32 core

Base: upstream main `f1569101` (merged #205). The classic chip uses the shared hooks merged in
#183, including the LX6-only UR 234-236 gate. The shared bus trait adds an inline identity
conversion for fixed-clock chips; classic overrides it for CPU-clock deadlines. Timer stepping and
alarm-gap logic are shared through `#[inline(always)]` `Timer` helpers; S3/C3/C6 keep 54-bit
masks and main's unsaturated deadline product, classic uses 64-bit masks and a saturating
deadline. Shared structs keep their main layout. Native and WASM front ends select the new
chip outside execution loops; the WASM chip enum appends the classic variant last.

The chip models the ECO3 memory map, PRO flash MMU, 256-byte decode-cache
invalidation, DPORT routing, UART, GPIO/IO_MUX, eFuse, flash and boot SHA-256,
RTC control and TIMG including LACT. TIMG edge sources are 58 and 62.
The APP MMU table is stored but execution currently uses the PRO mapping.
Bus time uses a stable 240 MHz reference; the selected CPU runs at 40 MHz XTAL
or 80/160/240 MHz PLL. APB is 40 MHz on XTAL and 80 MHz on PLL; RTC slow is 150 kHz.
No PSRAM, DFP arithmetic or hardware timing validation is claimed. LACT sleep
stepping and timer-group watchdog execution remain outside the model.
Board callbacks and matrix peripheral signals belong to the peripherals part;
the core has no board callbacks in its tick or MMIO paths. A classic UART
wrapper adds the receive-pointer register; the shared UART is unchanged.
Direct app boot is rejected.

## Review fixes

1. **Flash identity:** preserve both SPI0/SPI1 JEDEC bytes, as the S3 reset path
   does. `classic_reset_clock_and_flash_size` boots the committed fixture with
   `--flash-mb 16 --max-seconds 25`: all three boots report `Detected size(16384k)`;
   none revert to 8192k. The image header remains 4096k, so the expected warning
   is retained rather than changing the firmware.
2. **CPU time:** `DPORT_CPU_PER_CONF` (+0x3c), CPUPERIOD_SEL 0/1/2 selects
   80/160/240 MHz when `RTC_CNTL_CLK_CONF` (+0x70), SOC_CLK_SEL[28:27] is 1;
   source 0 uses the 40 MHz XTAL. CCOUNT and instruction/idle budgets stay in
   CPU clocks. Classic bus ticks convert them to the stable time reference,
   preserving half ticks at 160 MHz. Peripheral, stop and idle-script deadlines
   convert back to CPU clocks. LACT advances the same 1 ms for 80,000/160,000/
   240,000 CPU clocks at the respective PLL settings.
   IDF v5.5.4: `components/soc/esp32/register/soc/dport_reg.h:154-174`,
   `rtc_cntl_reg.h:863-881`, and
   `components/hal/esp32/include/hal/clk_tree_ll.h:543-626`.
   `components/freertos/Kconfig:486-489,526-528` selects the classic CCOUNT
   tick source; the operative `_xt_tick_divisor = esp_clk_cpu_freq() / XT_TICK_PER_SEC`
   definition is `components/freertos/FreeRTOS-Kernel/portable/xtensa/xtensa_init.c:51-54`.
   Before the clock fix, resets occurred at **7.351, 14.702, 22.052 s**.
   After it, resets occur at **11.043 and 22.086 s** in the 25-second run.
   The stock fixture actually delays for each count **10 through 0** (eleven
   one-second delays), so about 11 s including boot is expected, not 10 s.
   The 3-second golden now ends at “Restarting in 8 seconds...” instead of 6;
   its report changes from 4474 exceptions / 903 interrupts to 3174 / 597.
   These are guest exception totals, including normal Xtensa window traps,
   not a zero-exception claim. S3/C3/C6 retain their identity conversion.
3. **RWDT:** clear classic WDTCONFIG0 at +0x8c, leaving CONFIG3 at +0x98 alone.
   IDF v5.5.4 `components/soc/esp32/register/soc/rtc_cntl_reg.h:1615,1722`.
4. **GPIO:** expose GPIO23. Keep GPIO20 hidden because this D0WD model does not
   model the PICO-V3-02 bond-out. Exclude 24 and 28–31 according to IDF v5.5.4
   `components/soc/esp32/include/soc/soc_caps.h:181-186`.
5. **RTC alias:** separate classic WAKEUP_STATE (+0x38) from RESET_STATE (+0x34).
   Preserve writable wake-enable/filter fields and read-only zero wake causes;
   sleep wake events remain unmodeled. IDF v5.5.4
   `components/soc/esp32/register/soc/rtc_cntl_reg.h:505-523`.

6. **XTAL APB:** derive divider 6 (40 MHz) on SOC_CLK_SEL=0 and divider 3
   (80 MHz) on PLL. APB phase is retained across source changes; timer deadlines
   use the same divider. `xtal_apb_lact_prescaler_counts_real_milliseconds`
   reproduces the probe: XTAL with LACT divider 20 counted **4000 before, 2000
   after per emulated millisecond**. PLL with divider 40 remains 2000 per ms
   (80 MHz APB), including a PLL→XTAL transition. A separate phase/deadline
   regression covers short batches and both selectors.
   IDF v5.5.4 `components/esp_hw_support/port/esp32/rtc_clk.c:373-389`
   sets APB=CPU and LACT prescale `cpu_freq / LACT_TICKS_PER_US`;
   `components/esp_hw_support/include/esp_private/systimer.h:26` defines
   two LACT ticks per microsecond, hence prescaler 20 at 40 MHz.

The shared core's CCOUNT reset behavior is deliberately unchanged. Clock
switches take effect at the existing scheduling-round boundary, not at an
individually priced instruction. APLL, RC clock calibration, XTAL division and
strict cost-model timing are not qualified here; unsupported CPU selectors use
40 MHz CPU, with RC_FAST/APLL APB still approximated as 80 MHz.
Reverse deadline conversion rounds up to the next CPU boundary, so a stop bound
can overshoot by up to 5 reference ticks on XTAL (about 21 ns), a cosmetic limit.
Clock state after a non-SW reset (RTC +0x70 retained, DPORT rebuilt) is unverified
against hardware; no fixture exercises it. #199 and #205 are merged; restack/re-pin
the remaining classic stack in order #207 → #212 after #206.

## Inputs and reproduction

[Input hashes](inputs.json) identify the committed firmware. The
[recipe](../../../examples/hello_world-classic/README.md) pins pioarduino
55.03.38-1, ESP-IDF 5.5.4 and GCC 14.2.0_20260121. Two clean builds in different
source/output directories produced identical bootloader, partition and app
bytes. The ROM fetcher pins esp-rom-elfs 20260528 and the ECO3 ELF hash.

```sh
tools/fetch-demo-assets.sh --no-linux
cargo +1.99.0 clippy --workspace --all-targets -- -D warnings
cargo +1.99.0 clippy --release --target wasm32-unknown-unknown -p esp32sim-wasm --features jit-tests -- -D warnings
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
cargo +1.99.0 test --release --workspace
RUSTUP_TOOLCHAIN=1.99.0 tools/wasm-build.sh
node tools/wasm-test.mjs hello c3-hello c6-hello c6-energy-scan c6-contiki c6-contiki-net c6-rpl-net panel
node tools/check-evidence-privacy.mjs
```

The workspace test runs use an empty HOME, with RUSTUP_HOME pointing to the
installed toolchain. ESP32SIM_ROM_DIR is the only firmware input environment
variable for the ignored-test run; the plain run has no firmware variables.
The classic golden pins console, exceptions and interrupt totals/per-source
counts. It does not pin cycle-derived instruction counts. The earlier CPU-time correction intentionally re-pinned the two classic
hello goldens. This APB follow-up changes no golden bytes: the hello fixture
never enters the XTAL phase. All golden files are byte-identical to reviewed
`schematik-engineering/esp32sim@9580363f`. JIT implementation files are unchanged.

## Results

Rust 1.99.0 on the f1569101-based source: both Clippy targets pass with warnings
denied; 848 CI-mode workspace tests and 820 plain tests
(46 ignored) pass with empty HOME. All eight production WASM scenarios
and evidence privacy pass. The corrected classic goldens pass; all non-classic
goldens remain byte-identical. The validated revision is in [checks.json](checks.json).
The fixture reports silicon revision v2.0 despite using the ECO3 ROM; the
model does not supply the additional revision-3 date bit. Its boot output
also retains inferred clock-calibration warnings. These are recorded limits,
not hardware equivalence claims.

## Mutation checks

[Mutation table](mutations.json): each row names the changed expression and
the test that fails. All 36 mutations (17 original, 16 prior review-fix mutations, 3 APB mutations)
are killed by assertions, not compile errors. Reproduce each with
`cargo +1.99.0 test --release -p esp32 --lib TEST`, or
`cargo +1.99.0 test --release -p esp32 --test review_core TEST` when the row
names that target, after applying the replacement; then restore the expression.
The GPIO test drives an undriven pad low with a pull-down before enabling
its pull-up; starting high alone cannot detect missing pull handling.

## Register sources

The fixture uses IDF 5.5.4. Source comments cite its
`components/soc/esp32/register/soc/*_reg.h` definitions and
`components/soc/esp32/include/soc/{soc,interrupts,gpio_sig_map}.h`.
The interrupt enum's lines 75 and 79 give edge sources 58 and 62.
`timer_group_reg.h` lines 42-53 give T0 edge/level bits 12/11;
397-438 give LACT enable/direction/reload/divider/alarm bits.
`io_mux_reg.h` lines 41-66 give pulls, input enable and function selection.
Completion timing and ROM compatibility behavior are inferred, not measured
on hardware. This fixture does not establish an IDF 4.4 firmware contract.

## CPU comparison

Maintainer-measured re-review of `schematik-engineering/esp32sim@9580363f`
(branch `esp32-classic-2-core`) against main `2b992d2d`: user CPU, seven alternating
load-gated pairs, identical instructions and hashes, no flag. These measurements
were supplied by the maintainer; no new CPU benchmark was run for this APB follow-up.

| Workload | Change | PR slower in N/7 |
| --- | ---: | ---: |
| C3 hello | 0.0% | 3/7 |
| C6 hello | 0.0% | 3/7 |
| S3 hello | −0.2% | 4/7 |
| Pocket Tank | +0.4% | 5/7 |

The older measured samples below remain historical, not current-head timings.

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on schematik-engineering/esp32sim@274516d4 (branch `esp32-classic-2-core` at the time) against main `954f2a68`. These are historical pre-clock-fix samples, not measurements of the current branch.
No CPU benchmark was run for these review fixes. Classic-only clock conversion
adds integer arithmetic per scheduling round; its host CPU cost is unmeasured.
User CPU seconds for the historical source follow.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.2583 (3.2237–3.3002) | 3.2710 (3.2610–3.2895) | +0.39% | 4/7 | 4800000000 | 4.14 |
| Pocket Tank | 35.6747 (35.6081–38.3170) | 35.7083 (35.6130–35.9298) | +0.09% | 3/7 | 10073833775 | 3.03 |
| S3 hello | 25.4214 (25.0221–28.6998) | 25.3276 (24.9841–26.3929) | -0.37% | 3/7 | 1789819657 | 4.10 |
| C3 hello | 2.4209 (2.4161–2.4617) | 2.4404 (2.4134–2.4785) | +0.81% | 4/7 | 4800000000 | 3.11 |

### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.315423 | 3.386911 | 4.05/4.05/4.05 | 4.14/4.05/4.14 | accepted |
| 1 | 2 | M→B | 3.300188 | 3.284723 | 4.05/3.81/4.05 | 3.81/3.58/3.81 | accepted |
| 2 | 3 | B→M | 3.243717 | 3.270954 | 3.58/3.53/3.58 | 3.58/3.58/3.58 | accepted |
| 3 | 4 | M→B | 3.253785 | 3.260999 | 3.53/3.33/3.53 | 3.33/3.33/3.33 | accepted |
| 4 | 5 | B→M | 3.288584 | 3.261825 | 3.14/2.97/3.14 | 3.33/3.14/3.33 | accepted |
| 5 | 6 | M→B | 3.258280 | 3.284469 | 2.97/2.97/2.97 | 2.97/3.05/3.05 | accepted |
| 6 | 7 | B→M | 3.296547 | 3.265093 | 3.13/3.13/3.13 | 3.05/3.13/3.13 | accepted |
| 7 | 8 | M→B | 3.223713 | 3.289536 | 3.13/3.12/3.13 | 3.12/3.03/3.12 | accepted |

### Pocket Tank

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 35.732971 | 35.459644 | 2.50/2.74/2.74 | 3.03/2.50/3.03 | accepted |
| 1 | 2 | M→B | 35.674723 | 35.708295 | 2.74/2.08/2.74 | 2.08/1.91/2.08 | accepted |
| 2 | 3 | B→M | 35.608068 | 35.929791 | 1.51/1.75/1.82 | 1.91/1.51/1.91 | accepted |
| 3 | 4 | M→B | 35.879542 | 35.619119 | 1.75/2.48/2.56 | 2.48/1.96/2.48 | accepted |
| 4 | 5 | B→M | 35.633072 | 35.683283 | 1.58/1.77/1.77 | 1.96/1.58/1.96 | accepted |
| 5 | 6 | M→B | 38.317014 | 35.864055 | 1.77/1.94/2.12 | 1.94/1.91/1.95 | accepted |
| 6 | 7 | B→M | 35.648992 | 35.613042 | 1.58/1.46/1.58 | 1.91/1.58/1.91 | accepted |
| 7 | 8 | M→B | 35.932761 | 35.879612 | 1.46/1.46/1.50 | 1.46/1.40/1.56 | accepted |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 25.249511 | 25.158672 | 1.40/1.46/1.50 | 1.40/1.40/1.47 | accepted |
| 1 | 2 | M→B | 25.038804 | 25.533096 | 1.46/1.42/1.55 | 1.42/1.63/1.68 | accepted |
| 2 | 3 | B→M | 26.068775 | 26.392894 | 2.45/3.14/3.28 | 1.63/2.45/2.45 | accepted |
| 3 | 4 | M→B | 25.514731 | 25.327616 | 3.14/2.56/3.14 | 2.56/2.16/2.56 | accepted |
| 4 | 5 | B→M | 25.022109 | 25.116522 | 1.76/1.95/2.12 | 2.16/1.76/2.16 | accepted |
| 5 | 6 | M→B | 25.399362 | 25.116221 | 1.95/2.37/2.53 | 2.37/2.49/2.49 | accepted |
| 6 | 7 | B→M | 28.699832 | 25.734873 | 4.10/3.56/4.10 | 2.49/4.10/4.10 | accepted |
| 7 | 8 | M→B | 25.421370 | 24.984144 | 3.56/3.57/3.57 | 3.57/3.11/3.57 | accepted |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.462578 | 2.526983 | 3.02/3.02/3.02 | 3.11/3.02/3.11 | accepted |
| 1 | 2 | M→B | 2.461655 | 2.429068 | 3.02/2.86/3.02 | 2.86/2.86/2.86 | accepted |
| 2 | 3 | B→M | 2.416063 | 2.455406 | 2.71/2.71/2.71 | 2.86/2.71/2.86 | accepted |
| 3 | 4 | M→B | 2.419689 | 2.457303 | 2.71/2.57/2.71 | 2.57/2.57/2.57 | accepted |
| 4 | 5 | B→M | 2.419744 | 2.478514 | 2.53/2.53/2.53 | 2.57/2.53/2.57 | accepted |
| 5 | 6 | M→B | 2.420874 | 2.413361 | 2.53/2.56/2.56 | 2.56/2.56/2.56 | accepted |
| 6 | 7 | B→M | 2.440300 | 2.440420 | 2.52/2.52/2.52 | 2.56/2.52/2.56 | accepted |
| 7 | 8 | M→B | 2.442235 | 2.434022 | 2.52/2.48/2.52 | 2.48/2.48/2.48 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Raw output files are not committed; every accepted pair is in the tables above.

Historical static check (no timing): release `esp32sim` binaries from main `2f9443a9`
and this branch before the rebase were disassembled with `llvm-objdump -d` and compared function
by function after normalising addresses, alignment `nop`s, adrp page offsets
and linker-chosen symbol aliases. The S3/C3/C6 execution path is
instruction-identical to main, including `Machine::run`, `step_core`, each
`SocBus` `tick` and `next_deadline`, `TimerGroup::tick` and the UART, TIMG,
RTC_CNTL and SHA register paths. The classic RTC adapter masks its translated
offset to the 4 KiB block so the shared `RtcCntl::write` keeps main's code.
Shared `Gpio::write`, which the classic GPIO adapter also calls, uses `ubfx`
instead of `lsr` for one register index, with the same instruction count.
Other differences are in CLI setup and reporting, `Cpu::dump`, a coverage
report and the opt-in register-trace observer. The comparison was repeated
against main `eec7b2c9` on the rebased stack tip (#211); the S3/C3/C6 hot
path result is unchanged.

## Current static code-generation check

Release binaries at main `f1569101` and
`schematik-engineering/esp32sim@0c1e324b` (branch `esp32-classic-2-core-apb`)
were disassembled and compared with the committed [asmcmp.py](asmcmp.py) and
[hot-path selector](hot.py) (addresses, relocation pages and symbol aliases
normalized). All **119 selected S3/C3/C6 hot functions match**, with zero
differences or missing functions. The same selector previously selected 115
on main `2b992d2d`; the new base emits four additional selected functions.
[Artifact hashes](disassembly.json) identify the current comparison.

```sh
cargo +1.99.0 build --release -p esp32sim --bins
xcrun llvm-objdump -d --demangle --no-show-raw-insn target/release/esp32sim > candidate.dis
# Repeat on main f1569101 with a separate or clean target directory, producing main.dis.
python3 docs/evidence/classic-core/hot.py main.dis candidate.dis
# hot functions: same=119 diff=0 missing=0
``` The earlier GPIO index
`lsr`/`ubfx` difference above is historical: explicitly inlining the shared
GPIO write into its layout adapters restores identical fixed-chip code.
No S3/C3/C6 chip source or shared CCOUNT reset behavior changed.

## Privacy

No private captures are retained. Firmware reproducible-build settings map
source paths to generic labels. Input records contain hashes and public tool
versions; link-map paths are omitted. License notices retain upstream legal
attribution.
