# Probatio reference

The detail behind [README.md](../README.md). The agent contract is [AGENTS.md](../AGENTS.md), and each command's flags and output fields are in [schemas/](../schemas/) or `probatio schema <command>`.

## What a command prints

Every command prints one JSON object and exits 0 only when `ok` is true. The object has `schemaVersion`, `ok`, `command`, `summary`, `next`, and `nextCall`. `schemaVersion` is 2. Read `summary` and `next`. Run `nextCall` as printed. It continues in the out directory it names.

Kill lists and the full list of mutants that were not started are files under the out directory. They are not required on stdout. A home directory that appears in `summary` or `next` is written as `~`. The argv inside `nextCall`, and the `suiteCommand` template, keep their real paths, because both are run as written.

```bash
probatio mutate generate --package . --out .probatio/generate
probatio mutate run --package . --patches .probatio/generate --out .probatio/runs --suite-command 'cargo test'
probatio mutate tally --out .probatio/runs
```

## Generate, then run, then tally

`mutate generate` walks the package and writes `mutants.json`. It does not run the suite. A mutant is a text replacement in a condition, a comparison, or a boolean. Strings and comments are left alone. The id comes from the file, the offset, the operator, and the replacement. The walk skips directories named `target`, `node_modules`, and `dist`, and anything whose name starts with `.`.

`mutate run` needs `--package`, `--patches`, and `--out`. It runs the suite once on the unchanged code. That is the baseline. Then it scores mutants that do not already have a result file.

`mutate tally` reads `results/*.json`. It is safe while a run is still going: it counts the mutants that have a result against the run's patches, from `run.json`, which `mutate run` writes before its first mutant. A gap names its `file:line`. `deletedTests` is 0. `noKillsYet` lists names that saw no kill in this batch. It is not a list of tests to delete. Tally does not delete a file.

## Where the suite command comes from

The first match wins:

1. `--suite-command`
2. A non-empty `suiteCommand` string in `.probatio.json`
3. A marker file at the package root

`env` is an object of strings and is passed to the suite. `testLine` and `passLine` are regular expressions: a shell suite names a test from each `testLine` match, and a non-zero exit blames the last of those lines unless `passLine` was printed. Any other key is ignored. If the file is not valid JSON, or a pattern does not compile, the run stops and discovery is not used. The same is true of a `package.json` that is not valid JSON.

Discovery does not launch the tool. It only names the command, from the package root:

| Marker | Command |
| --- | --- |
| `go.mod` | `go test ./...` |
| `Cargo.toml` | `cargo test` |
| `pom.xml` | `mvn -B test` |
| `package.json` with a non-empty `scripts.test` string | that string, as written |
| any other `package.json` | `node --test` |
| `pytest.ini` or `pyproject.toml` | `python3 -m pytest` |
| `Package.swift` | `swift test` |
| a root `.sln`, `.csproj`, or `.fsproj` | `dotnet test` |
| `Makefile`, `makefile`, or `GNUmakefile` | `make test` |
| `.mocharc.json`, `.mocharc.js`, `.mocharc.cjs`, `.mocharc.yml`, or `.mocharc.yaml` | `mocha` |
| `CMakeLists.txt` | `ctest --output-on-failure` |
| `test_*.py` in the package root, or in a direct child directory named `tests` | `python3 -m unittest discover` |
| a root `.cob`, `.cbl`, or `.cobol` | `cobc -xj` |
| a root `.c`, `.cc`, `.cpp`, `.h`, or `.hpp` | `make test` |
| a root `.s`, `.S`, or `.asm` | `make test` |

`Cargo.toml` is checked before `package.json`, so a package with both uses `cargo test`. If none of these markers is present, the run stops and asks for `--suite-command`.

`pytest`, a path whose last component is `pytest` (including `.venv/bin/pytest`), and `python` or `python3` followed by `-m pytest` are run by a reporter shipped in the binary. Arguments after that program are passed through. `{tests}` and `{tests,}` stay tokens. The same Python programs followed by `-m unittest discover`, with an optional `-s` directory, use the unittest reporter. That run loads `test_*.py` by file path from the package root, from `tests/`, and from `test/`, including a folder with no `__init__.py`. `-s` limits the walk to the directory given. The reporter prints one JSON line, and that line decides the score. A test that ran and failed is a kill, including a failure whose exception is `ValueError`. A mutant that fails to load (`SyntaxError`, `IndentationError`, `ImportError`, or `ModuleNotFoundError`) is "did not build". On a pytest baseline, if `coverage.py` imports, Probatio writes `<out>/lines.json` and uses it as the line map. Names from that map are arguments to the reporter, and the command does not need a `{tests}` token. If `coverage.py` does not import, the map is not written, and more than 30 mutants still stop before the first mutant. A `--line-map` you pass is kept. `make test` stays on the process exit. It is also the command for C, COBOL, and assembly. When a mutant is scored from the process exit, a crash whose text contains `error:` can be filed as did not build.

`{tests}` puts each selected name in single quotes. `{tests,}` joins the names with commas. A name may contain ASCII letters, digits, and `_` `-` `.` `/` `:`. Anything else is refused, and so is an empty name. The `suiteCommand` field on the envelope is the template, before the names are filled in. `baseline.json` stores that same template. A result written after the suite started stores the rendered line in `command`. A no-coverage result that never started the suite has no `command` field.

When the package has `node_modules/.bin`, that directory is placed on the suite process `PATH`. That includes a mutant run from a git worktree or a copy. Both skip `.git`, `target`, `node_modules`, `dist`, and `.probatio` when they are created, and both get a symlink to the package `node_modules` when they do not already have one. The command string itself is not rewritten. A shell exit 127 whose text says `command not found` or `: not found` means the tool is missing on the baseline, and it is an error on a mutant. The first is bash. The second is dash, which is `/bin/sh` on Ubuntu. The binary does not download a toolchain.

## What a result means

- **killed**: a test the suite already had failed an assertion.
- **survived**: a test ran the change and the suite did not notice. That is a gap.
- **no coverage**: nothing executed that line. Leave it. There is no evidence either way.
- **unviable**: the mutant failed to compile or link. The summary text says "did not build". A Node test file that no longer links counts here.
- **timeout**: the suite clock ran out. The clock is the baseline's clock.
- **error**: the suite process failed in some other way. Exit 127 with `command not found` or `: not found` is this.

A killed mutant is an assertion failure. Golden checks and contract checks are what say the expected result is still the expected result.

A red baseline, a baseline with no test report, a baseline timeout, or a missing tool stops before the first mutant. That envelope has no kill count.

## When a run stops

The suite runs in its own process group. When the deadline hits, the binary kills that group, including a grandchild that ignores `SIGTERM`. On Windows the suite runs under Git for Windows' `sh`, found from `git.exe` when `sh` is not on `PATH`, and the deadline uses `taskkill /T`. The default deadline is 120000 milliseconds. A Maven suite needs 600000 or more.

Two limits both use the number 30. They are different limits.

**Mutants in one call.** With no line map, more than 30 mutants waiting stops the run before the first mutant. A baseline under five seconds does not waive that stop. The ids go to `not-started.txt`. With a line map, one call scores every mutant that has no result yet. `--max-mutants`, `--budget-ms`, or `--max-minutes` can stop that call earlier, and `nextCall` continues in the same out directory. `--confirm-count` scores every mutant still waiting only when the number you pass equals that waiting count. The resume argv leaves `--confirm-count` off.

**Names recorded for one mutant.** One mutant records at most 30 test names unless `--max-selected` raises the cap. The result says when the cap applied (`selectionCapped`, `selectedCount`). A line map that chooses tests still feeds every one of its names into the baseline command. The cap applies per mutant, after that.

`--workers` defaults to 1. A higher value runs that many git worktrees at once, or that many copies when the package is not a git checkout. A `cargo` suite shares one `CARGO_TARGET_DIR` per worker under the temp directory, so each mutant after the first is an incremental build. It is removed when the run finishes, and a `CARGO_TARGET_DIR` you set wins.

The package itself is never edited. A mutant is applied in the worktree or copy, so stopping a run, even by killing the process, cannot leave a mutant in your source. The next run on the same out directory removes a leftover worktree. The MCP server stops a running call, and everything under it, when the client cancels it or closes stdin.

A green `baseline.json` is kept when its command template matches the new call. A different template runs the baseline again. A mutant that already has `results/<id>.json` is not run again. The checkout you started in is left as it was.

## Naming tests with a line map

A line map whose top-level value is a `files` object names the tests for a covered line:

```json
{
  "files": {
    "src/A.java": [
      {"start": 3, "end": 5, "tests": ["FooTest", "BarTest"]}
    ]
  }
}
```

`start` and `end` are inclusive, counting from line 1 of the unmutated file.

`{"lines":[1]}` is a permission slip. So is any other non-empty file that is not a `files` object, including a file that is not JSON. A permission slip allows a large batch. It does not choose tests. On the MCP tool, `serial: true` writes that slip at `<out>/lines.json` when you did not pass a map.

An empty `{"files":{}}` does choose tests, and it names none of them. Every mutant in that run is no coverage, and the suite is not started for them.

A line the map lists with an empty `tests` array was executed, and the report named no test. The whole suite runs for that mutant. A line the map does not list is no coverage: duration 0, the suite does not start, and the result has no `command` field.

When the map names at least one mutant, the suite command needs `{tests}` or `{tests,}`. Without a token, the run refuses after the green baseline and before it writes a result. These are the usual shapes:

```bash
mvn -B test -Dtest={tests,}
cargo test {tests}
python3 -m pytest {tests}
node --test {tests}
```

A line map written by the baseline to `<out>/lines.json` is the exception. A pytest map passes those names to the reporter, with no token in the command. A map from Go, Node, Java, or Rust runs the suite command as written when that command has no token, and records the names on the result. A line map you passed still needs the token when it names tests.

After a green baseline, and only when you did not pass `--line-map`, Probatio tries to write that file for these commands. It does not download a tool. If the report is missing, `baseline.json` records `lineMap` as an empty string, the large-batch stop still applies, and `next` names the tool. A failure of that second step leaves the baseline green.

- `go test ./...` runs `go test -coverprofile=<out>/coverage-raw.out ./...`. A span with count 0 is not a hit. A span with count above 0 and no `# test` name runs the whole suite. A profile that already has `# test Name` keeps that name.
- `node --test` runs `node --test --experimental-test-coverage`, with the report under the out directory. If Node rejects the flag, the baseline stays green and the map stays empty.
- `mvn -B test`, or that command with a `{tests}` token: if `pom.xml` contains `jacoco` and `target/site/jacoco/jacoco.xml` exists, a line with `ci` above 0 is a hit. Otherwise the suite did not write a coverage report. The binary does not add a plugin and does not fetch an agent.
- `cargo test`: if `cargo llvm-cov` is on `PATH`, its report is read. If it is not, `next` says so, and the green baseline stands.
- pytest, including a venv path: the `coverage.py` map above. That map still names tests. Unittest does not grow a coverage tool.

A map this binary writes does not stamp one list of test classes onto every line.

A command that already contains a token, and that has no names, and whose map does not choose tests, refuses before the baseline.

## The other commands

Each of these does the work its name says, and the envelope is `ok: false` when that work did not succeed. `schema` with no argument lists the commands and, with a name, returns that command's JSON Schema from inside the binary, so it works outside this checkout. `check-kill` reapplies the mutant and reruns it. `mutate sealed` runs with the hidden test removed, checks the label file for `bug_id`, `issue_id`, and `expected_fail`, and does not restore the hidden test. `verify-change` mutates the lines a commit changed. `golden record` logs plain calls from the tests you name. `golden compare` asks whether that table kills what those tests kill, and it deletes nothing. `ledger build` reverts fix commits into forward patches and defaults `--commit` to HEAD. `ledger check` runs those patches again. A red ledger is not re-recorded away. `gap fix` records the guard. It does not invent the missing test. `status` writes `PROBATIO.md`.

`git`, Maven, Go, Cargo, Node, and Python stay outside this binary.
