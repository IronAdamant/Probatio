#!/usr/bin/env python3
"""Run unittest and print one Probatio report as a single JSON line.

A failure whose traceback is SyntaxError, IndentationError, ImportError, or
ModuleNotFoundError did not load the mutant. That failure is marked
staticLoad. An assertion, or any other exception from a test that ran, is not.

When PROBATIO_COVERAGE_MAP is set, also write a per-test line map there, with
the standard library only: sys.monitoring on Python 3.12 and later, else
sys.settrace. Each line of a file under the working directory names the tests
that ran it. A line that ran outside any one test (a module body, setUpClass)
gets an empty list, which runs the whole suite for a mutant there. Test files
are left out. If tracing fails, no map is written and the report is unchanged.
"""
import importlib.util
import json
import os
import re
import threading
import sys
import unittest
from pathlib import Path

LOAD_ERRORS = ("SyntaxError", "IndentationError", "ImportError", "ModuleNotFoundError")


def package_name(path: Path):
    """The dotted name of a test file inside a package (every folder up to the top package has
    __init__.py), e.g. tests/test_x.py -> tests.test_x. None for a file outside a package."""
    parts = [path.stem]
    folder = path.resolve().parent
    while (folder / "__init__.py").is_file():
        parts.insert(0, folder.name)
        folder = folder.parent
    return ".".join(parts) if len(parts) > 1 else None


def load(path: Path):
    # A test file inside a package loads under its real name, so code that imports it again by name
    # (a spawn-based process pool, pickle, `from tests.helpers import ...`) finds the same module.
    # The import goes through sys.path, whose first entry is the working directory (the mutant's
    # tree), so the module comes from the tree under test.
    real = package_name(path)
    if real:
        try:
            module = importlib.import_module(real)
        except ModuleNotFoundError:
            module = None
        if module is not None and Path(getattr(module, "__file__", "") or "").name == path.name:
            return module
    name = f"probatio_{path.stem}"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def walk(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from walk(item)
        else:
            yield item


def parse_args(argv):
    pattern = None
    files = []
    walked = False
    index = 0
    while index < len(argv):
        if argv[index] == "--pattern":
            pattern = argv[index + 1]
            index += 2
            continue
        if argv[index] == "--walked":
            walked = True
            index += 1
            continue
        files.append(argv[index])
        index += 1
    return pattern, files, walked


def load_failure(trace):
    last = ""
    for raw in trace.splitlines():
        line = raw.strip()
        for token in LOAD_ERRORS:
            if line.startswith(token + ":") or line.startswith(token + " "):
                last = token
    return last in LOAD_ERRORS


class LineMap:
    """Which lines of the files under `root` each test ran."""

    TOOL = 4  # the sys.monitoring tool id, chosen in start()

    def __init__(self, root, skip):
        self.root = root + os.sep
        self.skip = set(skip)
        self.wanted = {}
        self.current = None
        self.hits = {}
        self.outside = set()
        self.monitoring = False

    def keep(self, filename):
        known = self.wanted.get(filename)
        if known is None:
            full = os.path.realpath(filename) if filename and not filename.startswith("<") else ""
            known = bool(full) and full.startswith(self.root) and full not in self.skip
            known = known and os.sep + ".probatio" + os.sep not in full and "site-packages" not in full
            name = os.path.basename(full)
            known = known and not (name.startswith("test_") or name.endswith("_test.py") or name == "conftest.py")
            known = full[len(self.root):].replace(os.sep, "/") if known else ""
            self.wanted[filename] = known
        return known

    def record(self, rel, line, module=False):
        # A module body runs once, at the first import, whichever test caused it: every test
        # depends on it, so it counts as outside any one test.
        if self.current is None or module:
            self.outside.add((rel, line))
        else:
            self.current.add((rel, line))

    def start(self):
        monitoring = getattr(sys, "monitoring", None)
        if monitoring is not None:
            # Another tool (coverage.py takes 1) may hold an id: take the first free one.
            for tool in (4, 5, 3, 2):
                try:
                    monitoring.use_tool_id(tool, "probatio")
                    self.TOOL = tool
                    break
                except ValueError:
                    continue
            else:
                raise RuntimeError("no free sys.monitoring tool id")
            disable = monitoring.DISABLE

            def on_line(code, line):
                rel = self.keep(code.co_filename)
                if rel:
                    self.record(rel, line, code.co_name == "<module>")
                # Each line is seen once until restart_events at the next test, so a loop costs nothing.
                return disable

            monitoring.register_callback(self.TOOL, monitoring.events.LINE, on_line)
            monitoring.set_events(self.TOOL, monitoring.events.LINE)
            self.monitoring = True
            return

        def local(frame, event, arg):
            if event == "line":
                code = frame.f_code
                self.record(self.keep(code.co_filename), frame.f_lineno, code.co_name == "<module>")
            return local

        def tracer(frame, event, arg):
            if not self.keep(frame.f_code.co_filename):
                return None
            if event == "call":
                code = frame.f_code
                self.record(self.keep(code.co_filename), frame.f_lineno, code.co_name == "<module>")
            return local

        threading.settrace(tracer)
        sys.settrace(tracer)

    def stop(self):
        if self.monitoring:
            monitoring = sys.monitoring
            monitoring.set_events(self.TOOL, 0)
            monitoring.register_callback(self.TOOL, monitoring.events.LINE, None)
            monitoring.free_tool_id(self.TOOL)
            self.monitoring = False
        else:
            sys.settrace(None)
            threading.settrace(None)

    def begin(self, name):
        self.current = self.hits.setdefault(name, set())
        if self.monitoring:
            sys.monitoring.restart_events()

    def end(self):
        self.current = None
        if self.monitoring:
            sys.monitoring.restart_events()

    def write(self, dest):
        lines = {}
        for name, seen in self.hits.items():
            for rel, line in seen:
                lines.setdefault(rel, {}).setdefault(line, []).append(name)
        for rel, line in self.outside:
            lines.setdefault(rel, {})[line] = []
        files = {}
        for rel, rows in lines.items():
            files[rel] = {str(line): sorted(set(tests)) for line, tests in sorted(rows.items())}
        if not files:
            return
        temp = dest + ".tmp"
        with open(temp, "w", encoding="utf-8") as handle:
            json.dump({"files": files}, handle)
            handle.write("\n")
        os.replace(temp, dest)


def start_map():
    dest = os.environ.get("PROBATIO_COVERAGE_MAP")
    if not dest:
        return None, None
    try:
        skip = [os.path.realpath(__file__)] + [os.path.realpath(path) for path in sys.argv[1:] if path.endswith(".py")]
        tracker = LineMap(os.path.realpath(os.getcwd()), skip)
        tracker.start()
        return tracker, dest
    except Exception:
        return None, None


def traced_result(tracker):
    """A result class that tells the tracker which test is running."""

    class Traced(unittest.TextTestResult):
        def startTest(self, test):
            try:
                tracker.begin(test.id())
            except Exception:
                pass
            super().startTest(test)

        def stopTest(self, test):
            super().stopTest(test)
            try:
                tracker.end()
            except Exception:
                pass

    return Traced


def main():
    pattern, files, walked = parse_args(sys.argv[1:])
    tracker, dest = start_map()
    sys.path.insert(0, str(Path.cwd()))
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    compiled = re.compile(pattern) if pattern else None
    try:
        if files:
            for file in files:
                suite.addTest(loader.loadTestsFromModule(load(Path(file))))
        elif not walked:
            suite.addTest(loader.discover(".", pattern="test*.py"))
    except Exception as exc:
        name = type(exc).__name__
        static = name in LOAD_ERRORS or load_failure(str(exc))
        item = {"name": "discover", "file": "", "line": 0}
        if static:
            item["staticLoad"] = True
        report = {"tests": 1, "pass": 0, "fail": 1, "failed": [item], "names": ["discover"]}
        sys.stdout.write(json.dumps(report) + "\n")
        sys.exit(1)
    cases = []
    for case in walk(suite):
        if case is None:
            continue
        if compiled is None or compiled.fullmatch(case.id()):
            cases.append(case)
    ran = unittest.TestSuite(cases)
    names = [case.id() for case in cases]
    with open(os_devnull(), "w") as stream:
        runner = unittest.TextTestRunner(stream=stream, verbosity=0)
        if tracker is not None:
            runner.resultclass = traced_result(tracker)
        result = runner.run(ran)
    if tracker is not None:
        try:
            tracker.stop()
            tracker.write(dest)
        except Exception:
            pass
    failed = []
    for case, trace in list(result.failures) + list(result.errors):
        frames = re.findall(r'File "([^"]+)", line (\d+)', trace)
        file, line = frames[-1] if frames else ("", "0")
        item = {"name": case.id(), "file": file, "line": int(line)}
        if load_failure(trace):
            item["staticLoad"] = True
        failed.append(item)
    report = {
        "tests": len(names),
        "pass": len(names) - len(failed),
        "fail": len(failed),
        "failed": failed,
        "names": names,
    }
    sys.stdout.write(json.dumps(report) + "\n")
    if report["tests"] <= 0:
        sys.exit(2)
    if report["fail"] > 0:
        sys.exit(1)
    sys.exit(0)


def os_devnull():
    import os

    return os.devnull


if __name__ == "__main__":
    main()
