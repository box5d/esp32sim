# EX229: classic host input and I2S receive

Base: PR #211, branch `esp32-classic-ble-bb`, `schematik-engineering/esp32sim@363e6f0a69188cde5d148d0363ff049fa4273166`, on upstream main `bb2b33d5`.

Classic UART APB/AHB routing, matrix input, GPIO feedback/release, physical SPI,
raw ADC observations and Ethernet relay already come from EX230 and EX224-EX228.
This part adds I2S0/1 RX through the shared PCM packer, source bank, clocked
queue and GDMA receive channel from EX214/EX215. The routing descriptor now
takes BCLK/WS indices explicitly because classic's data and clock signals are
not adjacent; S3/C3/C6 pass their previous indices.

## Register mapping

The adapter keeps classic registers in a `RegRam` and re-encodes them into the
shared S3-layout model:

- **Owner check.** `I2S_LC_CONF_REG` (+0x60) bit 12 `CHECK_OWNER`, reset 0
  (`i2s_reg.h:856-868`), drives the shared channel's owner check. IDF v5.5.4 never
  calls `i2s_ll_dma_enable_owner_check` (`hal/esp32/include/hal/i2s_ll.h:60-63`)
  from the I2S driver. It sets `owner = 1` once when it allocates the circular
  ring (`esp_driver_i2s/i2s_common.c:534,551`). The writeback clears the owner bit,
  so with the reset value the ring keeps running. LC_CONF resets to `0x100`
  (OUT_EOF_MODE).
- **DMA reach.** Descriptor reads, buffer writes and descriptor writeback go
  through a DRAM-only bus for the shared channel. The bus covers
  `0x3FFA_E000..0x4000_0000` (`SOC_DMA_LOW`/`SOC_DMA_HIGH`,
  `components/soc/esp32/include/soc/soc.h:203-205`). Registers, ROM, IRAM, RTC
  memory and flash fault without being accessed. `I2S_INLINK_ADDR` is 20 bits
  under `0x3FF0_0000` (`i2s_reg.h:763-768`), so offset `0x40000` names the UART0
  FIFO. It now raises IN_DSCR_ERR and leaves the FIFO alone.
- **Resets.** `I2S_CONF_REG` bit 1 `RX_RESET` and bit 3 `RX_FIFO_RESET` map onto
  the shared RX reset bits. Bit 0 `TX_RESET` and bit 2 `TX_FIFO_RESET`
  (`i2s_reg.h:105-128`) do not touch the receiver. All four reset bits are
  plain R/W and read back their written values. Bit 5 `RX_START` and bit 7
  `RX_SLAVE_MOD` map as before.
- **Slot selection.** `RX_CHAN_MOD` (`I2S_CONF_CHAN_REG` bits 4:3,
  `i2s_reg.h:692-695`) 1 or 2 selects one slot. `RX_MSB_RIGHT` (CONF bit 17,
  reset 1, `i2s_reg.h:21-23`) decides which one. `i2s_ll_rx_select_std_slot`
  (`i2s_ll.h:859-875`) writes RIGHT as `msb_right ? 1 : 2` and LEFT as
  `msb_right ? 2 : 1`, and IDF's standard slot defaults set `msb_right` only for
  16-bit data (`esp_driver_i2s/include/driver/i2s_std.h:40-41`). CONF resets to
  `0x30300`.
- **Sample containers.** Classic keeps 24-bit samples in 32-bit words:
  `i2s_ll_rx_set_sample_bit` sets RX_FIFO_MOD 2 above 16 bits
  (`i2s_ll.h:676-680`), and `i2s_get_buf_size` allocates
  `((bits + 15) / 16) * 2` bytes per sample on ESP32
  (`esp_driver_i2s/i2s_common.c:447-448`). The adapter therefore sets #201's
  RX_24_FILL_EN for 24-bit RX.
- **Interrupts.** The shared channel's IN_DONE, IN_SUC_EOF, IN_DSCR_ERR and
  IN_DSCR_EMPTY become classic INT_RAW bits 8, 9, 13 and 15
  (`i2s_reg.h:142-184`). IN_SUC_EOF also latches `I2S_IN_EOF_DES_ADDR_REG`
  (+0x3c). IN_DSCR_EMPTY uses the shared channel's condition: samples remain
  after the inlink ended. That this matches the classic bit's meaning, including
  raising IN_DSCR_ERR when samples remain at inlink end, is inferred.
- **EOF.** `RXEOF_NUM` (+0x24, reset 64, `i2s_reg.h:676-682`) counts words
  (`i2s_ll.h:639-643`). Byte counts above `u32::MAX` raise IN_DSCR_ERR instead of
  wrapping.

## Idle structure and static check

Classic fields are appended. Receiver state and the PCM bank use #202's boxed
per-controller state (the bank on controller 0), allocated on host access. The
shared queue advances lazily on RX access or `attach_pcm_sources()`; there is no
per-tick source scan. An inline DPORT clock/channel gate guards receive work.
No per-tick work is added to S3/C3/C6.

Static check (no timing): Rust 1.99.0 fat-LTO release `esp32sim` binaries
from upstream main `bb2b33d5` and this rebased stack were compared using the
committed EX230 `asmcmp.py` and `hot.py` selector. Of 119 selected S3/C3/C6
hot functions, 111 match, eight differ and none are missing; hashes and
source revisions are in [codegen.json](codegen.json).

- C3/C6 `i2s_receive` and S3 `dma_i2s_port` materialize the explicit BCLK/WS
  signal indices for classic's non-adjacent matrix signals. Only active RX
  executes this work; the other chips retain their original signal values.
- Shared I2C tick loses six instructions in bounds-panic tails because the
  macro expansion folds their source sites; normal byte stepping and the
  idle entry match. `RtcCntl::write` has inherited classic-stack codegen
  differences (five fewer instructions).
- The three `apply_due_script_events` functions include the classic touch
  event variant. Their no-event execution loops do not change.

Selected run/step, bus tick/deadline/MMIO dispatch, shared GDMA, interrupt
refresh and other peripheral tick paths match. This is not a claim that
all active-device instructions match, and no new CPU timing was collected.

## Sources and limits

Classic register definitions: ESP-IDF v5.5.4
`components/soc/esp32/register/soc/i2s_reg.h` at the lines above;
`include/soc/gpio_sig_map.h:69-72,299,309-312,343` supplies I2S0/1 input data and
BCLK/WS indices; `include/soc/interrupts.h:50-51` supplies sources 32/33;
`register/soc/dport_reg.h:947,965` supplies the I2S clock gates. EX214/EX215 cite
IDF v5.5.5 for the S3/C3/C6 layouts; classic uses the v5.5.4 headers matching its
inherited fixture. No IDF 4.4 validation is claimed.

Supported: standard 16/24/32-bit RX with the PLL clock. Not implemented: APLL,
external slave clocking, ADC-to-I2S capture, serial edge timing, classic I2S TX,
`RX_RIGHT_FIRST` (stereo frames are always written left first), LC_CONF
`IN_RST`/`AHBM_RST` and `INLINK_RESTART` continuing from the current descriptor
(a restart reloads `INLINK_ADDR`). Routing follows the shared matrix contract and
does not validate IO_MUX/pad enable. A route that matches no source slot reads the
per-controller input, as #202 specifies. Host source and queued PCM state survive
reset. No hardware microphone or audio-fidelity claim is made. A possible follow-up is
to share `DramDma::reaches` with classic SPI and Wi-Fi; their current DMA reach
rules are unchanged here. The inherited
classic fixture/ROM hashes are in [EX230](../classic-core/README.md). I2S tests
generate their PCM samples and descriptors in memory; no firmware, external input
or private capture is involved.

## Verification

Commands: the full [EX230 Rust 1.99.0 set](../classic-core/README.md), with the
validated source revision and counts in [checks.json](checks.json): both
Clippy targets with warnings denied, empty-HOME CI-mode
(`ESP32SIM_ROM_DIR=web/wasm/fw`, `--include-ignored --skip external_`) and plain
release workspace tests, production WASM build and the eight required scenarios,
and evidence privacy.



[Mutation table](mutations.json): 32 single replacements, all killed by assertion
failures. For each row, apply the replacement, run
`cargo +1.99.0 test --release -p esp32 --test TARGET TEST` (or `--lib TEST`
when the row has no target), then restore the source.

Regressions specific to the Oct 11 review:
`i2s_two_descriptor_ring_keeps_running_after_owner_writeback` runs six passes over
a two-descriptor circular ring with `owner = 1` set once, then shows CHECK_OWNER
stopping it. `i2s_rejects_empty_unowned_with_check_owner_and_non_dram_descriptors`
covers size 0, an unowned descriptor with CHECK_OWNER set, DROM/IRAM/RTC
descriptors and buffers, a buffer straddling DRAM's end, and acceptance of an
unowned descriptor at the reset value.
`i2s_inlink_offset_into_registers_does_not_touch_uart_fifo` checks that the UART0
RX FIFO keeps its bytes and nothing is transmitted.
`rx_fifo_reset_is_conf_bit_3_not_tx_bit_2`, `rx_chan_mod_selects_slot_by_msb_right`
and `classic_24_bit_samples_fill_words` pin the CONF mapping.

## CPU comparison

Not re-measured on this head. An earlier head of branch `esp32-classic-host`,
`schematik-engineering/esp32sim@bcd4df7e`, was measured against main `2f9443a9`.
It predates the rebase onto `eec7b2c9`, the updated #201/#202 and the Oct 11
review commits. Classic code is unused in these workloads. The static check above
covers the bb2b33d5-based source without making a new timing claim.

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 2f9443a9 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.2784 (3.0442–3.3351) | 3.2612 (3.0412–3.3331) | -0.52% | 2/7 | 4800000000 | 5.27 |
| Pocket Tank | 35.2968 (34.6376–38.3461) | 35.0204 (34.7165–38.7101) | -0.78% | 3/7 | 10073833665 | 4.26 |
| S3 hello | 24.6510 (23.7365–32.0593) | 24.3939 (23.8923–27.6721) | -1.04% | 4/7 | 1789819657 | 4.96 |
| C3 hello | 2.3837 (2.3635–2.5350) | 2.4178 (2.3742–2.5332) | +1.43% | 5/7 | 4800000000 | 3.53 |

### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.311011 | 3.280495 | 4.96/4.96/4.96 | 4.78/4.96/4.96 | accepted |
| 1 | 2 | M→B | 3.335100 | 3.309226 | 4.96/4.80/4.96 | 4.80/4.89/4.89 | accepted |
| 2 | 3 | B→M | 3.326745 | 3.261224 | 4.89/5.06/5.06 | 4.89/4.89/4.89 | accepted |
| 3 | 4 | M→B | 3.278430 | 3.333128 | 4.73/4.51/4.73 | 4.51/4.51/4.51 | accepted |
| 4 | 5 | B→M | 3.300833 | 3.297490 | 5.27/5.01/5.27 | 4.51/5.27/5.27 | accepted |
| 5 | 6 | M→B | 3.243410 | 3.127133 | 4.40/4.40/4.40 | 4.40/4.45/4.45 | accepted |
| 6 | 7 | B→M | 3.044205 | 3.072730 | 4.41/4.41/4.41 | 4.45/4.41/4.45 | accepted |
| 7 | 8 | M→B | 3.064612 | 3.041217 | 4.41/4.54/4.54 | 4.54/4.26/4.54 | accepted |

### Pocket Tank

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 34.873879 | 35.176084 | 3.07/2.66/3.07 | 4.26/3.07/4.26 | accepted |
| 1 | 2 | M→B | 35.670774 | 34.777742 | 2.66/3.12/3.24 | 3.12/3.13/3.57 | accepted |
| 2 | 3 | B→M | 34.967871 | 34.716506 | 2.39/2.89/2.89 | 3.13/2.39/3.13 | accepted |
| 3 | 4 | M→B | 34.960377 | 34.809702 | 2.89/3.09/3.19 | 3.09/3.12/3.22 | accepted |
| 4 | 5 | B→M | 34.637622 | 35.020398 | 3.22/3.12/3.22 | 3.12/3.22/3.32 | accepted |
| 5 | 6 | M→B | 35.296837 | 38.710090 | 3.12/3.14/3.17 | 3.14/3.14/3.35 | accepted |
| 6 | 7 | B→M | 35.409490 | 38.432319 | 3.33/3.56/3.56 | 3.14/3.33/3.53 | accepted |
| 7 | 8 | M→B | 38.346102 | 35.122780 | 3.56/3.14/3.63 | 3.14/2.68/3.14 | accepted |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 24.392411 | 23.938263 | 2.33/2.64/2.64 | 2.68/2.33/2.68 | accepted |
| 1 | 2 | M→B | 24.650971 | 27.672066 | 2.64/2.75/2.84 | 2.75/3.50/3.58 | accepted |
| 2 | 3 | B→M | 23.921096 | 24.166101 | 3.45/3.25/3.69 | 3.50/3.45/3.50 | accepted |
| 3 | 4 | M→B | 27.041111 | 25.220509 | 3.25/3.38/3.38 | 3.38/3.19/3.38 | accepted |
| 4 | 5 | B→M | 32.059315 | 24.092009 | 2.65/3.59/3.73 | 3.19/2.65/3.19 | accepted |
| 5 | 6 | M→B | 28.264140 | 24.393876 | 3.59/3.63/4.50 | 3.63/3.61/3.82 | accepted |
| 6 | 7 | B→M | 32.061965 | 54.232288 | 24.96/16.14/24.96 | 3.61/24.96/25.74 | discarded: load >7 |
| 6 | 8 | B→M | 23.736473 | 23.892318 | 4.21/3.30/4.21 | 4.96/4.21/4.96 | accepted |
| 7 | 9 | M→B | 23.980689 | 25.012749 | 3.30/2.95/3.30 | 2.95/2.93/2.95 | accepted |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.499618 | 2.457794 | 2.93/2.93/2.93 | 2.93/2.93/2.93 | accepted |
| 1 | 2 | M→B | 2.376265 | 2.533197 | 2.93/2.94/2.94 | 2.94/2.94/2.94 | accepted |
| 2 | 3 | B→M | 2.383738 | 2.391059 | 2.78/2.78/2.78 | 2.94/2.78/2.94 | accepted |
| 3 | 4 | M→B | 2.388470 | 2.374218 | 2.78/2.88/2.88 | 2.88/2.88/2.88 | accepted |
| 4 | 5 | B→M | 2.363515 | 2.404074 | 2.89/2.89/2.89 | 2.88/2.89/2.89 | accepted |
| 5 | 6 | M→B | 2.535040 | 2.508027 | 2.89/3.14/3.14 | 3.14/3.14/3.14 | accepted |
| 6 | 7 | B→M | 2.412236 | 2.417849 | 3.53/3.53/3.53 | 3.14/3.53/3.53 | accepted |
| 7 | 8 | M→B | 2.369542 | 2.431076 | 3.53/3.40/3.53 | 3.40/3.40/3.40 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above.


## Rebase qualification on bb2b33d5

Classic I2S CONF reset bits are plain R/W in IDF v5.5.4 i2s_reg.h, so read(8) retains their written values. An assertion test pins all four bits. IN_DSCR_ERR on inlink end with samples left is inferred, as well as IN_DSCR_EMPTY/stop behavior; no silicon claim is made. Moving DramDma::reaches into the bus and sharing it with SPI/Wi-Fi is a possible follow-up, not included here.

The host I2S and raw-ADC fixtures now advance explicit 240 MHz bus-reference
ticks through the core's `cpu_cycles_for_bus` conversion, including after reset.
Their sample, interrupt and descriptor expectations are unchanged; reset-time
XTAL CPU clocks are no longer mistaken for reference ticks. The runtime I2S
step receives elapsed reference ticks from the corrected classic bus.

Rust 1.99.0: both Clippy targets pass; empty-HOME CI-policy 938 passed/0 failed/0 ignored; plain 910 passed/0 failed/46 ignored; production WASM build, eight demos and privacy pass. `checks.json` identifies the exercised source revision; subsequent receipt changes are documentation only.
