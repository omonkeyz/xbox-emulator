"""Run `python tools/verify/run_d3d12_guest.py` in an x64 Visual Studio developer
shell with MSVC Rust and the Windows SDK. Each launch saves mdbg and compat logs
alongside the guest in the printed staging directory.
"""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]


def main():
    target = ROOT / "target"
    subprocess.run(
        ["cargo", "build", "--locked", "-p", "d3d12_xs", "--target-dir", target],
        cwd=ROOT, check=True,
    )
    stage = Path(tempfile.mkdtemp(prefix="d3d12-guest-", dir=target))
    print(f"Guest and launch logs in {stage}", flush=True)
    guest = stage / "d3d12_guest.exe"
    tracer = stage / "mdbg.exe"
    subprocess.run(
        ["rustc", "--edition=2021", "-O", ROOT / "tools/dev/mdbg.rs", "-o", tracer],
        cwd=stage, check=True,
    )
    subprocess.run(
        [
            "cl.exe", "/nologo", "/std:c++17", "/W4", "/O1", "/EHsc", "/MD",
            ROOT / "tools/verify/d3d12_guest.cpp", f"/Fe{guest}",
            "/link", target / "debug/d3d12_xs.dll.lib",
        ],
        cwd=stage, check=True,
    )
    shutil.copy2(target / "debug/d3d12_xs.dll", stage)

    for color, expected in [("red", "ff 00 00 ff"), ("green", "00 ff 00 ff")]:
        trace = stage / f"{color}.trace.log"
        with trace.open("wb") as output:
            run = subprocess.run(
                [tracer, guest, color], cwd=stage, stdout=output, stderr=subprocess.STDOUT, timeout=90,
                env={**os.environ, "TOMBOX_LOG": str(stage / f"{color}.compat.log")},
            )
        lines = trace.read_text(encoding="utf-8", errors="replace").splitlines()
        guest_lines = [line for line in lines if line.startswith("[ODS] d3d12 guest ")]
        exits = [line for line in lines if line.startswith("[trace] EXIT_PROCESS ")]
        print(*guest_lines, *exits, sep="\n")
        # mdbg itself can return zero when its child failed or timed out.
        pixel = f"[ODS] d3d12 guest {color} pixel(2,1)={expected}"
        if run.returncode or pixel not in guest_lines or exits != ["[trace] EXIT_PROCESS code=0 (0x0)"]:
            raise SystemExit(f"{color} expected {expected} at (2,1) and child exit 0. See {trace}")


if __name__ == "__main__":
    main()
