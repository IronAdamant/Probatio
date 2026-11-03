# Benchmark results

Linux 6.12.94+ x86_64, 4 cores, Intel(R) Xeon(R) Processor, 16387280 kB. Rust binary /agent/repos/Probatio/target/release/probatio. TypeScript cli /tmp/probatio-ts/dist/cli.js under node.

Five runs of each command. The cell is median milliseconds, then min-max. Peak RSS is the highest VmHWM in kilobytes seen during those runs. TypeScript is c5cd175. Rust is the release binary built in this checkout. Node was 22.23.4 and pytest was 9.2.0. itsdangerous is a depth-1 clone of pallets/itsdangerous; `freezegun` was installed so the suite could collect. Kill counts matched: JS and Python samples were 2 killed and 4 survived of 6, and itsdangerous was 7 killed and 3 survived of 10. Generate counts matched too (6, 6, and 10).

| Command | TypeScript ms | Rust ms | TS peak RSS kB | Rust peak RSS kB |
| --- | --- | --- | --- | --- |
| startup schema | 287 (287-297) | 11 (11-11) | 119936 | 1116 |
| generate JS sample | 307 (307-308) | 11 (11-11) | 121836 | 1104 |
| generate Python sample | 307 (297-308) | 11 (11-11) | 122316 | 1120 |
| run JS sample, no confirm | 800 (791-803) | 723 (722-806) | 124232 | 59708 |
| run Python sample, no confirm | 1429 (1418-1450) | 1130 (1090-1171) | 122440 | 32272 |
| run Python sample, workers 4, no confirm | 805 (796-851) | 491 (471-495) | 123144 | 32220 |
| generate itsdangerous src | 369 (360-371) | 21 (21-21) | 131304 | 3620 |
| run itsdangerous, max 20 mutants, no confirm | 7568 (7528-7625) | 7003 (6995-7049) | 133508 | 42552 |

## What the runs printed

### startup schema

TypeScript, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "schema",
  "summary": "24 commands have a published schema.",
  "next": "Pass one name, for example: probatio schema mutate.run",
  "nextCall": null,
  "commands": [
    "check-kill",
    "findings.add",
    "gap.fix",
    "gap.revert",
    "golden.check",
    "golden.compare",
    "golden.record",
    "guard.check",
    "ledger.build",
    "ledger.check",
    "matrix.report",
    "mcp",
    "mutate.generate",
    "mutate.help",
    "mutate.run",
    "mutate.sealed",
    "mutate.tally",
    "queue.claim",
    "queue.reap",
    "queue.seed",
    "schema",
    "seal",
    "status",
    "verify-change"
  ]
}
```

Rust, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "schema",
  "summary": "24 commands have a published schema.",
  "next": "Pass one name, for example: probatio schema mutate.run",
  "nextCall": null,
  "commands": [
    "check-kill",
    "findings.add",
    "gap.fix",
    "gap.revert",
    "golden.check",
    "golden.compare",
    "golden.record",
    "guard.check",
    "ledger.build",
    "ledger.check",
    "matrix.report",
    "mcp",
    "mutate.generate",
    "mutate.help",
    "mutate.run",
    "mutate.sealed",
    "mutate.tally",
    "queue.claim",
    "queue.reap",
    "queue.seed",
    "schema",
    "seal",
    "status",
    "verify-change"
  ]
}
```

### generate JS sample

TypeScript, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.generate",
  "summary": "6 mutants in 1 files. No string-literal mutant.",
  "next": "6 mutants. Suite is node --test. The first run pays that suite once, then a line map narrows each mutant.",
  "nextCall": {
    "argv": [
      "mutate",
      "run",
      "--package",
      "/tmp/probatio-bench-js",
      "--patches",
      "/tmp/pb-js-ts-4/mutants",
      "--out",
      "/tmp/pb-js-ts-4/runs"
    ]
  },
  "mutantCount": 6,
  "operators": "core",
  "stringLiteralMutants": 0,
  "filesVisited": 1,
  "budgetHit": false,
  "mutants": [
    {
      "id": "m38be612d0ee2",
      "file": "src/calc.js",
      "line": 7,
      "op": "and-to-or"
    },
    {
      "id": "me96bde61bf73",
      "file": "src/calc.js",
      "line": 2,
      "op": "lt-to-le"
    },
    {
      "id": "m6fae2f896cfc",
      "file": "src/calc.js",
      "line": 6,
```

Rust, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.generate",
  "summary": "6 mutants in 1 files. No string-literal mutant.",
  "next": "6 mutants. Suite is node --test.",
  "nextCall": {
    "argv": [
      "mutate",
      "run",
      "--package",
      "/tmp/probatio-bench-js",
      "--patches",
      "/tmp/pb-js-rs-4/mutants",
      "--out",
      "/tmp/pb-js-rs-4/runs"
    ]
  },
  "mutantCount": 6,
  "operators": "core",
  "stringLiteralMutants": 0,
  "filesVisited": 1,
  "budgetHit": false,
  "mutants": [
    {
      "id": "m38be612d0ee2",
      "file": "src/calc.js",
      "line": 7,
      "op": "and-to-or"
    },
    {
      "id": "me96bde61bf73",
      "file": "src/calc.js",
      "line": 2,
      "op": "lt-to-le"
    },
    {
      "id": "m6fae2f896cfc",
      "file": "src/calc.js",
      "line": 6,
```

### generate Python sample

TypeScript, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.generate",
  "summary": "6 mutants in 1 files. No string-literal mutant.",
  "next": "6 mutants. Suite is python3 pytest. The first run pays that suite once, then a line map narrows each mutant.",
  "nextCall": {
    "argv": [
      "mutate",
      "run",
      "--package",
      "/tmp/probatio-bench-py",
      "--patches",
      "/tmp/pb-py-ts-4/mutants",
      "--out",
      "/tmp/pb-py-ts-4/runs"
    ]
  },
  "mutantCount": 6,
  "operators": "core",
  "stringLiteralMutants": 0,
  "filesVisited": 1,
  "budgetHit": false,
  "mutants": [
    {
      "id": "mb2796bbfdfe2",
      "file": "src/calc.py",
      "line": 12,
      "op": "and-to-or"
    },
    {
      "id": "md6b4d0692bfb",
      "file": "src/calc.py",
      "line": 2,
      "op": "lt-to-le"
    },
    {
      "id": "m33130cdb735a",
      "file": "src/calc.py",
      "line": 9,
```

Rust, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.generate",
  "summary": "6 mutants in 1 files. No string-literal mutant.",
  "next": "6 mutants. Suite is python3 -m pytest.",
  "nextCall": {
    "argv": [
      "mutate",
      "run",
      "--package",
      "/tmp/probatio-bench-py",
      "--patches",
      "/tmp/pb-py-rs-4/mutants",
      "--out",
      "/tmp/pb-py-rs-4/runs"
    ]
  },
  "mutantCount": 6,
  "operators": "core",
  "stringLiteralMutants": 0,
  "filesVisited": 1,
  "budgetHit": false,
  "mutants": [
    {
      "id": "mb2796bbfdfe2",
      "file": "src/calc.py",
      "line": 12,
      "op": "and-to-or"
    },
    {
      "id": "md6b4d0692bfb",
      "file": "src/calc.py",
      "line": 2,
      "op": "lt-to-le"
    },
    {
      "id": "m33130cdb735a",
      "file": "src/calc.py",
      "line": 9,
```

### run JS sample, no confirm

TypeScript, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.run",
  "summary": "0 no coverage, 4 survived, 2 killed, 0 flaky, 0 timed out, 0 errored, of 6 finished.",
  "next": "First gap is m3a4db39054e1 at src/calc.js:3. A survivor is not a pass.",
  "nextCall": {
    "argv": [
      "mutate",
      "tally",
      "--out",
      "/tmp/pb-js-ts-run-4"
    ]
  },
  "killed": 2,
  "unviable": 0,
  "unviableIds": [],
  "survived": 4,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "noCoverage": 0,
  "notStarted": [],
  "suiteCommand": "node --test",
  "baselineMs": 113,
  "commands": [
    {
      "id": "m38be612d0ee2",
      "outcome": "killed",
      "command": "node --test --test-name-pattern=both|clamp lo test/calc.test.js",
      "wholeSuite": false,
      "reused": false,
      "cause": "test",
      "next": "Killed by test/calc.test.js::both."
    },
    {
      "id": "m3a4db39054e1",
      "outcome": "survived",
      "command": "node --test --test-name-pattern=clamp hi test/calc.test.js",
      "wholeSuite": false,
```

Rust, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.run",
  "summary": "0 no coverage, 4 survived, 2 killed, 0 flaky, 0 timed out, 0 errored, of 6 finished.",
  "next": "Each mutant is a file under results/. A kill is an assertion failure. The line map marked covered lines. The suite ran whole. Run mutate tally on this out directory. The suite command was node --test.",
  "nextCall": {
    "argv": [
      "mutate",
      "tally",
      "--out",
      "/tmp/pb-js-rs-run-4"
    ]
  },
  "suiteCommand": "node --test",
  "counts": {
    "noCoverage": 0,
    "survived": 4,
    "killed": 2,
    "unviable": 0,
    "flaky": 0,
    "timedOut": 0,
    "errored": 0,
    "finished": 6
  },
  "finished": 6,
  "killed": 2,
  "survived": 4,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "noCoverage": 0,
  "unviable": 0
}
```

### run Python sample, no confirm

TypeScript, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.run",
  "summary": "0 no coverage, 4 survived, 2 killed, 0 flaky, 0 timed out, 0 errored, of 6 finished.",
  "next": "First gap is m33130cdb735a at src/calc.py:9. A survivor is not a pass.",
  "nextCall": {
    "argv": [
      "mutate",
      "tally",
      "--out",
      "/tmp/pb-py-ts-run-4"
    ]
  },
  "killed": 2,
  "unviable": 0,
  "unviableIds": [],
  "survived": 4,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "noCoverage": 0,
  "notStarted": [],
  "suiteCommand": "python3 pytest",
  "baselineMs": 152,
  "commands": [
    {
      "id": "m33130cdb735a",
      "outcome": "survived",
      "command": "python3 pytest",
      "wholeSuite": false,
      "reused": false,
      "cause": null,
      "next": ""
    },
    {
      "id": "mb2796bbfdfe2",
      "outcome": "killed",
      "command": "python3 pytest",
      "wholeSuite": false,
```

Rust, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.run",
  "summary": "0 no coverage, 4 survived, 2 killed, 0 flaky, 0 timed out, 0 errored, of 6 finished.",
  "next": "Each mutant is a file under results/. A kill is an assertion failure. The suite ran whole. coverage.py did not write a map. Run mutate tally on this out directory. The suite command was python3 -m pytest.",
  "nextCall": {
    "argv": [
      "mutate",
      "tally",
      "--out",
      "/tmp/pb-py-rs-run-4"
    ]
  },
  "suiteCommand": "python3 -m pytest",
  "counts": {
    "noCoverage": 0,
    "survived": 4,
    "killed": 2,
    "unviable": 0,
    "flaky": 0,
    "timedOut": 0,
    "errored": 0,
    "finished": 6
  },
  "finished": 6,
  "killed": 2,
  "survived": 4,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "noCoverage": 0,
  "unviable": 0
}
```

### run Python sample, workers 4, no confirm

TypeScript, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.run",
  "summary": "0 no coverage, 4 survived, 2 killed, 0 flaky, 0 timed out, 0 errored, of 6 finished.",
  "next": "First gap is m33130cdb735a at src/calc.py:9. A survivor is not a pass.",
  "nextCall": {
    "argv": [
      "mutate",
      "tally",
      "--out",
      "/tmp/pb-py-ts-w-4"
    ]
  },
  "killed": 2,
  "unviable": 0,
  "unviableIds": [],
  "survived": 4,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "noCoverage": 0,
  "notStarted": [],
  "suiteCommand": "python3 pytest",
  "baselineMs": 147,
  "commands": [
    {
      "id": "m33130cdb735a",
      "outcome": "survived",
      "command": "python3 pytest",
      "wholeSuite": false,
      "reused": false,
      "cause": null,
      "next": ""
    },
    {
      "id": "mb2796bbfdfe2",
      "outcome": "killed",
      "command": "python3 pytest",
      "wholeSuite": false,
```

Rust, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.run",
  "summary": "0 no coverage, 4 survived, 2 killed, 0 flaky, 0 timed out, 0 errored, of 6 finished.",
  "next": "Each mutant is a file under results/. A kill is an assertion failure. The suite ran whole. coverage.py did not write a map. Run mutate tally on this out directory. The suite command was python3 -m pytest.",
  "nextCall": {
    "argv": [
      "mutate",
      "tally",
      "--out",
      "/tmp/pb-py-rs-w-4"
    ]
  },
  "suiteCommand": "python3 -m pytest",
  "counts": {
    "noCoverage": 0,
    "survived": 4,
    "killed": 2,
    "unviable": 0,
    "flaky": 0,
    "timedOut": 0,
    "errored": 0,
    "finished": 6
  },
  "finished": 6,
  "killed": 2,
  "survived": 4,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "noCoverage": 0,
  "unviable": 0
}
```

### generate itsdangerous src

TypeScript, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.generate",
  "summary": "10 mutants in 8 files. No string-literal mutant.",
  "next": "10 mutants. Suite is python3 pytest. The first run pays that suite once, then a line map narrows each mutant.",
  "nextCall": {
    "argv": [
      "mutate",
      "run",
      "--package",
      "/tmp/itsdangerous",
      "--patches",
      "/tmp/pb-oss-ts-4/mutants",
      "--out",
      "/tmp/pb-oss-ts-4/runs"
    ]
  },
  "mutantCount": 10,
  "operators": "core",
  "stringLiteralMutants": 0,
  "filesVisited": 8,
  "budgetHit": false,
  "mutants": [
    {
      "id": "md86ad2406dd4",
      "file": "src/itsdangerous/serializer.py",
      "line": 234,
      "op": "or-to-and"
    },
    {
      "id": "m63a72a4a3e81",
      "file": "src/itsdangerous/serializer.py",
      "line": 226,
      "op": "or-to-and"
    },
    {
      "id": "m47b51b94b65d",
      "file": "src/itsdangerous/signer.py",
      "line": 210,
```

Rust, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.generate",
  "summary": "10 mutants in 8 files. No string-literal mutant.",
  "next": "10 mutants. Suite is python3 -m pytest.",
  "nextCall": {
    "argv": [
      "mutate",
      "run",
      "--package",
      "/tmp/itsdangerous",
      "--patches",
      "/tmp/pb-oss-rs-4/mutants",
      "--out",
      "/tmp/pb-oss-rs-4/runs"
    ]
  },
  "mutantCount": 10,
  "operators": "core",
  "stringLiteralMutants": 0,
  "filesVisited": 8,
  "budgetHit": false,
  "mutants": [
    {
      "id": "md86ad2406dd4",
      "file": "src/itsdangerous/serializer.py",
      "line": 234,
      "op": "or-to-and"
    },
    {
      "id": "m63a72a4a3e81",
      "file": "src/itsdangerous/serializer.py",
      "line": 226,
      "op": "or-to-and"
    },
    {
      "id": "m47b51b94b65d",
      "file": "src/itsdangerous/signer.py",
      "line": 210,
```

### run itsdangerous, max 20 mutants, no confirm

TypeScript, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.run",
  "summary": "0 no coverage, 3 survived, 7 killed, 0 flaky, 0 timed out, 0 errored, of 10 finished.",
  "next": "First gap is m0fb9066ba3c0 at src/itsdangerous/timed.py:148. A survivor is not a pass.",
  "nextCall": {
    "argv": [
      "mutate",
      "tally",
      "--out",
      "/tmp/pb-oss-ts-run-4"
    ]
  },
  "killed": 7,
  "unviable": 0,
  "unviableIds": [],
  "survived": 3,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "noCoverage": 0,
  "notStarted": [],
  "suiteCommand": "python3 pytest",
  "baselineMs": 627,
  "commands": [
    {
      "id": "m0fb9066ba3c0",
      "outcome": "survived",
      "command": "python3 pytest",
      "wholeSuite": false,
      "reused": false,
      "cause": null,
      "next": ""
    },
    {
      "id": "m183cd86e2e02",
      "outcome": "survived",
      "command": "python3 pytest",
      "wholeSuite": false,
```

Rust, last run, first 40 lines:

```
{
  "schemaVersion": 2,
  "ok": true,
  "command": "mutate.run",
  "summary": "0 no coverage, 3 survived, 7 killed, 0 flaky, 0 timed out, 0 errored, of 10 finished.",
  "next": "Each mutant is a file under results/. A kill is an assertion failure. The suite ran whole. coverage.py did not write a map. Run mutate tally on this out directory. The suite command was python3 -m pytest.",
  "nextCall": {
    "argv": [
      "mutate",
      "tally",
      "--out",
      "/tmp/pb-oss-rs-run-4"
    ]
  },
  "suiteCommand": "python3 -m pytest",
  "counts": {
    "noCoverage": 0,
    "survived": 3,
    "killed": 7,
    "unviable": 0,
    "flaky": 0,
    "timedOut": 0,
    "errored": 0,
    "finished": 10
  },
  "finished": 10,
  "killed": 7,
  "survived": 3,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "noCoverage": 0,
  "unviable": 0
}
```
