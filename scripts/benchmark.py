#!/usr/bin/env python3
"""Time TypeScript Probatio (c5cd175) against this Rust build.

Five runs per command. The report is median milliseconds and the min-max.
Peak RSS is the highest VmHWM observed while the process and its children ran.
Nothing here is estimated. A side that cannot run is recorded as such.

Usage:
  python3 scripts/benchmark.py

Environment:
  RUST_BIN   release binary (default: ./target/release/probatio)
  TS_NODE    node binary (default: node)
  TS_CLI     compiled TypeScript cli.js (default: /tmp/probatio-ts/dist/cli.js)
  BENCH_OSS  a real project to generate and run (default: /tmp/itsdangerous)
"""

from __future__ import annotations

import os
import platform
import shutil
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUST = Path(os.environ.get("RUST_BIN", ROOT / "target" / "release" / "probatio"))
NODE = os.environ.get("TS_NODE", "node")
TS = Path(os.environ.get("TS_CLI", "/tmp/probatio-ts/dist/cli.js"))
OSS = Path(os.environ.get("BENCH_OSS", "/tmp/itsdangerous"))
OUT = ROOT / "scripts" / "benchmark_results.md"
RUNS = 5


def machine() -> str:
    cpu = "unknown"
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        for line in cpuinfo.read_text(errors="replace").splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    mem = "unknown"
    meminfo = Path("/proc/meminfo")
    if meminfo.is_file():
        for line in meminfo.read_text().splitlines():
            if line.startswith("MemTotal"):
                mem = line.split(":", 1)[1].strip()
                break
    return (
        f"{platform.system()} {platform.release()} {platform.machine()}, "
        f"{os.cpu_count()} cores, {cpu}, {mem}. "
        f"Rust binary {RUST}. TypeScript cli {TS} under {NODE}."
    )


def peak_rss(pid: int) -> int:
    """Highest VmHWM, in kilobytes, of this pid and its children."""
    best = 0
    seen = set()
    stack = [pid]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        try:
            text = Path(f"/proc/{current}/status").read_text(errors="replace")
        except Exception:
            continue
        for line in text.splitlines():
            if line.startswith("VmHWM:"):
                parts = line.split()
                if len(parts) >= 2 and parts[1].isdigit():
                    best = max(best, int(parts[1]))
        try:
            kids = Path(f"/proc/{current}/task/{current}/children").read_text().strip()
        except Exception:
            kids = ""
        stack.extend(int(item) for item in kids.split() if item.isdigit())
    return best


def time_command(argv: list[str], cwd: Path, env: dict[str, str] | None = None) -> tuple[int, float, int, str]:
    merged = os.environ.copy()
    if env:
        merged.update(env)
    start = time.perf_counter()
    proc = subprocess.Popen(argv, cwd=cwd, env=merged, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    rss = 0
    while proc.poll() is None:
        try:
            rss = max(rss, peak_rss(proc.pid))
        except Exception:
            pass
        time.sleep(0.01)
    stdout, stderr = proc.communicate()
    try:
        rss = max(rss, peak_rss(proc.pid))
    except Exception:
        pass
    elapsed = (time.perf_counter() - start) * 1000
    return proc.returncode or 0, elapsed, rss, stdout + stderr


def median_row(samples: list[float]) -> str:
    ordered = sorted(samples)
    mid = statistics.median(ordered)
    return f"{mid:.0f} ({min(ordered):.0f}-{max(ordered):.0f})"


def commit_sample(path: Path) -> None:
    if not (path / ".git").is_dir():
        subprocess.run(["git", "init"], cwd=path, check=True, capture_output=True)
        subprocess.run(["git", "config", "user.email", "bench@example.com"], cwd=path, check=True)
        subprocess.run(["git", "config", "user.name", "bench"], cwd=path, check=True)
    subprocess.run(["git", "add", "."], cwd=path, check=True)
    subprocess.run(["git", "commit", "-m", "sample", "--allow-empty"], cwd=path, check=True, capture_output=True)


def prepare_js(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    (path / "src").mkdir(exist_ok=True)
    (path / "test").mkdir(exist_ok=True)
    (path / "package.json").write_text('{"scripts":{"test":"node --test"}}\n')
    (path / "src" / "calc.js").write_text(
        "export function clamp(n, lo, hi) {\n  if (n < lo) return lo;\n  if (n > hi) return hi;\n  return n;\n}\n"
        "export function adult(age) { return age >= 18 && age < 120; }\n"
        "export function both(a, b) { return a && b; }\n"
    )
    (path / "test" / "calc.test.js").write_text(
        "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { clamp, adult, both } from '../src/calc.js';\n"
        "test('clamp lo', () => { assert.equal(clamp(1, 2, 5), 2); });\n"
        "test('clamp hi', () => { assert.equal(clamp(9, 2, 5), 5); });\n"
        "test('adult boundary', () => { assert.equal(adult(18), true); });\n"
        "test('both', () => { assert.equal(both(true, false), false); });\n"
    )
    commit_sample(path)


def prepare_py(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    (path / "src").mkdir(exist_ok=True)
    (path / "tests").mkdir(exist_ok=True)
    (path / "src" / "calc.py").write_text(
        "def clamp(n, lo, hi):\n    if n < lo:\n        return lo\n    if n > hi:\n        return hi\n    return n\n\n"
        "def adult(age):\n    return age >= 18 and age < 120\n\n"
        "def both(a, b):\n    return a and b\n"
    )
    (path / "tests" / "test_calc.py").write_text(
        "import sys\nfrom pathlib import Path\nsys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))\nimport calc\n\n"
        "def test_clamp_lo():\n    assert calc.clamp(1, 2, 5) == 2\n\n"
        "def test_clamp_hi():\n    assert calc.clamp(9, 2, 5) == 5\n\n"
        "def test_adult():\n    assert calc.adult(18) is True\n\n"
        "def test_both():\n    assert calc.both(True, False) is False\n"
    )
    (path / "pytest.ini").write_text("[pytest]\n")
    commit_sample(path)


def repeat(argv: list[str], cwd: Path, env: dict[str, str] | None = None) -> tuple[str, str, str]:
    times = []
    rss = []
    last = ""
    code = 0
    for index in range(RUNS):
        fresh = [item.replace("{run}", str(index)) for item in argv]
        code, elapsed, peak, text = time_command(fresh, cwd, env)
        times.append(elapsed)
        rss.append(peak)
        last = text
    note = ""
    if code != 0:
        note = f" last exit {code}"
    return median_row(times) + note, f"{max(rss)}", last


def main() -> None:
    js = Path("/tmp/probatio-bench-js")
    py = Path("/tmp/probatio-bench-py")
    prepare_js(js)
    prepare_py(py)
    rows = []
    notes = []

    def add(name: str, ts_argv: list[str] | None, rs_argv: list[str] | None, cwd: Path, env=None):
        ts_cell = "not run"
        rs_cell = "not run"
        ts_rss = ""
        rs_rss = ""
        ts_out = ""
        rs_out = ""
        if ts_argv and TS.is_file() and shutil.which(NODE):
            ts_cell, ts_rss, ts_out = repeat([NODE, str(TS), *ts_argv], cwd, env)
        elif ts_argv:
            ts_cell = "TypeScript cli not built"
        if rs_argv and RUST.is_file():
            rs_cell, rs_rss, rs_out = repeat([str(RUST), *rs_argv], cwd, env)
        elif rs_argv:
            rs_cell = "Rust binary not built"
        rows.append((name, ts_cell, rs_cell, ts_rss, rs_rss))
        notes.append((name, ts_out, rs_out))

    add("startup schema", ["schema"], ["schema"], ROOT)
    gen_js_ts = ["mutate", "generate", "--package", str(js), "--out", "/tmp/pb-js-ts-{run}", "--src", "src"]
    gen_js_rs = ["mutate", "generate", "--package", str(js), "--out", "/tmp/pb-js-rs-{run}", "--src", "src"]
    add("generate JS sample", gen_js_ts, gen_js_rs, js)
    gen_py_ts = ["mutate", "generate", "--package", str(py), "--out", "/tmp/pb-py-ts-{run}", "--src", "src"]
    gen_py_rs = ["mutate", "generate", "--package", str(py), "--out", "/tmp/pb-py-rs-{run}", "--src", "src"]
    add("generate Python sample", gen_py_ts, gen_py_rs, py)

    # One generate so the run has patches. The timed generate above already wrote them.
    run_js_ts = ["mutate", "run", "--package", str(js), "--patches", "/tmp/pb-js-ts-{run}/mutants", "--out", "/tmp/pb-js-ts-run-{run}", "--no-confirm"]
    run_js_rs = ["mutate", "run", "--package", str(js), "--patches", "/tmp/pb-js-rs-{run}/mutants", "--out", "/tmp/pb-js-rs-run-{run}", "--no-confirm", "--timeout-ms", "60000"]
    add("run JS sample, no confirm", run_js_ts, run_js_rs, js)
    run_py_ts = ["mutate", "run", "--package", str(py), "--patches", "/tmp/pb-py-ts-{run}/mutants", "--out", "/tmp/pb-py-ts-run-{run}", "--no-confirm", "--tests-dir", "tests"]
    run_py_rs = ["mutate", "run", "--package", str(py), "--patches", "/tmp/pb-py-rs-{run}/mutants", "--out", "/tmp/pb-py-rs-run-{run}", "--no-confirm", "--timeout-ms", "60000"]
    add("run Python sample, no confirm", run_py_ts, run_py_rs, py)
    add(
        "run Python sample, workers 4, no confirm",
        ["mutate", "run", "--package", str(py), "--patches", "/tmp/pb-py-ts-{run}/mutants", "--out", "/tmp/pb-py-ts-w-{run}", "--no-confirm", "--workers", "4", "--tests-dir", "tests"],
        ["mutate", "run", "--package", str(py), "--patches", "/tmp/pb-py-rs-{run}/mutants", "--out", "/tmp/pb-py-rs-w-{run}", "--no-confirm", "--workers", "4", "--timeout-ms", "60000"],
        py,
    )

    if OSS.is_dir() and (OSS / "src").is_dir():
        env = {"PYTHONPATH": str(OSS / "src")}
        add(
            "generate itsdangerous src",
            ["mutate", "generate", "--package", str(OSS), "--out", "/tmp/pb-oss-ts-{run}", "--src", "src"],
            ["mutate", "generate", "--package", str(OSS), "--out", "/tmp/pb-oss-rs-{run}", "--src", "src"],
            OSS,
            env,
        )
        add(
            "run itsdangerous, max 20 mutants, no confirm",
            ["mutate", "run", "--package", str(OSS), "--patches", "/tmp/pb-oss-ts-{run}/mutants", "--out", "/tmp/pb-oss-ts-run-{run}", "--no-confirm", "--max-mutants", "20", "--tests-dir", "tests"],
            ["mutate", "run", "--package", str(OSS), "--patches", "/tmp/pb-oss-rs-{run}/mutants", "--out", "/tmp/pb-oss-rs-run-{run}", "--no-confirm", "--max-mutants", "20", "--timeout-ms", "120000"],
            OSS,
            env,
        )
    else:
        rows.append(("itsdangerous", "checkout missing", "checkout missing", "", ""))

    lines = [
        "# Benchmark results",
        "",
        machine(),
        "",
        "Five runs of each command. The cell is median milliseconds, then min-max. Peak RSS is the highest VmHWM in kilobytes seen during those runs. TypeScript is c5cd175. Rust is the release binary built in this checkout. Re-running this script overwrites this file with new measurements.",
        "",
        "| Command | TypeScript ms | Rust ms | TS peak RSS kB | Rust peak RSS kB |",
        "| --- | --- | --- | --- | --- |",
    ]
    for name, ts_cell, rs_cell, ts_rss, rs_rss in rows:
        lines.append(f"| {name} | {ts_cell} | {rs_cell} | {ts_rss} | {rs_rss} |")
    lines.append("")
    lines.append("## What the runs printed")
    lines.append("")
    for name, ts_out, rs_out in notes:
        lines.append(f"### {name}")
        lines.append("")
        lines.append("TypeScript, last run, first 40 lines:")
        lines.append("")
        lines.append("```")
        lines.append("\n".join(ts_out.splitlines()[:40]))
        lines.append("```")
        lines.append("")
        lines.append("Rust, last run, first 40 lines:")
        lines.append("")
        lines.append("```")
        lines.append("\n".join(rs_out.splitlines()[:40]))
        lines.append("```")
        lines.append("")
    OUT.write_text("\n".join(lines))
    print(OUT.read_text().split("## What")[0])


if __name__ == "__main__":
    main()
