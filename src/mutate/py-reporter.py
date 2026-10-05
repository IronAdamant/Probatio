#!/usr/bin/env python3
"""Run unittest modules and print one Probatio report as a single JSON line."""
import importlib.util
import json
import os
import re
import sys
import unittest
from pathlib import Path


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


def main():
    pattern, files = parse_args(sys.argv[1:])
    sys.path.insert(0, str(Path.cwd()))
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    compiled = re.compile(pattern) if pattern else None
    for file in files:
        loaded = loader.loadTestsFromModule(load(Path(file)))
        if compiled is None:
            suite.addTest(loaded)
        else:
            for case in walk(loaded):
                if compiled.fullmatch(case.id()):
                    suite.addTest(case)
    cases = [case for case in walk(suite) if case is not None]
    names = [case.id() for case in cases]
    with open(os.devnull, "w") as stream:
        result = unittest.TextTestRunner(stream=stream, verbosity=0).run(suite)
    failed = []
    for case, tb in list(result.failures) + list(result.errors):
        frames = re.findall(r'File "([^"]+)", line (\d+)', tb)
        file, line = frames[-1] if frames else ("", "0")
        failed.append({"name": case.id(), "file": file, "line": int(line)})
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
