#!/usr/bin/env python3
"""Verify the portable cold-field guard; run from the repository root."""
import json
import pathlib
import subprocess

path = pathlib.Path("esp32c6/src/periph.rs")
source = path.read_text()
edits = [
    ("pub optional: Box<OptionalPeripherals>,", "pub optional: OptionalPeripherals,"),
    ("optional: Box::new(OptionalPeripherals { i2s0: esp_periph::I2s::new(CPU_HZ), ecc: Default::default(), parlio: crate::parlio::Parlio::new() }),",
     "optional: OptionalPeripherals { i2s0: esp_periph::I2s::new(CPU_HZ), ecc: Default::default(), parlio: crate::parlio::Parlio::new() },"),
]
command = ["nice", "-n", "10", "cargo", "+1.99.0", "test", "--release", "-p", "esp32c6", "--test", "layout"]

def run():
    return subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)

baseline = run()
assert baseline.returncode == 0, baseline.stdout
mutated = source
for old, new in edits:
    assert mutated.count(old) == 1, old
    mutated = mutated.replace(old, new)
try:
    path.write_text(mutated)
    result = run()
finally:
    path.write_text(source)
restored = run()
test = "cold_devices_do_not_grow_the_native_hot_bus"
killed = result.returncode != 0 and f"test {test} ... FAILED" in result.stdout
assert killed, result.stdout
assert "could not compile" not in result.stdout, result.stdout
assert restored.returncode == 0, restored.stdout
receipt = {
    "mutation": "Store OptionalPeripherals inline instead of in its single Box",
    "edits": [{"before": old, "after": new} for old, new in edits],
    "command": " ".join(command),
    "test": test,
    "baseline_exit": baseline.returncode,
    "mutation_exit": result.returncode,
    "restored_exit": restored.returncode,
    "killed": killed,
    "scope": "Architecture-independent actual-field size guard; no fixed Peripherals/SocBus sizes or offsets",
}
pathlib.Path("docs/evidence/led-rmt-parlio-fixes/layout-mutation.json").write_text(json.dumps(receipt, indent=2) + "\n")
print(json.dumps(receipt, indent=2))
