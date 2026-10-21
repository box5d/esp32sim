# EX220: C3/C6 SHA DMA and C6 Wi-Fi calibration/ECC

Base: upstream main `ff64902a`, which includes merged #201 and #202.
The source is the commits on this branch. This change
extends EX202's station contract to unstubbed C6 PHY startup and TLS accelerator
use. EX210's shared StationLink from merged #193 remains the only Wi-Fi link.
S3, C3 and C6 share SHA DMA execution. Their crypto OUT gathers read descriptors only
through #201's shared walker (see [Shared descriptor reader](#shared-descriptor-reader));
this branch adds no descriptor reader or step budget of its own. Camera receive DMA and
the Wi-Fi frame transport are unchanged.

## Contract and result

The committed Arduino-ESP32 3.3.11 / ESP-IDF 5.5.5 firmware obtains
`10.0.2.15` on C3 and C6 without stubs or an application ELF. Two Mbed TLS peers
complete TLS 1.2 ECDHE-PSK with P-256 and AES-128-CBC-SHA256, then send and check
all 256 byte values in each direction. Memory queues carry TLS records;
`--net none` provides the existing local DHCP service. No host endpoint is used.
C6's three existing `pll_cal exceeds 2ms` warnings remain visible in its golden.

The new goldens pin the complete console, `[emu] stop` exception/interrupt totals,
per-source interrupt counts, and Wi-Fi/network totals. The old station goldens
remain byte-identical. Their C6 fixture still uses its existing `bb_init` stub;
the new firmware supplies the unstubbed proof. No cycle-derived count is added
as a TLS correctness oracle.

SHA handles split descriptors, bounded work, START versus CONTINUE, either
SHA/GDMA arm order, CHECK_OWNER, ownership return and DONE/EOF/error state.
Bad or short input is not hashed. As in the existing S3 reader, a failure may
have returned ownership for descriptors already consumed. C3/C6 errors clear
BUSY, stop the channel and assert OUT_DSCR_ERR. Their descriptors and payloads
must be wholly in SRAM. There is no MMIO fallback that can recursively invoke
another peripheral transfer.

### Shared descriptor reader

`gather_out` (`esp-periph/src/gdma/transmit.rs`) is the OUT side of #201's shared GDMA
code. Each descriptor is read with `DescriptorWalk::read_with`, so decoding, the per-call
`GDMA_DESCRIPTOR_STEP_BUDGET` of 4096 descriptors and the read faults are #201's. The
gather adds OUT bookkeeping only: a visited set that rejects cycles even without
CHECK_OWNER, the CHECK_OWNER test (OUT_CONF1 bit 12), the buffer copy, owner write-back,
OUT_DONE per descriptor, and OUT_EOF plus the EOF descriptor address at SUC_EOF. A fault
returns the walker's fault type and keeps the bytes gathered so far.

Memory is behind a small trait. The S3 adapter uses main's unpriced bus reads and writes
and mapped-buffer copy, and reaches the channel through the bus, so a write-back that
lands on a GDMA register keeps its side effect. S3 SHA and AES therefore keep the
completion the maintainer checked on the previous head: the same 4096-descriptor
budget, OUT_DSCR_ERR and `running = false` on any fault, and `irq_dirty` set after the
step. The C3/C6 adapter (`SramOut`) reads and writes only data SRAM; every word, buffer
and write-back is bounds-checked, and an address outside SRAM is a fault, never MMIO.

ECC supports P-256 scalar multiplication, point verification and combined
verify/multiply, parameter RAM, reset, raw/masked interrupt state and W1C.
VERIFICATION_RESULT changes only in the verification modes (2 and 3); plain point
multiplication (mode 0) leaves it as it was. The header defines the bit as the
verification result, valid when a calculation is done (`ecc_mult_reg.h:111-113`);
that mode 0 leaves it unchanged is **inferred**. The PCR reset pulse that
`ecc_ll_reset_register` issues (PCR ECC_CONF bit 1) returns the accelerator to its
reset state; a PCR write without the bit, such as the bus-clock enable, does not.
Noncanonical coordinates are rejected, including a point congruent modulo p
to a valid point. Unsupported modes and key lengths do not manufacture completion. Arithmetic
uses the existing big-integer multiplication, reduction and exponentiation.
[OpenSSL reproduction](verify-ecc.py) independently confirms the nontrivial
scalar vectors in [ecc-vectors.json](ecc-vectors.json).

## Register sources

All paths below are in public ESP-IDF **v5.5.5**. The new fixture uses that
version; the older station fixture uses **v5.5.4**. [Header hashes](headers.json)
compare both tags. Register layouts and the SHA HAL are identical. The ECC HAL
adds a power-up assertion and updates its copyright year in 5.5.5; its mode,
parameter and interrupt operations are unchanged. IDF 4.4 is not a fixture input.

* `components/soc/esp32c3/include/soc/hwcrypto_reg.h:42-58` and
  `components/soc/esp32c6/register/soc/sha_reg.h:50-120`: SHA modes, six-bit
  block count, BUSY, DMA_START and DMA_CONTINUE.
* `components/soc/{esp32c3,esp32c6}/include/soc/gdma_channel.h:15`: SHA trigger 7.
  `components/soc/esp32c3/register/soc/gdma_reg.h:1621-1627` and the C6 header
  at `1710-1716`: OUT_CHECK_OWNER bit 12. C3's existing adapter translates
  its combined interrupt bits to the shared OUT_DONE/EOF/DSCR_ERR bits 0/1/2.
  C6's raw OUT interrupt layout is in `gdma_reg.h:691-723`.
* `components/hal/include/hal/dma_types.h`: descriptor size/length, EOF, owner,
  buffer and next-link layout.
* `components/soc/esp32c6/register/soc/ecc_mult_reg.h:14-154`: raw/status/enable/
  clear offsets 0x0c/0x10/0x14/0x18, interrupt bit 0, CONF fields and parameter RAM.
  `components/hal/esp32c6/include/hal/ecc_ll.h:73-90`: work modes 0/2/3.
  `components/soc/esp32c6/include/soc/interrupts.h:98`: ECC source 76.
* `components/soc/esp32c3/register/soc/reg_base.h:12-16` and the C6 header
  at `39-44`: the SHA/GDMA bases and C6 ECC_MULT base 0x6008b000.
* `components/soc/esp32c6/register/soc/ecc_mult_reg.h:111-113`: VERIFICATION_RESULT,
  RO/SS, bit 8. `components/soc/esp32c6/register/soc/pcr_reg.h:1322-1336`: PCR
  ECC_CONF at offset 0xd8, ECC_CLK_EN bit 0, ECC_RST_EN bit 1.
  `components/hal/esp32c6/include/hal/ecc_ll.h:40-44`: the reset pulse (1, then 0).

### IDF 4.4 (Arduino-ESP32 2.x) on C3

Arduino-ESP32 2.x builds C3 firmware with ESP-IDF 4.4. The C3 SHA/GDMA layout this
model uses is the same in IDF **v4.4.8**; no 4.4 firmware fixture is part of this receipt.

* `components/soc/esp32c3/include/soc/hwcrypto_reg.h:53-65`: SHA_MODE 0x00,
  BLOCK_NUM 0x0c, START 0x10, CONTINUE 0x14, BUSY 0x18, DMA_START 0x1c,
  DMA_CONTINUE 0x20, H 0x40, TEXT 0x80, as in 5.5.5 (the file differs only in its
  licence header and AES registers).
* `components/soc/esp32c3/include/soc/soc.h:33,37`: SHA base 0x6003b000, GDMA base
  0x6003f000. `components/soc/esp32c3/include/soc/gdma_channel.h:23`: SHA trigger 7.
* `components/soc/esp32c3/include/soc/gdma_reg.h`: OUT_CONF0 0xd0 with OUT_AUTO_WRBACK
  bit 2 (`1291`, `1316`), OUT_CONF1 0xd4 with OUT_CHECK_OWNER bit 12 (`1333`, `1336`),
  OUT_LINK 0xe0 (`1399`), OUT_PERI_SEL 0x100 (`1504`), and OUT_DONE/OUT_EOF/OUT_DSCR_ERR
  interrupt bits 3/4/6 (`89`, `82`, `68`), which the existing C3 adapter maps.
* `components/esp_rom/include/esp32c3/rom/lldesc.h:67-79`: `lldesc_t` has the same
  size/length/EOF/owner/buffer/next layout as 5.x `dma_descriptor_t`.

The 4.4 driver differs in configuration, not layout. `mbedtls/port/sha/dma/sha.c:290-334`
builds one or two `lldesc_t` (owner set, SUC_EOF on the last), arms GDMA first and then
writes DMA_START or DMA_CONTINUE; both arm orders are tested here.
`mbedtls/port/crypto_shared_gdma/esp_crypto_shared_gdma.c:96-127` connects trigger 7 and
starts the channel without applying a GDMA strategy, so CHECK_OWNER stays at its reset
value 0. The model treats CHECK_OWNER as optional and is tested both ways.

The C6 baseband offsets 0x418 and 0x810/0x814, start bit 0, TX DC done bit 22
and calibration done bits 14..16 are **inferred**, not public register facts.
They model the libphy polling contract and ideal zero comparator output.
Completion follows START synchronously and clears when START is cleared,
consistent with the existing IQ-status model. No silicon verification or
80-APB-tick analog latency is claimed.

## Inputs and reproduction

[inputs.json](inputs.json) records image sizes/hashes, SDK archive hashes,
tool versions and the retained link-map archive inventory. Both chips' app,
bootloader and partition table reproduce byte-for-byte from different source
and output directories. The [recipe](../../../examples/crypto-tls/README.md)
fixes the build epoch and maps paths before building. ELF debug information is
removed before hashing the ELF into the flash image. The public fixture's
source and complete linked-component notices accompany the binaries.

```sh
tools/fetch-demo-assets.sh --no-linux
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release \
  -p esp32sim --test goldens crypto_tls -- --ignored
python3 docs/evidence/c3-c6-sha-dma-wifi-cal/verify-ecc.py
python3 docs/evidence/c3-c6-sha-dma-wifi-cal/mutate.py
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" \
  python3 docs/evidence/c3-c6-sha-dma-wifi-cal/mutate.py --firmware
```

Run mutations without concurrent source edits or builds; each mutation restores
its source file before the next one. The machine has no role in the firmware's
protocol inputs. No default Rust toolchain is changed.

## Mutation table

[mutations.json](mutations.json) is the rule-removal table: exact source
replacement, command, failing test names and exit status for each mutation.
It includes both arm orders, hash state, bounds, ownership, interrupts, C6
calibration status, ECC arithmetic/verification/reset, the PCR reset and the live S3
writeback contract. Every retained mutation is killed by a test assertion, not a build
failure. [firmware-mutations.json](firmware-mutations.json) separately removes
C3 SHA DMA, C6 SHA DMA, C6 ECC, TX DC completion and calibration completion;
each prevents the corresponding firmware golden from passing.

| Removed rule | Test that fails |
| --- | --- |
| SHA start wiring C3 | `owner_check_is_optional_and_requested_length_bounds_input` |
| SHA start wiring C6 | `owner_check_is_optional_and_requested_length_bounds_input` |
| GDMA late start C3 | `sha_before_gdma_waits_for_channel_and_never_ticks` |
| GDMA late start C6 | `sha_before_gdma_waits_for_channel_and_never_ticks` |
| DMA first resets digest | `sha_before_gdma_waits_for_channel_and_never_ticks` |
| DMA continue retains digest | `dma_continue_preserves_the_previous_hash_state` |
| DMA initializes only the first block | `one_dma_request_hashes_every_block_without_reinitializing` |
| DMA consumes pending request | `dma_continue_preserves_the_previous_hash_state` |
| DMA clears busy | `invalid_chains_raise_descriptor_error_without_hashing` |
| DMA rejects short input | `invalid_chains_raise_descriptor_error_without_hashing` |
| DMA error interrupt | `cycle_detection_is_independent_of_owner_checking` |
| DMA error stops channel | `invalid_chains_raise_descriptor_error_without_hashing` |
| DMA owner check | `invalid_chains_raise_descriptor_error_without_hashing` |
| DMA check-owner gate | `owner_check_is_optional_and_requested_length_bounds_input` |
| DMA ownership return | `sha256_split_descriptor_completes_on_start_write` |
| DMA EOF interrupt | `sha256_split_descriptor_completes_on_start_write` |
| DMA DONE interrupt | `sha256_split_descriptor_completes_on_start_write` |
| DMA EOF descriptor | `sha256_split_descriptor_completes_on_start_write` |
| DMA cycle detection | `cycle_detection_is_independent_of_owner_checking` |
| DMA descriptor budget | `gdma::transmit::tests::zero_progress_chain_stops_at_the_shared_descriptor_budget` |
| DMA SRAM-only bounds | `gdma::transmit::tests::sram_bounds_owner_and_partial_output` |
| S3 writeback side effects | `bus::dma_tests::crypto_descriptor_writeback_preserves_mmio_side_effects` |
| TX DC completion | `txdc_comparator_outputs_ignore_software_writes` |
| Calibration completion | `txdc_and_calibration_status_follow_start_without_a_clock` |
| TX DC comparator mask | `txdc_comparator_outputs_ignore_software_writes` |
| ECC on-curve equation | `ecc::tests::verification_rejects_off_curve_and_noncanonical_points` |
| ECC START self-clear | `ecc::tests::scalar_results_match_independent_openssl_vectors` |
| ECC mode 0 keeps verification result | `ecc::tests::point_multiplication_leaves_the_verification_result` |
| ECC completion interrupt | `ecc::tests::noncanonical_point_congruent_to_valid_point_is_rejected` |
| ECC masked interrupt | `ecc_mmio_routes_cached_interrupts_without_clock_work` |
| ECC canonical coordinates | `ecc::tests::noncanonical_point_congruent_to_valid_point_is_rejected` |
| ECC unsupported key length | `ecc::tests::unsupported_modes_do_not_report_success_and_reset_clears_interrupts` |
| ECC scalar multiplication | `ecc::tests::scalar_results_match_independent_openssl_vectors` |
| ECC verification gates multiplication | `ecc::tests::noncanonical_point_congruent_to_valid_point_is_rejected` |
| ECC verify-only mode | `ecc::tests::verify_only_preserves_parameters_and_result_is_read_only` |
| ECC result read-only | `ecc::tests::verify_only_preserves_parameters_and_result_is_read_only` |
| ECC unsupported mode | `ecc::tests::unsupported_modes_do_not_report_success_and_reset_clears_interrupts` |
| ECC reset | `ecc::tests::unsupported_modes_do_not_report_success_and_reset_clears_interrupts` |
| ECC W1C | `ecc::tests::scalar_results_match_independent_openssl_vectors` |
| ECC MMIO mapping | `ecc_mmio_routes_cached_interrupts_without_clock_work` |
| PCR ECC reset pulse | `pcr_ecc_reset_pulse_clears_the_accelerator` |
| PCR reset dispatch | `pcr_ecc_reset_pulse_clears_the_accelerator` |
| I2S allocation prefix | `optional_devices_preserve_the_existing_i2s_indirection` |
| ECC interrupt routing | `ecc_mmio_routes_cached_interrupts_without_clock_work` |

## Verification

Final command results and test counts are in [checks.json](checks.json).
Native target: `aarch64-apple-darwin`; WASM target: `wasm32-unknown-unknown`.
Rust 1.99.0 is selected per command. Native and WASM Clippy deny warnings. Workspace checks run with a fresh HOME,
with the ROM directory supplied for the CI-policy suite and no emulator input
variables for the plain suite. CARGO_HOME and RUSTUP_HOME select build tools
outside that HOME. Tests receive no local firmware build, ELF or hardware input.
Only the new `crypto-tls-*` goldens are generated; existing goldens are unchanged.

The empty-home test commands, with only build-tool infrastructure retained:

```sh
mkdir -p target/ex220-home target/ex220-tmp
env -i PATH="$PATH" HOME="$PWD/target/ex220-home" TMPDIR="$PWD/target/ex220-tmp" \
  CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
  ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" \
  cargo +1.99.0 test --release --workspace -- --include-ignored --skip external_
env -i PATH="$PATH" HOME="$PWD/target/ex220-home" TMPDIR="$PWD/target/ex220-tmp" \
  CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
  cargo +1.99.0 test --release --workspace
```

Final results: **751** CI-policy tests pass; **728** plain tests pass with
**37** ignored. Both Clippy targets, the WASM build, all eight requested WASM
scenarios, evidence privacy, **44/44** rule mutations and **5/5** firmware
mutations pass.

## CPU comparison

### Restacked head versus parent #201

The central load-gated run of `schematik-engineering/esp32sim@3d9eee3c`
(branch `c3-c6-sha-dma-wifi-cal`) against parent
`schematik-engineering/esp32sim@baf9845b` found C6 hello **1.37% slower in
7/7 pairs**, 3.0448 → 3.0866 median user CPU seconds. Guest instruction counts
and console hashes matched. This historical result flagged an idle cost, not a pass or a
proven layout-only timing effect; the merged-head remeasurement below resolves it. C3 was −0.89% (1/7 slower), S3 +0.09%
(5/7), and Pocket Tank −0.36% (4/7). The samples and conditions are retained
in [cpu-parent.md](cpu-parent.md).

The size-neutral cold-device allocation below restores the parent's hot
layout and execution-loop code. **The maintainer closed the C6 idle-path check
after measuring merged #203.** Maintainer `joakimeriksson` compared main before #203
(`ff64902a`) with main after #203 (`a175e357`) in seven alternating, load-gated
pairs, using user CPU time and identical guest instructions and console SHA-256:

| Workload | Before median | After median | Change | After slower |
| --- | ---: | ---: | ---: | ---: |
| C6 hello | 3.50 s | 3.51 s | +0.3% | 3/7 |

These are the maintainer's reported values, not a new measurement here; raw
pair samples are not retained in this receipt. He confirmed that both his
earlier +1.2% (6/7) on `3d9eee3c` and the +1.37% (7/7) in
[cpu-parent.md](cpu-parent.md) are gone. The earlier 7/7 flag is resolved;
its historical measurements remain below and in that receipt.

### Historical pre-restack measurement

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Main 954f2a68 built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured on `schematik-engineering/esp32sim@e3aefb93` (branch `c3-c6-sha-dma-wifi-cal`, before the rebase onto #201 and the shared-walker change) against main `954f2a68`. User CPU seconds. These historical samples do not describe the restacked head or its layout fix.

| Workload | Main median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.3037 (3.2643–3.8944) | 3.3320 (3.2987–3.6422) | +0.86% | 5/7 | 4800000000 | 3.90 |
| C3 hello | 2.5193 (2.4666–2.7821) | 2.5483 (2.5192–2.7967) | +1.15% | 5/7 | 4800000000 | 4.31 |
| S3 hello | 25.0660 (24.5564–28.6332) | 24.7702 (24.5346–27.6966) | -1.18% | 2/7 | 1789819657 | 5.44 |
| Pocket Tank | 35.6546 (35.2570–36.5770) | 35.9444 (35.0265–37.9563) | +0.81% | 4/7 | 10073833775 | 5.54 |

### C6 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.282571 | 3.843635 | 2.81/3.07/3.07 | 2.89/2.81/2.89 | accepted |
| 1 | 2 | M→B | 3.270256 | 3.298681 | 3.07/3.07/3.07 | 3.07/3.14/3.14 | accepted |
| 2 | 3 | B→M | 3.298709 | 3.324059 | 3.13/3.13/3.13 | 3.14/3.13/3.14 | accepted |
| 3 | 4 | M→B | 3.264350 | 3.332014 | 3.13/3.04/3.13 | 3.04/2.96/3.04 | accepted |
| 4 | 5 | B→M | 3.306566 | 3.308866 | 2.96/3.20/3.20 | 2.96/2.96/2.96 | accepted |
| 5 | 6 | M→B | 3.303749 | 3.488665 | 3.20/3.10/3.20 | 3.10/3.10/3.10 | accepted |
| 6 | 7 | B→M | 3.894396 | 3.642231 | 3.26/3.64/3.64 | 3.10/3.26/3.26 | accepted |
| 7 | 8 | M→B | 3.631364 | 3.474084 | 3.64/3.64/3.64 | 3.64/3.90/3.90 | accepted |

### C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.485146 | 2.519265 | 4.31/4.31/4.31 | 3.90/4.31/4.31 | accepted |
| 1 | 2 | M→B | 2.519326 | 2.519179 | 4.31/4.29/4.31 | 4.29/4.29/4.29 | accepted |
| 2 | 3 | B→M | 2.480610 | 2.582611 | 4.10/4.10/4.10 | 4.29/4.10/4.29 | accepted |
| 3 | 4 | M→B | 2.466618 | 2.548317 | 4.10/4.18/4.18 | 4.18/4.18/4.18 | accepted |
| 4 | 5 | B→M | 2.661798 | 2.522541 | 4.08/4.08/4.08 | 4.18/4.08/4.18 | accepted |
| 5 | 6 | M→B | 2.782068 | 2.796716 | 4.08/4.07/4.08 | 4.07/4.07/4.07 | accepted |
| 6 | 7 | B→M | 2.681770 | 2.750300 | 4.07/3.90/4.07 | 4.07/4.07/4.07 | accepted |
| 7 | 8 | M→B | 2.503895 | 2.520618 | 3.90/3.90/3.90 | 3.90/3.75/3.90 | accepted |

### S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 25.091025 | 25.523782 | 3.79/3.71/3.92 | 3.75/3.79/3.85 | accepted |
| 1 | 2 | M→B | 25.119988 | 24.535718 | 3.71/3.09/3.71 | 3.09/2.88/3.09 | accepted |
| 2 | 3 | B→M | 25.288635 | 25.800910 | 2.98/3.00/3.40 | 2.88/2.98/2.98 | accepted |
| 3 | 4 | M→B | 24.910248 | 24.770172 | 3.00/2.65/3.00 | 2.65/2.28/2.68 | accepted |
| 4 | 5 | B→M | 28.633189 | 27.696565 | 4.74/5.32/5.44 | 2.28/4.74/4.74 | accepted |
| 5 | 6 | M→B | 24.578343 | 24.534623 | 4.59/4.30/4.59 | 4.30/3.52/4.43 | accepted |
| 6 | 7 | B→M | 24.556407 | 24.568998 | 3.30/2.96/3.43 | 3.52/3.30/3.52 | accepted |
| 7 | 8 | M→B | 25.065986 | 24.826962 | 2.96/2.84/2.96 | 2.84/3.55/3.55 | accepted |

### Pocket Tank

Status: PASS. Flags: —.

| Pair | Attempt | Order | Main s | PR s | Main load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 37.914729 | 47.451150 | 11.78/7.91/12.20 | 3.55/11.78/13.36 | discarded: load >7 |
| warmup | 2 | B→M | 36.409331 | 36.700058 | 4.84/3.59/4.84 | 4.81/4.84/4.84 | accepted |
| 1 | 3 | M→B | 36.576996 | 37.956348 | 3.59/4.22/4.42 | 4.22/4.12/4.51 | accepted |
| 2 | 4 | B→M | 36.456056 | 36.686776 | 5.63/15.07/16.13 | 4.12/5.63/5.69 | discarded: load >7 |
| 2 | 5 | B→M | 35.592760 | 35.944414 | 4.26/3.14/4.26 | 4.97/4.26/5.54 | accepted |
| 3 | 6 | M→B | 35.757709 | 35.162997 | 3.14/2.63/3.14 | 2.63/2.76/2.91 | accepted |
| 4 | 7 | B→M | 35.638270 | 35.026516 | 2.04/1.97/2.04 | 2.76/2.04/2.76 | accepted |
| 5 | 8 | M→B | 35.257001 | 35.958228 | 1.97/1.85/1.97 | 1.85/1.66/1.85 | accepted |
| 6 | 9 | B→M | 35.864536 | 35.436634 | 1.47/1.88/1.95 | 1.66/1.47/1.66 | accepted |
| 7 | 10 | M→B | 35.654557 | 36.878955 | 1.88/1.87/1.98 | 1.87/2.68/2.92 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above.

### Release disassembly and native layout

The current comparison builds upstream main `ff64902a` and this commit's
rebased source with
`cargo +1.99.0 build --release -p esp32sim --bins`, separate target directories,
`aarch64-apple-darwin`, and the workspace fat-LTO profile. Compared
`objdump -d --demangle --no-show-raw-insn` per function, normalizing instruction
addresses, branch/call targets, page offsets (including omitted zero offsets)
and nop/udf padding. ICF aliases
are matched by callee shape; field offsets and other immediates are retained.
[codegen.json](codegen.json) records the selected functions and `offset_of!`
layout probe. It also retains the earlier before/fixed comparison against
#201 (`schematik-engineering/esp32sim@baf9845b`).

The extra ECC box grew `Peripherals` from 8624 to 8632 bytes and `SocBus`
from 10176 to 10184 bytes. `SocBus.periph` remained at zero: this was size
growth, not a niche-driven relocation of the peripheral block. SRAM moved
8984 → 8992, cycles 10128 → 10136, and the enclosing machine fields moved by
eight bytes. ECC now shares the existing I2S allocation in a `repr(C)`
`OptionalPeripherals`, with I2S at offset zero. Both sizes and every probed
pre-existing field offset match main `ff64902a`; there is no new pointer,
extra I2S indirection, or periodic ECC work. Native layout and I2S-prefix
regression tests pin these properties. The source-level C6 I2S field is now
`periph.optional.i2s0`; its guest register interface is unchanged.

The current comparison reports 1932 identical and 15 differing common
demangled function entries against main `ff64902a`. The historical #201
comparison was 1868 identical / 65 differing before the layout fix and
1920 / 13 afterwards. Four of the current differing entries combine multiple HashMap
monomorphizations under one demangled name; those counts do not establish
four distinct changed hot functions.

* **Identical to main ff64902a, including field offsets:** C6 `Machine::run`,
  `run_unmodeled`, `step_core`, `Bus::tick`, `next_deadline`, `periph_read`,
  `pending_work`, and the Bus fetch/read/write entry points. Device tick
  dispatch is inlined in `Bus::tick`, not a separate symbol.
* **Remaining C6 differences:** `periph_write` (1660 → 1747 instructions,
  versus 1746 before the fix) still contains the SHA `dma_pending` byte test,
  ECC dispatch and PCR reset effect. `Peripherals::write32` is inlined there.
  These are total static function sizes, not instructions executed on each
  write. The SHA transfer/address filter and PCR reset handler remain cold,
  out-of-line functions; the common write-effect mask includes PERIPH_RESET.
  Register allocation is not identical throughout this MMIO function.
* `refresh_irq` remains 442 → 449 instructions: seven additional instructions
  merge ECC's cached source word. `refresh_optional` is 225 → 259 and
  `tick_optional` 194 → 227 because the ECC arm exists, although ECC has no
  clock and is never placed in the active optional-device list.
* `Peripherals::read32` is 679 → 677 with the ECC arm and out-of-line
  `ModemBb::read`; `Peripherals::new` is 1560 → 1592 for cold allocation and
  initialization. The C6 guest-reset function and peripheral drop glue also
  differ in cold allocation/destruction paths. New ECC arithmetic and SHA DMA functions are feature paths.
* Other PR-wide differences remain C3 `periph_write` (SHA pending test), S3
  `tick_impl` (shared crypto OUT gather bodies) and `Rsa::write` (LTO outlining).
  This fix changes no C3/S3 or shared-walker source.

The layout-induced differences are removed. The complete MMIO/interrupt path
is **not** instruction-identical to main ff64902a, and disassembly alone does not prove
that the 7/7 C6 CPU regression is cured. The maintainer's separate
`ff64902a` → `a175e357` measurement above resolves that flag.

## Limits and evidence curation

This is a functional model, not accelerator latency, RF calibration, hardware
parity, browser-speed or TLS-over-TCP evidence. P-192 is unsupported: selecting
its key length does not complete an operation. ECC Jacobian/division modes,
certificate verification and TLS 1.3 are outside this fixture. The deterministic
RNG and PSK are public test inputs and provide no production security.

Retained evidence contains source-relative commands, public versions, hashes,
known-answer vectors and test verdicts. Build logs, absolute build paths and
ELF debug information are not retained. The binary strings were checked for
host paths before inclusion. Redaction removes no measured values; the CPU
historical tables above hold their timing samples; the maintainer's merged-head
comparison is retained as reported summary values only. No private captures or private firmware are
needed to reproduce these tests.
