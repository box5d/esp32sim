#!/usr/bin/env python3
"""Dump every named Peripherals/SocBus field; restore sources even on failure.

Usage: CARGO_TARGET_DIR=<build-dir> python3 layout_probe.py <checkout> > layout.json
Run only in a clean, idle checkout: temporary unit tests are appended to source.
"""
import json
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(sys.argv[1]).resolve()
originals = {}
try:
    for chip in ("esp32c3", "esp32c6", "esp32s3"):
        for file, struct in (("periph.rs", "Peripherals"), ("bus.rs", "SocBus")):
            path = root / chip / "src" / file
            text = path.read_text()
            originals[path] = text
            match = re.search(r"pub struct " + struct + r"\s*\{(.*?)^\}", text, re.S | re.M)
            assert match is not None, (chip, struct)
            body = match.group(1)
            fields = re.findall(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:", body, re.M)
            code = "\n#[cfg(test)] mod restack_layout_probe { use super::*; #[test] fn restack_layout_dump() {\n"
            code += f'println!("LAYOUT {chip} {struct} size {{}}", std::mem::size_of::<{struct}>());\n'
            for field in fields:
                code += f'println!("LAYOUT {chip} {struct} {field} {{}}", std::mem::offset_of!({struct}, {field}));\n'
            path.write_text(text + code + "}}\n")
    result = subprocess.run(
        ["nice", "-n", "10", "cargo", "+1.99.0", "test", "--release",
         "-p", "esp32c3", "-p", "esp32c6", "-p", "esp32s3", "--lib",
         "restack_layout_dump", "--", "--nocapture"],
        cwd=root, capture_output=True, text=True, check=True,
    )
    print(json.dumps(sorted(line for line in result.stdout.splitlines() if line.startswith("LAYOUT")), indent=2))
finally:
    for path, text in originals.items():
        path.write_text(text)
