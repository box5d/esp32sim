#!/usr/bin/env python3
"""Hot-path comparison (hotcmp.py plus GDMA engines and refresh_irq). Usage: hot.py A.dis B.dis [diff]"""
import re, sys, difflib, types
from pathlib import Path
src = open(Path(__file__).with_name('asmcmp.py')).read().split("a, b = funcs(sys.argv[1])")[0]
m = types.ModuleType("asmcmp"); exec(src, m.__dict__)
ALIAS = re.compile(r" as esp_periph::device::Device"); PAGE0 = re.compile(r", #PAGEOFF\]")
def norm(fs):
    out = {}
    for n, body in fs.items():
        out.setdefault(ALIAS.sub("", n), [PAGE0.sub("]", ALIAS.sub("", i)) for i in body])
    return out
a = norm(m.funcs(sys.argv[1])); b = norm(m.funcs(sys.argv[2]))
HOT = re.compile(
    r"Machine<esp32(s3|c3|c6)::soc::(S3|C3|C6)>>::(run|run_modeled|run_unmodeled|step_core|after_round|present_irqs|settle_modeled_time|apply_due_script_events|deliver_events|drain_console)\b"
    r"|esp32(s3|c3|c6)::bus::SocBus( as [a-z_:]+)?>::(tick|tick_impl|tick_with_pins|refresh_tick_budget|next_deadline|refresh_irq|periph_read|periph_write|periph_write_inner|pending_work|devices|pin_deadline|fetch|read8|read16|read32|write8|write16|write32|read32_access|write32_access|read8_access|write8_access)\b"
    r"|esp32(s3|c3|c6)::.*(dma|gdma|Gdma|i2s_rx_step|i2s_receive|camera)"
    r"|esp_periph::gdma::"
    r"|esp32(s3|c3|c6)::periph::Peripherals>::(refresh_irq|refresh_lines|refresh_work|tick)\b"
    r"|esp_periph::(timg::TimerGroup|uart::Uart|gpio::Gpio|rtc_cntl::RtcCntl|sha::Sha|ledc::Ledc|i2c::I2c|rmt::Rmt|gpspi::GpSpi|i2s::I2s)>::(tick|read|write|next_deadline|hash_block|irq_sources|poll_active)"
    r"|xtensa_lx7::.*::(step|run|exec)|riscv_rv32::.*::(step|run|exec)"
    r"|esp_soc::pins::PinRoutes")
hot = sorted(n for n in set(a) | set(b) if HOT.search(n) and "esp32::" not in n and "{{closure}}" not in n)
same = diff = miss = 0
for n in hot:
    if n not in a or n not in b:
        miss += 1; print(f"MISSING-{'A' if n not in a else 'B'} {n[:220]}"); continue
    if m.same(a, b, n):
        same += 1
    else:
        diff += 1; print(f"DIFF {len(a[n])} {len(b[n])} {n[:220]}")
        if len(sys.argv) > 3:
            d = [l for l in difflib.unified_diff(a[n], b[n], lineterm="", n=0) if l[:1] in "+-" and not l.startswith(("+++", "---"))]
            print("\n".join("    " + x for x in d[:int(sys.argv[3])]))
print(f"hot functions: same={same} diff={diff} missing={miss}")

sys.exit(0 if diff == 0 and miss == 0 else 1)
