# M5Stack CoreS3 on v0.3.0 — upstream sync record

How the fork-era CoreS3 support was carried onto the v0.3.0 upstream base, what the
merge replaced, what had to be rebuilt, and how it is verified. The integration lives on
`main` after the `feature/upstream-sync-v030` merge (base `7f23a28`, upstream target
`origin/main` = `63670bb`, v0.3.0).

## What the merge brought in

v0.3.0 is 1,159 commits ahead of the fork base and reshaped the pieces the CoreS3 support
sat on:

- The **S3 bus is split** (`bus/dma.rs`, `bus/pins.rs`) and the camera pipeline moved into
  the general DMA engine: `dma_cam_step` generates the VSYNCs, and a frame is handed out
  through `board.camera_frame()`. The fork's chunked `cam_pos`/`cam_chunk_acc` scheme is
  replaced by the upstream GDMA IN chain (channel 5, `REC_DATA_BYTELEN` chunk EOF,
  `CAM_VSYNC_INT`), so the CoreS3 GC0308 needed no extra core code for this — see
  `871c13d`, where the chunked RGB565 lines are verified against `board.camera_frame()`.
- The **`BoardModel` trait changed**: `set_camera_picture` now returns `Result<(), &str>`,
  `set_imu_state` is replaced by `set_imu_motion(mode)`/`imu_motion(u32)`, and the timed
  GPIO/SPI surface is new (`gpio_output_at`, `spi_transfer_pins`, `uses_gpio_edges`,
  `SpiPins`). The CoreS3 board implements the current trait; the BMI270 sits on the same
  motion-queue API as upstream's QMI8658 instead of the old scripted state.
- **I2C moved to `esp-periph`** with a refreshed `I2cDevice` trait (`address`,
  `start_address`, `pins`, multi-curve `clock_ticks`). The CoreS3 devices (FT6336U,
  AXP2101, AW9523B, BMI270, GC0308, BM8563, ES7210, AW88298, LTR553) have no upstream
  counterparts, so they were ported wholesale onto the new trait in `c5e2357`.
- **SENS ADC1 one-shot** (`RtcCntl::sens_meas`, 0x6000_880c) arrives with upstream and is
  verified for the CoreS3 Port B channel wiring in `871c13d` (GPIO8 → ch7, START/DONE,
  held result while START stays high).

## What was rebuilt after the merge

The merge replaced fork-era front-end files, and two behaviours the browser tests rely on
came back through `8e21e63`:

- **Fragmented WebSocket messages are reassembled** (`esp-soc/src/web.rs`). Phone
  browsers (the Android WebView frontends) split a 76805-byte type-3 camera picture into
  ~32 KiB fragments; without reassembly every such picture was dropped. With several
  tabs connected, the newest complete picture wins.
- **A new camera picture re-pushes the RGB888 preview** (`self.ws.cam_sent = false` in
  `poll_camera_input`), so a stalled or stale preview refreshes when the host feeds a new
  image.
- **`ESP_EMU_WEB_BIND`** overrides the listener address (reach the page from a phone on
  the LAN) and **`ESP_EMU_WEB_ORIGIN`** adds exact-match extra origins to the WebSocket
  origin check — a client page on another port (the CoreS3 Studio console page drives the
  emulator over `ws://`) uses this. Documented in [web-ui.md](web-ui.md), and
  `run_m5cores3.sh` sets both.

`web/run.html` regained the `m5cores3` board branch (ILI9342C 320×240 panel, GC0308
camera panel) and the phone camera fixes: `capture="environment"` picture fallback on
insecure origins, rear-camera `facingMode`, and the preview kept visible while streaming.

## Verified state

- `ESP32SIM_ROM_DIR=… cargo test --release --workspace -- --include-ignored --skip
  external_`: 105 test binaries, 998 passed / 0 failed.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean
  (`6d891b5` rewrites five pre-existing upstream warnings to satisfy newer clippy; the
  wasm-target clippy step is covered by CI, see below).
- Browser end-to-end against the real `cores3_studio.html`: `tools/cores3-motion-browser.cjs`
  (IMU tilt/return reaches the BMI270 and the guest LCD) and
  `tools/cores3-camera-browser.cjs --fragment-camera` (a fragmented type-3 picture round-trips,
  red/green frames verified) both pass.
- The wasm-dependent CI steps (wasm clippy, `tools/wasm-build.sh` and the wasm tests) are
  not runnable on the machines used for this sync and are covered by CI.

The board itself is documented in [boards.md](boards.md).

## Keeping the fork working

The board name accepts `m5cores3`, `cores3`, and `m5stack-cores3` (upstream's registry
spells it `m5cores3`). `cores3_studio.html` (CoreS3 Studio, the task-5 verification client)
continues to speak the same protocol as `run.html`: JSON `stat`/`ring`/`serial`/`emu`,
type-1 LCD frames, type-4 camera previews, and outbound `touch`/`imu` JSON plus binary
type-3 camera pictures — see the tables in [web-ui.md](web-ui.md).
