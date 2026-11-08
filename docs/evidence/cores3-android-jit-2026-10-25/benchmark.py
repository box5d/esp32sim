"""Equal-work, interleaved native CoreS3 comparison; no live emulator is contacted."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import resource
import statistics
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--home', type=Path, required=True)
p.add_argument('--before', type=Path, required=True)
p.add_argument('--after', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--rounds', type=int, default=5)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
sha = lambda data: hashlib.sha256(data).hexdigest()
results = {}
for firmware in ['kuwagata', 'official-user-demo']:
    fw = a.home / 'agy-web-console/firmware/cores3' / firmware / 'firmware.bin'
    samples = {'before': [], 'after': []}
    expected = None
    for iteration in range(a.rounds + 1):
        for label, binary in ([('before', a.before), ('after', a.after)] if iteration % 2 == 0 else [('after', a.after), ('before', a.before)]):
            prefix = a.output / f'{firmware}-{iteration}-{label}'
            png, wav = prefix.with_suffix('.png'), prefix.with_suffix('.wav')
            cmd = [str(binary), '--chip', 's3', '--board', 'm5cores3', '--flash-mb', '16',
                   '--psram-mb', '8', '--boot', 'rom', '--rom',
                   str(a.home / '.espressif/tools/esp-rom-elfs/20260528/esp32s3_rev0_rom.elf'),
                   '--flash-image', str(fw), '--max-insns', '150000000', '--no-dump',
                   '--tft-png', str(png), '--wav', str(wav)]
            cpu = resource.getrusage(resource.RUSAGE_CHILDREN).ru_utime
            started = time.perf_counter()
            run = subprocess.run(cmd, capture_output=True, timeout=120, check=True)
            wall = time.perf_counter() - started
            cpu = resource.getrusage(resource.RUSAGE_CHILDREN).ru_utime - cpu
            stderr = run.stderr.decode()
            prefix.with_suffix('.stderr').write_text(stderr)
            prefix.with_suffix('.stdout').write_bytes(run.stdout)
            stop = next(line for line in stderr.splitlines() if line.startswith('[emu] stop:'))
            stop = re.sub(r'insns in .*?; emulated', 'insns; emulated', stop)
            output = {'stop': stop, 'serialSha256': sha(run.stdout), 'pngSha256': sha(png.read_bytes()),
                      'wavSha256': sha(wav.read_bytes()),
                      'devices': [line for line in stderr.splitlines() if line.startswith(('[m5cores3]', '[emu] i2s', '[emu] crypto:', '  core'))]}
            if expected is None:
                expected = output
            assert output == expected, f'Output mismatch: {firmware}/{iteration}/{label}'
            compiled = int(re.search(r'jit: (\d+) compiled', stderr)[1])
            assert (compiled > 0) == (label == 'after'), f'Unexpected JIT mode: {label}'
            if iteration:
                samples[label].append({'wallSeconds': wall, 'userCpuSeconds': cpu, 'jitBlocks': compiled})
            print(f'{firmware} round {iteration} {label}: wall={wall:.3f}s cpu={cpu:.3f}s', flush=True)
    medians = {label: {metric: statistics.median(row[metric] for row in values)
                       for metric in ['wallSeconds', 'userCpuSeconds']} for label, values in samples.items()}
    results[firmware] = {'firmwareSha256': sha(fw.read_bytes()), 'samples': samples,
                         'medians': medians, 'speedup': medians['before']['wallSeconds'] / medians['after']['wallSeconds'],
                         'identicalOutput': expected}
    print(f'{firmware}: {results[firmware]["speedup"]:.3f}x speedup', flush=True)
report = {'warmupPairs': 1, 'rounds': a.rounds, 'schedulerSteps': 150000000,
          'binarySha256': {label: sha(binary.read_bytes()) for label, binary in [('before', a.before), ('after', a.after)]},
          'results': results}
(a.output / 'results.json').write_text(json.dumps(report, indent=2))
