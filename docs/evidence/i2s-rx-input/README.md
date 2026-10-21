# EX214: I2S receive input

Original correctness base: upstream main `fe3a9c08`. EX214, merged in #201,
ports controller-bound stereo PCM input onto main's camera/crypto GDMA receive path.
It retains the existing C3 GDMA adapter; there is exactly one register-layout
translation. EX212 owns the existing scatter semantics. EX214 adds another producer
and moves that walker into `esp-periph` so all three chips use it. The later CPU
comparison below measures merged head `baf9845b` against `eec7b2c9`.

S3 controllers 0 and 1 and C3/C6 controller 0 accept 16-bit host frames. Standard
RX packs mono/stereo 16-bit words or left-aligned 24/32-bit samples. 24-bit samples
take 3 bytes, or 4 when the firmware sets RX_24_FILL_EN; 32-bit samples take 4. S3 controller
0 accepts converted PDM16. Queued input and generated tones survive chip reset.
Stopped receivers and unbound DMA channels do not consume the per-port queue.
Unsupported slave, raw-PDM, extended TDM, companding and reordered formats do not
advance it. These are PCM-boundary model rules, not measurements of serial pins
or PDM filter behavior.

## Register sources

Compared with the public headers shipped in Arduino-ESP32 3.3.11, ESP-IDF v5.5.5:

- `components/soc/esp32s3/register/soc/i2s_reg.h`: RX_CONF at line 119,
  mode bits at 123–246, widths/divider at 375–416, clocks at 466–565,
  slots at 677–692 and byte-count EOF at 1058–1065.
- `components/soc/esp32c3/register/soc/i2s_reg.h`: standard RX mode fields
  at 117–230, widths/divider at 375–390 and slots at 657–660.
- `components/soc/esp32c6/register/soc/i2s_reg.h`: mode fields at 149–290,
  widths/divider at 424–478 and slots at 930–945. Its clocks instead live in
  `components/soc/esp32c6/register/soc/pcr_reg.h`, lines 719–784.
- RX_24_FILL_EN, RX_CONF bit 16, reset 0, "1: store 24 channel bits to 32 bits.
  0: store 24 channel bits to 24 bits": esp32s3 `i2s_reg.h` lines 160–165, esp32c3
  lines 145–150, esp32c6 lines 238–244. ESP-IDF v5.5.5 never writes it; its
  `components/esp_driver_i2s/i2s_common.c` line 451 (`i2s_get_buf_size`) sizes
  `(bits + 7) / 8` bytes per sample on S3/C3/C6, so 24-bit RX defaults to 3 bytes.
  Where the 24 bits sit inside a padded 32-bit word is not documented; the model
  left-aligns them (inferred), the same placement the TX pump reads.
- RX_EOF_NUM resets to 0x40: esp32s3 `i2s_reg.h` line 1059, esp32c3 line 1005,
  esp32c6 line 1226. The model uses the value as a byte count, as IDF's
  `i2s_ll_rx_set_eof_num` and `i2s_common.c` line 557 do.
- `components/hal/esp32s3/include/hal/i2s_ll.h`: RX clock selectors at 257,
  fractional divider programming at 387. C6 selectors are in its `i2s_ll.h`,
  lines 270–285. XTAL/PLL240/PLL160 selectors are 0/1/2; external is unsupported.
- `components/soc/esp32s3/register/soc/gdma_reg.h`: descriptor error,
  successful EOF and done bits at 115–146. C3's existing adapter maps the
  shared status into its combined interrupt register.

The focused tests program these registers directly. No new firmware fixture is
required, and this receipt makes no Arduino 2.x / IDF 4.4 compatibility claim.
Source hashes are in `headers.json`.

## Reproduction and verification

Fetch assets with `tools/fetch-demo-assets.sh --no-linux`. `checks.json` records
the required native/WASM Clippy, CI-policy workspace, plain workspace and eight
production WASM demo commands and exit statuses. Workspace runs use an empty
`HOME`; only the CI-policy run sets `ESP32SIM_ROM_DIR` to `web/wasm/fw`'s absolute
path. `CARGO_HOME` and `RUSTUP_HOME` retain access to the installed Rust 1.99.0
toolchain. No local firmware or private fixture is used.

Focused contracts live in `cli/tests/i2s_rx.rs`, `esp-periph/src/i2s/rx.rs`
and `esp-periph/tests/i2s_rx_contracts.rs`. After #202, `PcmInput` aliases
`PcmSource` in `esp-periph/src/i2s/sources.rs`; the four controller-input
mutations target that file and its `queue.frames` storage directly. Run the removal checks with:

```sh
python3 docs/evidence/i2s-rx-input/mutations.py
```

`mutations.json` is the mutation-to-killing-test table. The script requires a
clean source snapshot and restores each file before starting the next mutation.
All 46 mutations fail an assertion, rather than merely failing compilation.
Existing golden files remain byte-identical; none were regenerated.

Docs follow-up verification on merged main `ff64902a` plus this receipt/runner
update (Rust 1.99.0):

| Command | Result |
| --- | --- |
| `cargo +1.99.0 clippy --workspace --all-targets -- -D warnings` | PASS |
| `python3 docs/evidence/i2s-rx-input/mutations.py` | 46/46 applied and killed by test failures |
| `python3 docs/evidence/audio-sources-adc-stream/mutations.py` | 48/48 applied and killed by test failures |
| `cargo +1.99.0 test -p esp-periph --lib --test i2s_rx_contracts` | 60 unit + 3 contract tests passed |
| `cargo +1.99.0 test -p esp32sim --test i2s_rx --test pcm_sources --test adc_stream` | 5 I2S RX + 3 PCM source + 2 ADC stream tests passed |
| `node tools/check-evidence-privacy.mjs` | PASS |

Both regenerated mutation JSON files are unchanged. The full-workspace/WASM
results in `checks.json` are the original qualification, not rerun for this
docs-only follow-up. No new CPU benchmarks were run; the measurements below
are the recorded runs of the identified revisions.

## Shared GDMA descriptor reader

`esp-periph/src/gdma/receive.rs` holds the one GDMA descriptor reader for all chips
and DMA users. Other DMA code reuses it instead of adding a second reader or budget:

- `GDMA_DESCRIPTOR_STEP_BUDGET` (4096) is defined only here and re-exported from
  `esp_periph::gdma`.
- `DescriptorWalk::new(budget)` bounds one pump call. Every `read(bus, addr)` (through
  a `Bus`, unpriced) or `read_with(addr, word)` (any word source, for DMA memory held
  outside a `Bus`) costs one step. Once the budget is spent, reads return
  `StepBudgetExceeded` without touching memory. Both return the raw DW0 for write-back
  and the decoded `DmaDesc`.
- `GdmaInCh::scatter`/`receive` keep #190's semantics. When data remains and the
  inlink is 0, they set IN_DSCR_EMPTY (bit 4). They honour CHECK_OWNER (IN_CONF1 bit
  12). They close at byte-count EOF or `finish` (IN_SUC_EOF with the length written
  back), clear `running` at the end of the chain and report IN_DSCR_ERR on faults
  (`fail_receive`). `close` is the shared write-back.
- OUT gathers (S3 TX, SPI2 and crypto) use the same `DescriptorWalk`.

## CPU comparison

### Merged head baf9845b against main eec7b2c9

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Baseline eec7b2c9 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured head `baf9845be9250eed8d78f4b297347d25349d9bb2` (branch `i2s-rx-input`, merged in #201) against main `eec7b2c9`. User CPU seconds; B denotes the branch and M the baseline.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.0929 (3.0479–4.8467) | 3.0769 (3.0404–3.1138) | -0.52% | 2/7 | 4800000000 | 4.57 |
| Pocket Tank | 35.1833 (34.9038–36.1175) | 35.2277 (34.7446–38.7771) | +0.13% | 3/7 | 10073833665 | 5.37 |
| S3 hello | 24.2008 (24.1281–24.4281) | 24.2654 (24.1591–25.0882) | +0.27% | 5/7 | 1789819657 | 5.36 |

#### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.090139 | 3.157381 | 4.19/4.01/4.19 | 4.19/4.19/4.19 | accepted |
| 1 | 2 | M→B | 3.047933 | 3.040404 | 4.01/4.01/4.01 | 4.01/4.01/4.01 | accepted |
| 2 | 3 | B→M | 3.064688 | 3.094078 | 4.01/4.01/4.01 | 4.01/4.01/4.01 | accepted |
| 3 | 4 | M→B | 3.066563 | 3.076993 | 4.01/4.01/4.01 | 4.01/4.01/4.01 | accepted |
| 4 | 5 | B→M | 4.846669 | 3.113836 | 4.01/4.57/4.57 | 4.01/4.01/4.01 | accepted |
| 5 | 6 | M→B | 3.279577 | 3.055250 | 4.57/4.52/4.57 | 4.52/4.52/4.52 | accepted |
| 6 | 7 | B→M | 3.109558 | 3.076877 | 4.32/4.29/4.32 | 4.52/4.32/4.52 | accepted |
| 7 | 8 | M→B | 3.092871 | 3.071213 | 4.29/4.29/4.29 | 4.29/4.19/4.29 | accepted |

#### Pocket Tank

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 39.072783 | 35.405438 | 4.17/6.16/7.92 | 4.19/4.17/4.33 | discarded: load >7 |
| warmup | 2 | B→M | 35.307844 | 35.225801 | 5.05/4.76/5.05 | 4.81/5.05/5.37 | accepted |
| 1 | 3 | M→B | 35.183316 | 36.523128 | 4.76/4.87/4.87 | 4.87/4.60/5.15 | accepted |
| 2 | 4 | B→M | 36.117539 | 35.269917 | 4.39/4.51/4.74 | 4.60/4.39/4.84 | accepted |
| 3 | 5 | M→B | 35.681648 | 35.227679 | 4.51/4.23/4.51 | 4.23/4.49/4.53 | accepted |
| 4 | 6 | B→M | 36.016530 | 38.777127 | 4.78/5.26/5.26 | 4.49/4.78/5.11 | accepted |
| 5 | 7 | M→B | 34.903791 | 34.744562 | 4.53/4.09/4.53 | 4.09/3.88/4.09 | accepted |
| 6 | 8 | B→M | 35.053234 | 34.808503 | 3.76/3.84/3.84 | 3.88/3.76/3.88 | accepted |
| 7 | 9 | M→B | 34.906288 | 35.074352 | 3.84/4.35/4.40 | 4.35/4.44/4.57 | accepted |

#### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 24.206321 | 24.370651 | 4.57/4.48/4.57 | 4.44/4.57/4.57 | accepted |
| 1 | 2 | M→B | 24.200792 | 24.265441 | 4.48/4.18/4.48 | 4.18/3.95/4.23 | accepted |
| 2 | 3 | B→M | 24.133667 | 24.201382 | 4.17/4.57/4.62 | 3.95/4.17/4.27 | accepted |
| 3 | 4 | M→B | 24.128088 | 24.380274 | 4.57/4.63/4.64 | 4.63/4.19/4.63 | accepted |
| 4 | 5 | B→M | 24.172214 | 24.159122 | 4.25/4.44/4.69 | 4.19/4.25/4.42 | accepted |
| 5 | 6 | M→B | 24.267293 | 24.334166 | 4.44/4.48/4.66 | 4.48/4.16/4.60 | accepted |
| 6 | 7 | B→M | 24.428119 | 25.088182 | 4.94/5.36/5.36 | 4.16/4.94/4.94 | accepted |
| 7 | 8 | M→B | 24.302456 | 24.194735 | 4.93/4.62/4.93 | 4.62/4.65/4.70 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Only the tables are retained in this receipt; raw output files and per-second load series are not committed. I2S TX was not timed.

### Earlier run: c4ffbf4d against main 954f2a68

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none. Pocket Tank and I2S TX workloads were not timed in this earlier run. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `schematik-engineering/esp32sim@c4ffbf4dc0460a24e2a3c220240a4fd4821f6868` (branch `i2s-rx-input`) against main `954f2a68`. The branch was then rebased onto `fe3a9c08` without changing its own diff. User CPU seconds.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.2518 (3.2310–3.3164) | 3.3031 (3.2661–3.4019) | +1.58% | 5/7 | 4800000000 | 2.90 |
| S3 hello | 24.9721 (24.7991–25.3546) | 24.9517 (24.7022–26.6243) | -0.08% | 3/7 | 1789819657 | 2.47 |
| C3 hello | 2.4401 (2.4295–2.4881) | 2.4553 (2.4103–2.4856) | +0.62% | 4/7 | 4800000000 | 1.89 |

#### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.317685 | 3.617584 | 2.75/2.75/2.75 | 2.90/2.75/2.90 | accepted |
| 1 | 2 | M→B | 3.253574 | 3.401929 | 2.75/2.61/2.75 | 2.61/2.56/2.61 | accepted |
| 2 | 3 | B→M | 3.251834 | 3.326952 | 2.56/2.43/2.56 | 2.56/2.56/2.56 | accepted |
| 3 | 4 | M→B | 3.316397 | 3.303052 | 2.43/2.32/2.43 | 2.32/2.32/2.32 | accepted |
| 4 | 5 | B→M | 3.233875 | 3.358455 | 2.21/2.51/2.51 | 2.32/2.21/2.32 | accepted |
| 5 | 6 | M→B | 3.277006 | 3.266109 | 2.51/2.51/2.51 | 2.51/2.39/2.51 | accepted |
| 6 | 7 | B→M | 3.251333 | 3.298689 | 2.44/2.44/2.44 | 2.39/2.44/2.44 | accepted |
| 7 | 8 | M→B | 3.230994 | 3.287996 | 2.44/2.33/2.44 | 2.33/2.30/2.33 | accepted |

#### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 24.789521 | 25.202236 | 2.05/1.88/2.05 | 2.30/2.05/2.30 | accepted |
| 1 | 2 | M→B | 25.284845 | 24.757919 | 1.88/1.96/2.05 | 1.96/1.70/1.96 | accepted |
| 2 | 3 | B→M | 24.814118 | 24.762465 | 1.60/1.54/1.60 | 1.70/1.60/1.71 | accepted |
| 3 | 4 | M→B | 24.972055 | 25.579252 | 1.54/1.49/1.58 | 1.49/1.75/1.75 | accepted |
| 4 | 5 | B→M | 24.799061 | 26.624315 | 1.87/2.47/2.47 | 1.75/1.87/1.95 | accepted |
| 5 | 6 | M→B | 24.930923 | 25.166946 | 2.47/2.19/2.47 | 2.19/1.86/2.19 | accepted |
| 6 | 7 | B→M | 25.354633 | 24.702150 | 1.69/1.66/1.69 | 1.86/1.69/1.86 | accepted |
| 7 | 8 | M→B | 25.220010 | 24.951730 | 1.66/1.57/1.66 | 1.57/1.89/1.89 | accepted |

#### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.424768 | 2.423405 | 1.89/1.82/1.89 | 1.89/1.89/1.89 | accepted |
| 1 | 2 | M→B | 2.451351 | 2.467182 | 1.82/1.82/1.82 | 1.82/1.75/1.82 | accepted |
| 2 | 3 | B→M | 2.488115 | 2.455299 | 1.75/1.69/1.75 | 1.75/1.75/1.75 | accepted |
| 3 | 4 | M→B | 2.453692 | 2.485630 | 1.69/1.69/1.69 | 1.69/1.72/1.72 | accepted |
| 4 | 5 | B→M | 2.433062 | 2.483267 | 1.72/1.66/1.72 | 1.72/1.72/1.72 | accepted |
| 5 | 6 | M→B | 2.436326 | 2.421284 | 1.66/1.66/1.66 | 1.66/1.69/1.69 | accepted |
| 6 | 7 | B→M | 2.440081 | 2.410282 | 1.69/1.71/1.71 | 1.69/1.69/1.69 | accepted |
| 7 | 8 | M→B | 2.429529 | 2.443767 | 1.71/1.71/1.71 | 1.71/1.65/1.71 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above. Only the tables above are retained.

Idle structure, for firmware that never enables I2S RX:

- Receiver queue, frame phase and sample buffer sit behind one lazily allocated
  pointer; an unused controller allocates no receiver state. C3 and C6 hold the
  whole controller in a `Box`.
- S3: one cached flag per controller (TX or RX enabled, refreshed on RX_CONF and
  TX_CONF writes) gates the per-flush DMA pump and the active-cadence check. It
  replaces the TX-only test there, so an idle flush does main's work.
  RX_CONF/TX_CONF are crate-private so nothing bypasses the flag; the existing
  S3 cadence test now enables TX by register write instead of a field poke.
- C3/C6: an RX_CONF write returns a write effect. The SoC tests it in the same
  mask test as the existing SPI effect, and only then refreshes pending work.
  C6's pending-work flag replaces its SPI check in the device step. C3 keeps its
  existing per-write pending-work refresh, with one cached boolean term added.
- Remaining idle work: a C3/C6 deadline query tests one cached boolean (C3
  `i2s_rx`, C6 `work_pending`) before applying the 256-cycle RX bound.

Disassembly of `cargo +1.99.0 build --release --bins` output (fat LTO) via
`objdump -d --disassemble-symbols=<symbol>`, against `954f2a68`:

| Function | Instructions | Difference |
| --- | ---: | --- |
| C3 `SocBus::tick` | 279 → 282 | alignment padding only |
| C6 `SocBus::tick` | 1968 → 1968 | SPI branch outlined; two field offsets +8 bytes |
| S3 `SocBus::tick_impl` | 3648 → 3600 | — |
| S3 `SocBus::refresh_tick_budget` | 280 → 279 | — |
| C3 `next_deadline` | 185 → 200 | byte load and branch on the idle path |
| C6 `next_deadline` | 285 → 290 | byte load and branch on the idle path |
| C6 `SocBus::periph_write` | 1594 → 1656 | I2S block arm; RX_CONF shares the SPI effect test |
| S3 `SocBus::periph_write_inner` | 1632 → 1664 | field offsets +16 bytes; I2S write arms longer |

October 11 review changes (24-bit packing, RX_EOF_NUM reset, `read_with`) against
the measured code (`52d09c73`, same diff as `c4ffbf4d`): normalised release disassembly
is identical for 1944 of 1948 functions, including S3 `tick_impl`, `dma_i2s_port` and
`refresh_tick_budget` and C3/C6 `tick` and `next_deadline`. The differences are
`I2s::rx_data`, which runs only while RX is active, and the three `Peripherals::new`
constructors (RX_EOF_NUM default).

## Limits

No microphone firmware golden, hardware audio capture, pin serializer, filter,
slave clock or audio-fidelity claim. The tests pin exact sample bytes, EOF,
ownership, channel errors and reset behavior. Existing full-run goldens cover
unrelated firmware. Source snapshots and result rows omit local paths and
machine identity; no raw capture was retained or redacted.
