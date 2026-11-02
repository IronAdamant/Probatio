#!/usr/bin/env python3
"""Run unittest and print one Probatio report as a single JSON line.

A failure whose traceback is SyntaxError, IndentationError, ImportError, or
ModuleNotFoundError did not load the mutant. That failure is marked
staticLoad. An assertion, or any other exception from a test that ran, is not.
"""
import importlib.util
import json
import re
import sys
import unittest
from pathlib import Path

LOAD_ERRORS = ("SyntaxError", "IndentationError", "ImportError", "ModuleNotFoundError")


def load(path: Path):
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
    index = 0
    while index < len(argv):
        if argv[index] == "--pattern":
            pattern = argv[index + 1]
            index += 2
            continue
        files.append(argv[index])
        index += 1
    return pattern, files


def load_failure(trace):
    last = ""
    for raw in trace.splitlines():
        line = raw.strip()
        for token in LOAD_ERRORS:
            if line.startswith(token + ":") or line.startswith(token + " "):
                last = token
    return last in LOAD_ERRORS


def main():
    pattern, files = parse_args(sys.argv[1:])
    sys.path.insert(0, str(Path.cwd()))
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    compiled = re.compile(pattern) if pattern else None
    try:
        if files:
            for file in files:
                suite.addTest(loader.loadTestsFromModule(load(Path(file))))
        else:
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
        result = unittest.TextTestRunner(stream=stream, verbosity=0).run(ran)
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
