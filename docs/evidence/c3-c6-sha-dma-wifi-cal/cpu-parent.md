# PR #203 — CPU pass 10

Rust 1.99.0; cargo +1.99.0 build --release --bins; separate target directories. Baseline `schematik-engineering/esp32sim@baf9845b` built once; each candidate fetched from origin immediately before its build. Sequential child user CPU via getrusage, including startup. One warmup B→M, then seven measured pairs M→B, B→M alternating. Before every attempt wait for 1-minute load <5 (15-second polling); monitor every second and discard/retry the entire pair if peak >7. Exact total/per-core instructions and console SHA-256 across all attempts. S3 hello: 3000 emulated seconds, board none; C3/C6: 30 seconds, board none; Pocket Tank: 30 seconds, waveshare-amoled18-v2. Ranges are min–max; change is ratio of medians. Flags: slower ≥6/7 or non-overlapping ranges. No per-second load series retained.

Measured branch `c3-c6-sha-dma-wifi-cal` at `schematik-engineering/esp32sim@3d9eee3ca392608f89f57818cafc68cce3ecbc09`. User CPU seconds.

| Workload | Parent median (range) | PR median (range) | Change | PR slower in N/7 | Instructions | max load |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C6 hello | 3.0448 (3.0246–3.0627) | 3.0866 (3.0625–3.1108) | +1.37% | 7/7 | 4800000000 | 4.89 |
| C3 hello | 2.3766 (2.3539–2.4463) | 2.3554 (2.3340–2.3907) | -0.89% | 1/7 | 4800000000 | 5.04 |
| S3 hello | 24.2665 (24.0082–24.4175) | 24.2894 (24.1204–25.0501) | +0.09% | 5/7 | 1789819657 | 5.70 |
| Pocket Tank | 35.5924 (34.7411–36.1382) | 35.4650 (35.0171–36.0679) | -0.36% | 4/7 | 10073833665 | 6.64 |

## C6 hello

Correctness: PASS. CPU: FLAGGED, slower in 7/7 pairs.

| Pair | Attempt | Order | Parent s | PR s | Parent load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 3.080924 | 3.031479 | 4.78/4.72/4.78 | 4.78/4.78/4.78 | accepted |
| 1 | 2 | M→B | 3.046924 | 3.086566 | 4.72/4.58/4.72 | 4.58/4.58/4.58 | accepted |
| 2 | 3 | B→M | 3.024588 | 3.075549 | 4.69/4.69/4.69 | 4.58/4.69/4.69 | accepted |
| 3 | 4 | M→B | 3.044797 | 3.065217 | 4.69/4.88/4.88 | 4.88/4.89/4.89 | accepted |
| 4 | 5 | B→M | 3.047114 | 3.062466 | 4.89/4.58/4.89 | 4.89/4.89/4.89 | accepted |
| 5 | 6 | M→B | 3.032835 | 3.103134 | 4.58/4.58/4.58 | 4.58/4.45/4.58 | accepted |
| 6 | 7 | B→M | 3.043065 | 3.110791 | 4.41/4.41/4.41 | 4.45/4.41/4.45 | accepted |
| 7 | 8 | M→B | 3.062671 | 3.087199 | 4.41/4.46/4.46 | 4.46/4.66/4.66 | accepted |

## C3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Parent s | PR s | Parent load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 2.384228 | 2.334058 | 4.66/4.69/4.69 | 4.66/4.66/4.66 | accepted |
| 1 | 2 | M→B | 2.365575 | 2.334023 | 4.69/4.69/4.69 | 4.69/5.04/5.04 | accepted |
| 2 | 3 | B→M | 2.353859 | 2.353076 | 4.43/4.40/4.43 | 4.43/4.43/4.43 | accepted |
| 3 | 4 | M→B | 2.387894 | 2.367267 | 4.40/4.40/4.40 | 4.40/4.40/4.40 | accepted |
| 4 | 5 | B→M | 2.376593 | 2.355355 | 4.44/4.44/4.44 | 4.40/4.44/4.44 | accepted |
| 5 | 6 | M→B | 2.384978 | 2.336822 | 4.44/4.41/4.44 | 4.41/4.41/4.41 | accepted |
| 6 | 7 | B→M | 2.366307 | 2.390690 | 4.29/4.29/4.29 | 4.41/4.29/4.41 | accepted |
| 7 | 8 | M→B | 2.446294 | 2.375078 | 4.29/4.35/4.35 | 4.35/4.35/4.35 | accepted |

## S3 hello

Status: PASS. Flags: —.

| Pair | Attempt | Order | Parent s | PR s | Parent load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 24.529207 | 24.308364 | 4.52/4.22/4.52 | 4.35/4.52/4.80 | accepted |
| 1 | 2 | M→B | 24.417527 | 24.737056 | 4.22/4.40/4.56 | 4.40/4.97/5.06 | accepted |
| 2 | 3 | B→M | 24.279431 | 25.050123 | 5.23/4.82/5.23 | 4.97/5.23/5.70 | accepted |
| 3 | 4 | M→B | 24.250895 | 24.289427 | 4.82/4.88/5.04 | 4.88/5.09/5.09 | accepted |
| 4 | 5 | B→M | 24.008233 | 24.275739 | 4.60/4.33/4.60 | 4.83/4.60/4.85 | accepted |
| 5 | 6 | M→B | 24.157828 | 24.120401 | 4.33/5.02/5.02 | 5.02/4.95/5.02 | accepted |
| 6 | 7 | B→M | 24.266466 | 24.352186 | 4.83/5.08/5.08 | 4.95/4.83/4.96 | accepted |
| 7 | 8 | M→B | 24.364894 | 24.198059 | 4.93/4.73/4.93 | 4.73/4.79/5.01 | accepted |

## Pocket Tank

Status: PASS. Flags: —.

| Pair | Attempt | Order | Parent s | PR s | Parent load before/after/peak | PR load before/after/peak | Result |
| --- | ---: | --- | ---: | ---: | --- | --- | --- |
| warmup | 1 | B→M | 35.141306 | 35.516029 | 4.76/4.39/4.76 | 4.79/4.76/4.99 | accepted |
| 1 | 2 | M→B | 35.592353 | 35.464985 | 4.39/4.67/4.67 | 4.67/4.59/4.67 | accepted |
| 2 | 3 | B→M | 35.990309 | 36.067852 | 4.93/4.54/4.93 | 4.59/4.93/5.20 | accepted |
| 3 | 4 | M→B | 36.138246 | 35.759377 | 4.54/5.14/5.15 | 5.14/4.90/5.14 | accepted |
| 4 | 5 | B→M | 35.506349 | 35.511532 | 4.79/6.64/6.64 | 4.90/4.79/5.10 | accepted |
| 5 | 6 | M→B | 34.741078 | 35.017769 | 4.67/4.40/4.67 | 4.40/3.98/4.45 | accepted |
| 6 | 7 | B→M | 35.627243 | 35.028041 | 4.50/5.12/5.12 | 3.98/4.50/4.50 | accepted |
| 7 | 8 | M→B | 34.807894 | 35.017070 | 4.49/4.64/4.80 | 4.64/4.25/4.75 | accepted |

Max load in summary includes accepted warmup and measured pairs; discarded attempts appear above.
