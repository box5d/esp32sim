# Android ARM64 native JIT (EX002)

Base source: `598beb6`; candidate is the local Android backend patch identified by the per-file hashes in `validation.json`. This extends EX002 to a previously disabled host platform. Scheduling, virtual clocks, instructions and peripheral models are unchanged.

The ARM64 emitter, executable-memory flags, instruction-cache invalidation and unavailable-backend guards now include Android. Rust links with `-nodefaultlibs`, so the Android build script asks the configured linker for its compiler-rt archive and links it explicitly. No shared compiler, package or environment settings change.

The existing emitter does not use Android's reserved x18 register ([Android ABI](https://developer.android.com/ndk/guides/abis#arm64-v8a)). Cache invalidation uses the installed compiler runtime rather than a new assembly implementation.

## Measurements

Native Termux, Rust 1.95.0, release profile. No concurrent build or regression test during measurement. One warmup pair, then five interleaved pairs with alternating order. Each arm runs 150,000,000 scheduler steps from ROM boot, with the same BIN and no external inputs. This measures native execution throughput, not hardware-calibrated FPS or sensor latency.

| BIN | Before median wall | After median wall | Speedup |
| --- | ---: | ---: | ---: |
| kuwagata | 1.522 s | 1.014 s | 1.501x |
| official-user-demo | 3.136 s | 2.073 s | 1.513x |

Every arm checks equal per-core instruction counts, cycles, exceptions, interrupts, serial SHA-256, framebuffer PNG SHA-256, WAV SHA-256 and board/device report. JIT compilation must be zero before and nonzero after. Both measured BINs pass these checks for all samples. Raw samples, medians, firmware and executable hashes are in `results.json`.

```sh
python3 docs/evidence/cores3-android-jit-2026-10-25/benchmark.py \
  --home "$HOME" --before /path/to/baseline/esp32sim \
  --after target/release/esp32sim --output /path/to/results --rounds 5
```

## Validation and limits

- All 1,003 native workspace tests pass, including ignored tests except `external_*`; existing golden outputs remain unchanged.
- Native workspace Clippy with all targets and warnings denied passes.
- Four native compiled-memory boundary tests and seven step/block/JIT semantic tests pass.
- Real CoreS3 Studio browser camera, fragmented-camera and IMU paths pass against an isolated native process running the official BIN.
- Initial gate-only attempt failed with undefined `__clear_cache`. Linking the compiler runtime corrected this; this failed attempt is not a speed result.
- The installed Rust toolchain was used. Shared runtime settings were preserved. No physical-board speed comparison, wasm32 cross-build, or guarantee of full real-time speed. Background host load was not controlled; all five samples and their variation are retained.

Adopted locally for the M5Stack launcher. No upstream PR or commit is implied.
