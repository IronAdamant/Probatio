#!/usr/bin/env python3
"""Run pytest and print one Probatio report as a single JSON line.

When PROBATIO_COVERAGE_MAP is set and coverage.py imports, also write a
per-test line map. A missing coverage module leaves the map unwritten.
"""
import json
import os
import sys

# The sysmon core on this interpreter drops dynamic context switches.
os.environ.setdefault("COVERAGE_CORE", "ctrace")

import pytest


class Collector:
    def __init__(self):
        self.names = []
        self.failed = []

    def pytest_collectreport(self, report):
        if not report.failed:
            return
        self._add(getattr(report, "nodeid", "") or "pytest", report)

    def pytest_runtest_logreport(self, report):
        if report.when not in ("setup", "call"):
            return
        if report.when == "setup" and not report.failed:
            return
        name = report.nodeid
        if report.when == "call" and name not in self.names:
            self.names.append(name)
        if report.failed:
            self._add(name, report)

    def _add(self, name, report):
        if any(item["name"] == name for item in self.failed):
            return
        location = getattr(report, "location", None) or ("", 0, "")
        self.failed.append({"name": name, "file": str(location[0] or ""), "line": int(location[1] or 0)})


class ContextSwitch:
    def __init__(self, cov):
        self.cov = cov

    def pytest_runtest_setup(self, item):
        self.cov.switch_context(item.nodeid)


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
    code = 1
    try:
        code = pytest.main([*args, "-p", "no:cacheprovider", "--tb=no", "-q"], plugins=plugins) or 0
    except BaseException:
        code = 1
    finally:
        if cov is not None and dest:
            write_coverage(cov, dest)
    names = collector.names or [item["name"] for item in collector.failed]
    failed = collector.failed
    # Exit 4 is a usage error (a bad -k expression). No test ran, so this is not a kill.
    if not names and code != 0:
        names = []
        failed = []
    report = {
        "tests": len(names),
        "pass": len(names) - len(failed),
        "fail": len(failed),
        "failed": failed,
        "names": names,
    }
    sys.stdout.write(json.dumps(report) + "\n")


if __name__ == "__main__":
    main()
