# Probatio

[![npm version](https://img.shields.io/npm/v/probatio)](https://www.npmjs.com/package/probatio)

Probatio scores a test suite by the bugs it already catches. A kill is an assertion the suite had. The kill count is not a merge gate.

Node 22.6 or newer:

```bash
npm install probatio
```

```bash
npx probatio mutate generate --package . --out .probatio/generate
npx probatio mutate run --package . --patches .probatio/generate/mutants --out .probatio/runs
npx probatio mutate tally --out .probatio/runs
```

Each command prints one JSON object and exits 0 only when `ok` is true.

```json
{
  "schemaVersion": 2,
  "ok": true,
  "summary": "0 no coverage, 0 survived, 1 killed, 0 flaky, 0 timed out, 0 errored, of 1 finished.",
  "next": "No survivor in this batch.",
  "nextCall": null
}
```

Every command's output has a JSON Schema in [schemas/](schemas/), and `probatio schema <command>` prints it. `schemaVersion` is 2. It changes when a field is renamed, removed, or changes meaning, and a new field does not change it.

Agents changing this repo should read [AGENTS.md](AGENTS.md). The MCP server is `probatio mcp`. Snippets are in the MCP section below. People building Probatio should read [docs/builders.md](docs/builders.md). Small suites live in [examples/](examples/).

## Status

What a result means:

- **killed**: a test the suite already had failed an assertion on the mutant. It is not proof the change is safe, and the count is not a merge gate.
- **survived**: a test ran the line and did not notice. That is a gap. Add a test that fails on the mutant, then rerun.
- **no coverage**: no test executed that line, so the suite was not started. It is not a pass and not a gap.
- **did not build** (`unviable`): the compiler rejected the mutant, or on Node a test file no longer links (a removed export, a missing module, a file that does not parse). No test code ran, so it is not a kill and not a gap. Java, Rust, Go, and C# reject the same change at compile time, so every language gets the same verdict. For a ledger fix it means the reverted commit cannot be a regression check as it stands: write a hand-made mutant (`source=hand`) that keeps the API and puts the old behaviour back.
- **timed out**: the baseline clock ran out. It is not a kill.
- A red baseline, or a baseline with no test report, stops before the first mutant.

What runs today:

- Discovery finds the project's own suite: `run_tests.sh`, Cargo, Go, Swift, Maven, dotnet, pytest, unittest, node:test, Mocha, and `make test` for COBOL. An unknown layout asks for `--suite-command`.
- Node, pytest, Go, Maven/Java, Rust, C#, Mocha, and C with LLVM coverage collect a line map on the baseline. A mutant runs only the tests that hit its line.
- A Node child process started by a test is mapped under that test, with no import in the child and even when it ends with `process.exit`. A child started with a cleared environment cannot be seen and stays `no coverage`.
- A slow suite with no line map (baseline 5 seconds or more, more than 30 mutants pending) stops before the first mutant.
- A project that needs its own environment or test-line format says so in `.probatio.json` (below). Probatio does not patch a project's files or environment to make one repository pass.

Sealed runs. A sealed run hides the test that came with a fix, puts the bug back, and asks whether the rest of the suite notices. Seven subjects are recorded (2026-10-05 and 10-06):

| Subject | Result |
|---|---|
| QuixBugs, Python program | no coverage: the planted line was not executed |
| QuixBugs, Java program | survived |
| Commons CSV, one bug | survived |
| tqdm, one bug | survived |
| First one-command `mutate sealed` run | survived, both runs |
| Commons CSV `c15a06ee` (CSV-288) | the suite stayed green with the test hidden |
| Commons CSV `2c83a308` (CSV-271) | killed on 10-06 with no line map; survived on 10-08 with the map; see below |

The CSV-271 kill came from `CSVFormatTest.testFormatThrowsNullPointerException`. The fix commit itself edited that test: it changed the asserted stack-frame class from `CSVFormat` to `java.util.Objects`. With the test as it was before the fix, all 92 `CSVFormatTest` tests pass on the reverted code, and no other test fails because of the bug. So no older test caught it. The honest record is that these suites did not notice a hidden real bug, which is what a sealed run is for. A kill that pins an internal detail, such as which class threw, is not the same as a test that checks behaviour.

Known limits:

- The sealed list (`seal`) is a plain id list in the state dir. An agent that can read that directory can see it. Keep the state dir and every `mutate sealed` label file outside the workspace of the agents being scored.
- `mutate sealed` checks whether the fix commit edited the killing test (`fixEdited`, `olderTestCatch`). The fix commit is the scored commit, or `--fix <commit>` when later work sits on top of it. It matches a kill to a file by path, class name, or a test name written in that file. A test renamed by the fix can slip past that match.
- The Java line map comes from JaCoCo, which does not count a line as executed when a call on that line throws. A test that reaches a line only through an exception is not selected for it, so a mutant there can survive a test that would fail. On Commons CSV-271 the map leaves out `testFormatThrowsNullPointerException` at `CSVPrinter.java:284`, and the sealed run reports survived.
- The default operator set (`core`) is small: condition swaps, relational swaps, booleans, a dropped `!`. `--operators wide` adds arithmetic, integer constants, and (TypeScript only) a dropped call statement, so a clean batch means more. A summary says which set ran.

The package version in this repository is in `package.json`. The npm badge above is the published version.

## mutate

One JSON object on stdout. `schemaVersion`, `ok`, `summary`, `next`, `nextCall`. Exit 0 only when `ok` is true. Human text is `--human`. A run checks the suite out in a throwaway worktree, so the checkout you are in stays as it is.

```bash
npx probatio mutate generate --package . --out .probatio/generate
npx probatio mutate run --package . --patches .probatio/generate/mutants --out .probatio/runs
npx probatio mutate tally --out .probatio/runs
```

`--operators wide` (on `generate`, `verify-change`, and `golden compare`) adds arithmetic swaps (`+`/`-`, `*`/`/`, `%`), integer constants (`0`→`1`, `1`→`0`, `n`→`n+1`), and in TypeScript a dropped call statement (`log(x)` becomes `void 0`). Text languages need the operator spaced on both sides, so `++`, `+=`, `->`, `//`, `**`, unary minus, and pointer stars are left alone, and hex, float, and suffixed literals are not constants. COBOL and assembly get no wide operators. `core` is the default, so ids and counts from earlier runs do not move. `mutants.json` records the set.

`generate` writes operator mutants (conditions, `&&`/`||`, `===`/`!==`, boundaries, booleans, a dropped `!`). The same class is written for COBOL (`.cob`, `.cbl`), Rust, C, C++, Java, Go, Python, JavaScript, and C#. Strings and comments are not mutated. A COBOL `*>` comment and a fixed-format line whose column 7 is `*` or `/` are comments. Each patch is forward: apply it to introduce the bug, and the file says so. When mutants were written, `next` states the count, the suite command if one was discovered, and whether the first run collects a line map.

`run` discovers the suite: `run_tests.sh`, Cargo, Go, Swift, Maven, dotnet, pytest, unittest, node:test, Mocha, then `make test` when COBOL tests sit under that Makefile. An unknown layout stops and asks for `--suite-command`. It does not compile one file and call that the suite. `make test` that would curl or wget a missing file stops, and nothing is fetched. A worktree that has no `node_modules` uses the main checkout's. Omitting `--build` runs no build. Pass `--build` with a command when the suite needs one first.

Discovery reads the package root. An example project inside it, such as a directory with its own `go.mod` or `pytest.ini`, is a different project and is not collected. Go needs a `go.mod` at or above the package.

Node, pytest, Go, Maven/Java, Rust, C#, and Mocha collect a line map on the baseline. C collects one when LLVM coverage was instrumented. Mocha writes its own `{ files }` record from root hooks, keyed by the Mocha title. The node:test collector is a different file and does not name Mocha tests. The map stores the file, the line, and the test names that hit that line. A later mutant runs only those tests (`go test -run`, Maven `-Dtest`, `cargo test`, `dotnet test --filter`, Mocha `--grep`). A line no test executed is `no coverage`, and the suite is not started. A Node child process started by a mapped test loads a small collector through `NODE_OPTIONS`. It writes the lines it ran when it exits, including after `process.exit`, and the parent test stores them under its own name. Each test process has its own dump directory, so parallel test files do not take each other's children. A child started with a cleared environment stays `no coverage`. On Node, a test filter that matches nothing is an error, not a survivor, and only the requested tests can be credited with a kill. A test file that fails before any test runs is one of two things. If it does not link or parse (a removed export, a missing module), no code ran, and the mutant is `unviable` unless some other test failed. If its own top-level code ran the mutated program and threw, that is a kill by that file, and confirm reruns the whole file. Covered mutants of one Node file are applied one after another in the suite process already running, and the line map is reset between them. `--workers` stays 1. A crash or a process left dirty ends that process. The next mutant starts clean. A baseline of 5 seconds or more, with no line map and more than 30 mutants still pending, stops before the first mutant. `next` names that baseline and tells you to narrow `--src` or pass a smaller patch directory. A one-file C, C++, Java, COBOL, or assembly launcher is not a suite discovery returns.

The mutant timeout is the baseline duration times 5, and at least 20 seconds, capped by `--suite-timeout-ms`. A timeout is not a kill. `--budget-ms` stops mutant work after the baseline. The first mutant still runs, except for that no-line-map stop. A kill is the test that failed. A mutant the compiler rejects is `unviable`: the summary says `N did not build`, `unviable` and `unviableIds` list them, and they are neither kills nor gaps. Confirm is on by default and reruns the failing names. Pytest names that `-k` cannot express are passed as node ids. A usage error, including pytest exit 4, is not a kill. `verify-change` does not confirm a second time. Add `--affected` to limit the file list to tests that can see the change. The default, once a line map exists, runs the tests on the changed line.

`tally` reads that run directory. The summary leads with the no-coverage count, then the survivors. `keep` names each test that killed a mutant, once, as an id `--only-test` can run. `noKillsYet` names tests that saw no mutant die in this batch. That is not a reason to delete them: one batch is not evidence, and the Auspex experiment caught 10 of 18 sealed bugs after pruning by kill evidence. `pruning.advice` stays empty and `deletedTests` is 0. A gap is a survivor. An unseen line is not a gap and not a pass. A timeout is neither a kill nor a gap.

`nextCall` writes to a fresh out dir next to the old one, because the old out dir would skip every finished mutant and repeat the old result. With a gap, it reruns the batch on `HEAD` (`<out>.after-gaps`) once the new test is committed. With no gap, it rescores the keep ids at the same commit with `--only-test` (`<out>.rescore`).

## .probatio.json

A project can say how its suite runs. The file sits at the package root and is read even when it is not committed.

```json
{
  "env": { "ARCH_NATIVE": "1" },
  "testLine": "^(Testing .+|Running .+)$",
  "passLine": "All done, tests as expected"
}
```

`env` is added to the suite's environment, for a flag the project's own CI sets. `testLine` names the tests of a `run_tests.sh` suite: each matching line is a test, and the first capture group is the name when there is one. When the suite exits non-zero, the last test line is the one that failed, unless `passLine` was printed. Without `testLine`, a shell suite that prints TAP (`ok 1 - name`, `not ok 2 - name`) is read as TAP. Otherwise it is one command: exit 0 passes, and a failure is `::command`, not an invented test name. An invalid file stops the run and names the field.

## ledger

```bash
npx probatio ledger build --package . --commit HEAD --out .probatio/ledger
```

A fix commit has a `Fixes-bug:` trailer, or it changes both `src` and a test. A commit that changes the version in `package.json`, `Cargo.toml`, `pyproject.toml`, or `setup.cfg` is a release, and it is not a fix unless it has the trailer: reverting it would put back several changes, not one bug. `ledger build` reverts that commit's src diff onto the tree you name. A diff that applies is written as a forward patch (`source=history`): apply it to put the bug back. A diff that does not apply is reported, and left for a hand-made patch in the same directory (`source=hand`, `fix=<commit>`). A rebuild keeps those hand-made files. Nothing is fuzzy-applied. A src diff over `--max-lines` (default 300) is skipped. `--max-commits N` reads only the newest N non-merge commits and says when older history was not scanned. The default reads the whole history. A tree is checked out only when a fix inside that cap has to be applied.

### ledger check

```bash
npx probatio ledger check --package . --ledger ledger --out .probatio/ledger-check
```

Runs every ledger bug that applies (history reverts and hand-made mutants) against the suite, with confirm on, and compares each outcome with `ledger/<id>.golden.json`. A bug that was caught and is not caught now is a regression: `ok` is false and `next` says to find the test that stopped guarding it. Any other change (a survivor now caught, a patch that no longer applies) also fails until it is re-recorded with `--update`, and a changed row needs `Golden-Change: <id>: <why>` in `--message`. A bug with no golden yet is recorded by `--update`. A hand-made mutant whose header says `source=hand fix=<commit>` replaces that fix's history revert, which is how a revert that does not build still guards its bug. Run it where the toolchain is complete: a test that skips for a missing tool can turn a kill into a survivor. This repository runs it on every push to `main` and nightly (`.github/workflows/self-score.yml`).

## matrix

```bash
npx probatio matrix report --scores round2-union-scores.tsv --direct round2-direct-scores.txt --patches mutants/holdout --patches mutants/train --out .probatio/matrix
```

Reads those recorded tables. It does not rerun the mutants. The headline is the gap list: `file:line`, the top 10, how many are left, and the path to the full JSON. A test that killed nothing can be named. The file is not deleted.

## golden

```bash
npx probatio golden check --recorded table.json --actual now.json --update wording
```

`next` and `nextLead` are wording. `ok`, `reason`, `status`, `hostChanged`, and `nextCall` are the contract. `--update wording` re-records a changed `next` or `nextLead` and leaves the contract fields alone. A file whose case names are the top-level keys is written back in that shape. A changed `ok` or `reason` also needs `Golden-Change: <row>: <why>` in `--message`. A home path, or `ok` without a reason, fails and nothing is written.

### golden record and golden compare

```bash
npx probatio golden record --package . --module src/text.ts --tests tests/text.test.ts
npx probatio golden compare --package . --module src/text.ts --tests tests/text.test.ts --golden tests/golden/text.golden.test.ts --out .probatio/golden-compare --operators wide
```

`golden record` runs those unit tests once, in a throwaway worktree, with every exported function of the module wrapped. Each call whose arguments and result are plain data becomes a row in `tests/golden/<module>.golden.json`: the function, the arguments, and what it returned, resolved, or threw. A module that reads the clock gets the recorded instant on each row, and the replay pins it. A call with a callback, a class instance, or another hidden input is counted and left out: the tests built on it stay as code. A call that answered two ways for the same arguments is left out as unstable. Recording from a red run is refused. It also writes `tests/golden/<module>.golden.test.ts`, a node:test file that replays every row and does not import Probatio. A changed row fails. That is a behaviour change: fix the code, or re-record and review the JSON diff.

`golden compare` mutates the module and scores the same mutants twice, once with only the unit tests and once with only the replay, with confirm on. It reports how many of the unit tests' kills the table also makes, and lists the mutants only the unit tests kill. When that list is empty, the table catches what those tests catch on these mutants. Replacing them is still a decision: keep any test with hidden inputs, run a sealed or ledger check, and give the reason in the commit. Nothing is deleted. Node and TypeScript ESM modules only for now.

## status

```bash
npx probatio status --state .probatio --page PROBATIO.md
```

Rewrites `PROBATIO.md` from the same JSON: open gaps, recent fixes and their guards, protected guards, decisions. A sealed id is left out of the page, the JSON, and `queue seed`. If visible kills rise and sealed kills do not, the summary is `these tests fit the yardstick, not the code.`

## verify-change

```bash
npx probatio verify-change --package . --base HEAD~1 --out .probatio/verify
```

Lists the tests that can see the diff, runs mutants on the changed lines, and reports caught, missed, mutants that were not run, lines with no coverage, and any golden contract change. The mutants are the same operator class `mutate generate` would write, including a non-TypeScript diff. A string or a comment is not a mutant. `ran` is still the direct-importer list. The tests that execute are chosen by the line map, the same way `mutate run` chooses them, including when that importer list is empty. No discovered suite leaves those mutants in `notRun` and does not start a run. The suite timeout is 10 minutes. `--max-tests` only shortens the reported importer list. `--max-mutants` (default 4) caps how many changed lines are run.

## queue

An item is `.probatio/queue/<id>.json`. `queue claim` renames it to `claimed/<agent>-<id>.json` with a lease. `queue reap` moves an expired lease back. `check-kill <id>` runs the tests that directly import the mutated file and confirms the killing test again. It is done only when that mutant dies and that suite is still green. `verify-change` is the edit-sized run: it uses the line map and does not confirm a second time.

## MCP

`probatio mcp` and the `probatio-mcp` bin speak stdio JSON-RPC, one JSON object per line. The tool name is `probatio`. `argv` is the CLI words. `ping` returns `{}`. An unknown method returns JSON-RPC `-32601` with the same id. A tool call runs in the background, so `ping` and other calls are answered while a long `mutate run` works. `notifications/cancelled` with that call's `requestId` stops the command and its suite, removes its worktrees, and sends no reply. The server answers with the client's `protocolVersion` when it is `2024-11-05`, `2025-03-26`, or `2025-06-18`.

```json
{
  "mcpServers": {
    "probatio": {
      "command": "probatio",
      "args": ["mcp"]
    }
  }
}
```

That block is the Claude, Cursor, and Grok shape. The tool runs the CLI and returns that command's JSON. `nextCall.argv` keeps absolute paths so a later spawn does not have to expand `~`.

`mutate generate` reads the commit (`HEAD` unless you pass `--commit`). `--working-tree` reads the checkout on disk and says so. `mutate run` scores the commit either way.

`verify-change` does not score uncommitted edits when `--base` and `--commit` are the same. A clean empty diff says `No diff-scoped mutant.`

## Releases

[docs/releasing.md](docs/releasing.md): green `test` and `self-score` on the version commit, a pushed `v<version>` tag, then `npm publish`. `prepublishOnly` refuses anything else.
