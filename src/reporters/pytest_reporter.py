#!/usr/bin/env python3
"""Run pytest and print one Probatio report as a single JSON line.

When PROBATIO_COVERAGE_MAP is set and coverage.py imports, also write a
per-test line map. A missing coverage module leaves the map unwritten.
A collection failure, or a setup failure whose text names SyntaxError,
IndentationError, ImportError, or ModuleNotFoundError, is staticLoad.
A call-phase failure is a kill even when its exception is ValueError.
"""
import json
import os
import sys

os.environ.setdefault("COVERAGE_CORE", "ctrace")

import pytest


class Collector:
    def __init__(self):
        self.names = []
        self.failed = []

    def pytest_collectreport(self, report):
        if not report.failed:
            return
        self._add(getattr(report, "nodeid", "") or "pytest", report, static_load=True)

    def pytest_runtest_logreport(self, report):
        if report.when not in ("setup", "call"):
            return
        if report.when == "setup" and not report.failed:
            return
        name = report.nodeid
        if report.when == "call" and name not in self.names:
            self.names.append(name)
        if report.failed:
            static = report.when == "setup" and self._setup_failed_to_load(report)
            self._add(name, report, static_load=static)

    def _setup_failed_to_load(self, report):
        text = _longrepr_text(report)
        return any(
            token in text
            for token in ("SyntaxError", "IndentationError", "ImportError", "ModuleNotFoundError")
        )

    def _add(self, name, report, static_load=False):
        if any(item["name"] == name for item in self.failed):
            return
        location = getattr(report, "location", None) or ("", 0, "")
        item = {"name": name, "file": str(location[0] or ""), "line": int(location[1] or 0)}
        if static_load:
            item["staticLoad"] = True
        self.failed.append(item)


class ContextSwitch:
    def __init__(self, cov):
        self.cov = cov

    def pytest_runtest_setup(self, item):
        self.cov.switch_context(item.nodeid)


def _longrepr_text(report):
    parts = []
    longrepr = getattr(report, "longrepr", None)
    if longrepr is not None:
        parts.append(str(longrepr))
        crash = getattr(longrepr, "reprcrash", None)
        if crash is not None:
            parts.append(str(getattr(crash, "message", "")))
    extra = getattr(report, "longreprtext", None)
    if extra:
        parts.append(str(extra))
    return "\n".join(parts)


def start_coverage():
    dest = os.environ.get("PROBATIO_COVERAGE_MAP")
    if not dest:
        return None, None
    try:
        import coverage
    except ImportError:
        return None, None
    data_file = dest + ".data"
    try:
        cov = coverage.Coverage(cover_pylib=False, data_file=data_file)
        cov.start()
        return cov, dest
    except Exception:
        return None, None


def write_coverage(cov, dest):
    try:
        cov.stop()
        cov.save()
    except Exception:
        return
    try:
        data = cov.get_data()
    except Exception:
        return
    root = os.path.realpath(os.getcwd())
    files = {}
    for filename in data.measured_files():
        full = os.path.realpath(filename)
        if not full.startswith(root + os.sep):
            continue
        if "/site-packages/" in full or "/node_modules/" in full:
            continue
        rel = full[len(root) + 1 :]
        try:
            by_line = data.contexts_by_lineno(filename) or {}
        except Exception:
            continue
        lines = {}
        for lineno, contexts in by_line.items():
            names = [item for item in contexts if item]
            if names:
                lines[str(lineno)] = names
        if lines:
            files[rel] = lines
    if not files:
        return
    with open(dest, "w", encoding="utf-8") as handle:
        json.dump({"files": files}, handle)
        handle.write("\n")


def main():
    args = []
    index = 1
    while index < len(sys.argv):
        if sys.argv[index] == "--pattern":
            args.extend(["-k", sys.argv[index + 1]])
            index += 2
            continue
        args.append(sys.argv[index])
        index += 1
    cov, dest = start_coverage()
    collector = Collector()
    plugins = [collector]
    if cov is not None:
        plugins.append(ContextSwitch(cov))
    try:
        pytest.main([*args, "-p", "no:cacheprovider", "--tb=no", "-q"], plugins=plugins)
    except BaseException:
        if cov is not None and dest:
            write_coverage(cov, dest)
        sys.exit(2)
    if cov is not None and dest:
        write_coverage(cov, dest)
    names = list(collector.names)
    for item in collector.failed:
        if item["name"] not in names:
            names.append(item["name"])
    failed = collector.failed
    report = {
        "tests": len(names),
        "pass": len(names) - len(failed),
        "fail": len(failed),
        "failed": failed,
        "names": names,
    }
    sys.stdout.write(json.dumps(report) + "\n")
    sys.stdout.flush()
    if report["tests"] <= 0:
        sys.exit(2)
    if report["fail"] > 0:
        sys.exit(1)
    sys.exit(0)


if __name__ == "__main__":
    main()
