# Agent contract

Probatio is the program in this repository. Agents run it. A person who is not going to run commands can stop after the top of `README.md`.

The shipped program is the Rust binary. `cargo build` writes it, and `cargo test` runs its suite. The TypeScript CLI was replaced by that rewrite, and the rewrite is complete. Changelog entries through 0.3.0, and the notes in `docs/` other than `docs/releasing.md`, describe the TypeScript program. Do not follow those notes as the current contract. The crate version is 0.8.0, and the MCP server reports it. Read a command from `schemas/` or `probatio schema <command>`. A changed `schemaVersion` means a field you read may have moved.

Probatio scores a test suite. It does not replace the suite, write the missing test, fetch a toolchain, or prove a change is safe. A kill is an assertion the suite already had.

## How to run it

Prefer the `probatio` MCP server when it is connected. The tools are `mutate_diff`, `mutate_generate`, `mutate_run`, `mutate_tally`, `job_status`, and `cancel_job` (group `core`), `schema` (group `diagnostics`), and `probatio`, which runs any CLI command by argv (group `cli`). Argument names are camelCase. An unknown argument, a missing required one, or a wrong type is refused in one line that names the argument and what is expected; fix the call rather than retrying it. The server is `probatio mcp` on stdin. `src/main.rs` handles that before dispatch. A normal command named `mcp` is a refusal, because the server is the process that reads stdin.

This checkout launches it from `.grok/config.toml` through `.grok/probatio-mcp.sh` (`cargo run --quiet --offline -- mcp`). A session that is already open does not see a server that was just added.

If the server is not connected, build once in this checkout and call the binary:

```bash
cargo build --offline
./target/debug/probatio <command>
```

Every command prints one JSON object and exits 0 only when `ok` is true. The fields are `schemaVersion` (2), `ok`, `command`, `summary`, `next`, and `nextCall`. `nextCall` is `{ "argv": [ ... ] }` or null. Run that argv as printed. Paths inside it are not rewritten to `~`. A home path in `summary` or `next` is.

Extra fields do not change `schemaVersion`. On `mutate run`, `suiteCommand` is the template, before `{tests}` is filled in. A refusal omits kill counts. The words "not a kill count" may still appear in `next`.

## After an edit

Call `mutate_diff` with `package` when you have changed code and want to know whether the changed lines are tested. Grep and coverage cannot tell you that: a line can run under a test that never checks it. It diffs the working tree against `since`, mutates only those lines, runs the suite on each mutant, and returns `survivors` with `file`, `line`, `column`, `original`, and `replacement`. Each survivor is a changed line no test checks: add a test that fails on that change and passes on your code, then call `mutate_diff` again. Each call starts from a fresh `<project root>/.probatio/diff/<package path>`, so the new test counts. More than `maxMutants` (default 30) mutants are reported as `notRun`. The refusals below (red baseline, missing tool) apply the same way.

`since`:

- `HEAD` (default): uncommitted edits. Untracked source files count whole.
- One ref, such as `main`: the lines this branch changed, diffed from the merge-base of that ref and HEAD. Lines that only changed on `main` are not scored, even after you merged `main` into the branch. Use this to score a branch.
- `A..B` starts at `A` itself (git's two dots); `A...B` starts at the merge-base of `A` and `B`. `B` must be HEAD (empty means HEAD), because mutants run on the working tree. Uncommitted edits count in every form.

Test code is not mutated: files under `tests/`, `test/`, `benches/`, `__tests__/`, test file names (`*_test.go`, `test_*.py`, `*.test.ts`, `FooTest.java`, Rust `tests.rs`, ...), and Rust items under `#[cfg(test)]`, `#[test]`, or another test attribute. `testLinesSkipped` counts the changed lines left out. Pass `includeTests: true` only when the user wants test code mutated. A mutant that did not build is in `unbuildable`, not in `survivors`: it is not a gap and not a score.

## Long runs are jobs

Every MCP call that runs tests (`mutate_diff`, `mutate_run`, `probatio`) is a job, so it is never cut off by the client's idle timeout. The call waits `waitSeconds` (default 20, at most 600). If the run ends by then, the reply is the result, with `status: "completed"`. If not, the reply has `status: "running"`, a `job_id`, and `progress` (`phase`, `done` of `total`, `killed`, `survived`), and the run goes on. Then call `job_status` with that `job_id` and `waitSeconds: 600`: it returns the result as soon as the run ends, or `running` with progress after ten minutes; call it again. `job_status` with `waitSeconds: 0` reads progress at once. It also answers after the run ended and after the server restarted (`status: "lost"` when the server that ran it stopped first; the results it wrote are kept). `cancel_job` stops a run and every process under it and removes its mutant tree. Do not start the same run twice to see whether the first is still going: ask `job_status`.

## A campaign

The package is the directory the user named. It can be this repo or another checkout. Do not edit that checkout's source, tests, or environment to make a suite pass. Do not install a tool the binary refused to fetch.

1. Call `mutate_generate` with `package` and `out`. Use `<project root>/.probatio/generate` unless the user named a directory. The project root is the top of the git checkout; `.probatio/` ignores itself, so it is never committed. Pass `src` when they named one directory. Pass `maxMutants` only when they asked for a slice. Generate does not run the suite. The summary states the count, and the bound when the walk stopped early.
2. Tell the user the mutant count and the suite command you will use. If the count is above 100 and they did not ask for the whole repository or a serial campaign, stop and wait.
3. Call `mutate_run` with `package`, `patches` set to the generate directory, and `out` set to `<project root>/.probatio/run` unless they named one. Set `timeoutMs` to one full suite run with room to spare. The default is 120000. A Maven suite needs 600000 or more.
4. For 30 mutants or fewer, omit `lineMap` and `serial`. For more than 30, pass a real line map, or set `serial` to true. See "Line map" below. With a line map, one call scores every mutant that has no result yet. Without one, more than 30 mutants stop before the first mutant. `confirmCount`, when you pass it, must equal the waiting count. `workers` (`--workers`) runs that many worktrees at once. A kill is confirmed by rerunning the killing test twice unless you pass `confirm: false` (`--no-confirm`).
5. Read the envelope. `ok` false is a stop. Show `summary` and `next`. A red baseline, a missing report, a timeout, a missing tool, or "No line map" means nothing was mutated. Do not read a kill count from that run.
6. When `ok` is true, call `mutate_tally` on the same `out`. `deletedTests` is 0. `noKillsYet` is not a deletion list. Do not delete a test.

`mutate generate` sets `nextCall` to `mutate run` with `--patches` at `<generate-out>/mutants` and `--out` at `<generate-out>/runs`. That argv has no suite command, so resolution below supplies one. A campaign that already chose `<package>/.probatio/run` should keep using that directory.

A `nextCall` from `mutate run` continues the same out dir. It keeps `--suite-command` as the template, and it omits `--confirm-count`. A finished run sets `nextCall` to null. Reusing an out dir keeps the result files already there and scores the mutants that do not have one. A new campaign uses a new out dir.

A long `mutate_run` is a job (above). Other calls are answered meanwhile, and another agent may call `mutate_tally` on the same `out`: it counts the mutants that have a result and the ones still waiting. If a run was stopped, run the same call again. Closing the server, cancelling a call that is still waiting, or `cancel_job`, stops the suite it started and removes its mutant tree. The package is never edited, so nothing needs restoring. A green `baseline.json` is kept when its `command` is the same template. A mutant that already has `results/<id>.json` is not run again.

## Suite command

Resolution order:

1. MCP `suiteCommand`, or CLI `--suite-command`. An empty string is an error.
2. A non-empty string `suiteCommand` in `<package>/.probatio.json`.
3. Discovery at the package root.

`env` must be an object of strings and is passed to the suite. `copy` must be an array of paths inside the package: files or folders git ignores that the mutant tree needs. `testLine` and `passLine` must be regular expressions. A shell suite names a test from each `testLine` match, and a non-zero exit blames the last of those lines unless `passLine` was printed. A pattern that matches nothing is not a report. Any other key is ignored. Invalid JSON, or a pattern that does not compile, stops the run. The next line says to fix the file. Discovery is not used. A flag or a `.probatio.json` command does not require `package.json` to parse.

Discovery does not launch the tool:

- `go.mod` then `go test ./...`
- else `Cargo.toml` then `cargo test` (this wins when `package.json` is also present)
- else `pom.xml` then `mvn -B test`
- else `package.json`: a non-empty string `scripts.test` is used as written, otherwise `node --test`. No other `scripts` key is read. Invalid `package.json` stops the run the same way.
- else `pytest.ini` or `pyproject.toml` then `python3 -m pytest`
- else `Package.swift` then `swift test`
- else a root `.sln`, `.csproj`, or `.fsproj` then `dotnet test`
- else `Makefile`, `makefile`, or `GNUmakefile` then `make test`
- else `.mocharc.json`, `.mocharc.js`, `.mocharc.cjs`, `.mocharc.yml`, or `.mocharc.yaml` then `mocha`
- else `CMakeLists.txt` then `ctest --output-on-failure`
- else `test_*.py` in the package root, or in a direct child directory named `tests`, then `python3 -m unittest discover`
- else a root `.cob`, `.cbl`, or `.cobol` then `cobc -xj`
- else a root `.c`, `.cc`, `.cpp`, `.h`, or `.hpp` then `make test`
- else a root `.s`, `.S`, or `.asm` then `make test`
- else stop and ask. Do not invent a command.

Pytest is `pytest`, a path whose last component is `pytest` (including `.venv/bin/pytest`), or `python` / `python3` followed by `-m pytest`. Arguments after that program are passed through. `{tests}` and `{tests,}` stay tokens. Unittest is the same Python programs followed by `-m unittest discover`, with an optional `-s` directory. The run loads `test_*.py` by file path from the package root, from `tests/`, and from `test/`, including a folder with no `__init__.py`. `-s` limits that walk. A test that ran and failed is a kill, including `ValueError`. A load failure (`SyntaxError`, `IndentationError`, `ImportError`, or `ModuleNotFoundError`) is unviable. A pytest baseline writes `<out>/lines.json` when `coverage.py` imports, and that map selects tests without a `{tests}` token. If `coverage.py` does not import, the map is not written and the large-batch stop still applies. A caller `--line-map` is not replaced. `make test` stays on the process exit. It is also the command for C, COBOL, and assembly. When a mutant is scored from the process exit, `next` says a crash whose text contains `error:` can be filed as did not build. The result file may include `killedBy`. The envelope does not.

`{tests,}` is filled in first, then `{tests}`. A command that only has `{tests,}` does not also count as `{tests}`. `{tests}` single-quotes each selected name. `{tests,}` joins names with commas. A name must be ASCII letters, digits, or `_` `-` `.` `/` `:`. An empty name, or any other character, refuses the run. `::command` is not a test id.

There is no `--only-test` flag. A keep id is a test name the suite can collect. To score names, put `{tests}` or `{tests,}` in the suite command and supply the names from a `files` map or a `--tests` file. Do not invent a flag.

When the package has `node_modules/.bin`, that directory is on the suite process PATH for the baseline and for a mutant worktree or copy. The command string is not rewritten. The worktree (git) holds your tracked files and the untracked files git does not ignore, never `.probatio`. The copy (no git) skips `.git`, `target`, `node_modules`, `dist`, and `.probatio`. Each gets a symlink to the package `node_modules` when the destination does not already have one. Removing the worktree or the copy does not delete the package `node_modules`. A shell exit 127 whose stderr contains `command not found` or `: not found` is a missing tool on the baseline (`judgement` `missing-tool`) and outcome `error` on a mutant. Bash prints the first phrase. Dash, the usual `/bin/sh` on Ubuntu, prints the second. An exit 1 whose text happens to contain those words can still be a kill. The binary does not fetch a toolchain.

A command with `{tests}` or `{tests,}`, an empty name list, and a line map that does not choose tests, refuses before the baseline.

## Line map

Pass `--line-map` only when the file exists and is non-empty. MCP `lineMap` is that path. MCP `serial: true` with no `lineMap` writes `<out>/lines.json` as `{"lines":[1]}` when that file is empty, then passes it. That file is a permission slip.

A `files` object chooses tests:

```json
{"files":{"src/A.java":[{"start":3,"end":5,"tests":["FooTest","BarTest"]}]}}
```

`start` and `end` are inclusive 1-based lines. The line of a mutant is one plus the number of newline bytes before its start offset in the unmutated package file.

Any other shape only allows the batch. That includes `{"lines":[1]}`, a non-JSON file, and `"files"` as an array. An empty `{"files":{}}` chooses, names nothing, and every mutant is `no coverage`. The empty-name refusal does not fire for that map.

Names for one mutant, in order: the mutant's own test list when it is non-empty, else the map names when the map chooses, else the `--tests` file (one name per line) when the map is absent or is a slip. `--tests` is ignored when the map chooses. The baseline command receives every name from a choosing map, with no cap. Each mutant then records at most `maxSelected` names (default 30). `selectionCapped` and `selectedCount` say when the cap applied.

A line the choosing map does not list is outcome `no coverage`, duration 0, no `command` field, and the suite is not started. A line the map lists with an empty test list was executed and named no test. The whole suite runs. The outcome is killed or survived.

A choosing map that names at least one mutant in the batch, together with a suite command that has neither token, refuses after a green baseline and before any result is written. Summary: "The line map named tests and the suite command has no {tests} token." The map at `<out>/lines.json` written by the baseline is the exception, and that refusal does not apply. A pytest map passes the names to the reporter. A Go, Node, Java, or Rust map runs the command as written when it has no token, and records the names on the result. A line map passed in still needs the token when it names tests.

After a green baseline, and only when the caller passed no line map, the run collects for `go test ./...`, `node --test`, `mvn -B test` (or that command with a `{tests}` token), `cargo test`, and pytest. Go reads a cover profile. Node reads the coverage report from `--experimental-test-coverage`. Java reads `target/site/jacoco/jacoco.xml` only when `pom.xml` contains `jacoco` and that file exists. Rust reads `cargo llvm-cov` only when that program is on `PATH`. Pytest keeps the `coverage.py` map, which still names tests. Unittest does not grow a coverage tool. A missing tool or an empty report records `lineMap` as an empty string, leaves the large-batch stop in place, and names the tool in `next`. That second step does not turn a green baseline red. The binary does not download a tool, and it does not stamp one list of test classes onto every line.

No usable line map, and more than 30 mutants pending: stop before the first mutant. A baseline under five seconds does not waive that stop. The ids go to `not-started.txt`. With a line map, one call scores every mutant that has no result yet. `confirmCount`, when passed, must equal that waiting count or the run stops before a mutant. `--workers` above 1 runs that many worktrees or copies, each with its own suite process. A finished run that still has a survivor sets `nextCall` to `mutate tally`.

Usual commands once a map names tests:

```bash
mvn -B test -Dtest={tests,}
cargo test {tests}
python3 -m pytest {tests}
node --test {tests}
```

## What a result means

Read no coverage first, then survivors. Neither is a pass.

- `killed`: an assertion failed. A kill is not proof the change is safe.
- `survived`: the suite ran the change and did not notice. That is a gap.
- `no coverage`: nothing executed that line. Leave it.
- `unviable`: the mutant did not build or link, including a Node test file that no longer links. The summary says "did not build".
- `timeout`: the baseline clock ran out.
- `error`: the process failed in a way that is not an assertion. Exit 127 with `command not found` or `: not found` is this.

A timeout, an unviable mutant, and an error are not kills and not gaps. A build failure that does not name the mutated file is an `error` (the tree did not build), and when most mutants in a call error, the run refuses with the first error: nothing was scored, which is never a pass. The mutant worktree is the whole git checkout, so a workspace member builds with its workspace; pass the member folder as `package`. It holds your tracked and untracked files, not ignored ones, and lives in Probatio's cache folder (`PROBATIO_CACHE_DIR`), one per worker, reused by every mutant of a run and removed when the run ends. Mutant builds always get their own `CARGO_TARGET_DIR`, kept between runs so a second run builds only what changed, never yours: a `--target-dir` in the suite command is pointed there for mutant runs too. Before the first mutant the suite runs once in each tree with no mutant: that builds the tree, and a test that fails there stops the run with no score ("fails in the mutant tree"). That usually means a test needs a file git ignores; list it in `.probatio.json` as `"copy": ["path"]` (from the package) and make the same call. For a ledger fix, write a hand-made mutant that keeps the API and restores the old behaviour.

A result file that already says `flaky` is counted as flaky. The scorer does not assign that outcome itself.

Baseline judgements, and the run stops before any mutant: `green`, `red` ("already failing"), `no-report`, `timed-out`, `missing-tool`. `baseline.json` stores the judgement, `durationMs`, and the template. A baseline that looked for a coverage map also stores `lineMap`: `lines.json` when a report was written, or an empty string when it was not. The file does not store stdout or stderr.

## Commands

MCP, in preference: `mutate_diff` after an edit; `mutate_generate`, `mutate_run`, `mutate_tally` for a whole package; `job_status` and `cancel_job` for a run that answered with a `job_id`; then `schema` and `probatio`.

CLI. Flags for each one live in `schemas/<name>.schema.json`. Ask with `probatio schema mutate.generate` (dots, not spaces). Do not invent a flag the schema does not list.

- `mutate diff`: mutate the lines changed since `--since` (default `HEAD`), score them, and name the survivors. Needs `--package`. Optional `--since`, `--out`, `--src`, `--suite-command`, `--timeout-ms`, `--max-mutants` (default 30), `--workers`, `--no-confirm`, `--include-tests`.
- `mutate generate`: write `mutants.json`. Does not run the suite. Needs `--package` and `--out`. Optional `--src`, `--max-mutants`, `--include-tests`.
- `mutate run`: baseline, then one chunk. Needs `--package`, `--patches`, `--out`. Optional `--suite-command`, `--line-map`, `--timeout-ms` (default 120000), `--tests`, `--max-selected` (default 30), `--confirm-count`.
- `mutate tally`: read `results/*.json`. `deletedTests` is 0. Needs the out dir.
- `mutate sealed`: writes `sealed.json` with the hidden test still hidden and no catch recorded. It does not restore a test. If a sealed run misses, record the miss.
- `ledger build`: records a commit id in `ledger.json`. It does not rewrite the checkout. Needs `--package`, `--out`, `--commit`.
- `ledger check`: a file that is red, or whose JSON status is red, means a fixed bug is back in reach of the suite. Do not re-record it away. Find the test that stopped guarding it, or give the reason in a `Golden-Change:` line.
- `matrix report`: counts from a finished out dir. The kill list is `matrix.json`, not stdout.
- `golden check`, `golden record`, `golden compare`: presence, a note, or a comparison. `golden compare` deletes nothing. A difference needs a `Golden-Change:` line before anyone records again.
- `gap fix`: records the guard and the commit. It does not invent the missing test. Add a test that fails on that mutant and passes on the unmutated code, then rerun.
- `gap revert`: does not delete a file.
- `guard check`: reads a guard file. It does not edit the project.
- `findings add`: writes a note to `findings.json`.
- `status`: reads an out dir. It does not delete a test.
- `seal`: writes a seal. A hidden test stays hidden.
- `queue seed`, `queue claim`, `queue reap`: a list of ids. Reap does not delete a test.
- `check-kill`: reads `results/<id>.json`. A kill is still an assertion failure.
- `verify-change`: with no `--suite-command`, runs nothing and introduces no mutant. With one, runs that command as a baseline.
- `schema`: lists the commands, or confirms one schema file.
- `help`, `--help`, `-h`: a short pointer. `ok` is true.

Before calling a sealed kill a catch, check that the fix commit did not also edit the killing test. If it did, the kill came from the fix, and it is a miss.

Do not publish around the release checks. A release needs a reason in the CHANGELOG. `docs/releasing.md` is the publish path. Do not push a tag or publish from a campaign.

## Choosing groups

The MCP tools come in groups. `core` (`mutate_diff`, `mutate_generate`, `mutate_run`, `mutate_tally`, `job_status`, `cancel_job`) is always on. Choose the rest for the project you are in, by adding one line to a fenced block whose info string is `agent-tools` in the `AGENTS.md` at the directory the server starts in:

- `diagnostics` (`schema`): a command's flags and output fields. Turn it off once you know the flags; each tool's own schema already covers the MCP calls.
- `cli` (`probatio`): every CLI command by argv, for ledger, golden, matrix, queue, check-kill, verify-change, sealed, gap, guard, findings, status, and seal. Turn it off when the work here is mutation campaigns only.

The line, inside that block: `probatio: disable cli, diagnostics`. `probatio: tools <groups>` is an allowlist instead. The server rereads the file on the next request and tells the client the tool list changed. A name it does not know is ignored and named in the `initialize` instructions. `PROBATIO_TOOLS` and `PROBATIO_DISABLE` in the client's `env` win over the file; a bad name there stops the server.

## Rules

- Start from a green suite. If the baseline has any failing test, or it returns no test report, stop. Do not read a kill count from that run.
- Do not delete a test. You may propose a deletion. You must not delete a file from a score.
- Do not treat a kill as proof the change is safe. Golden and contract checks are what say the expected result is still the expected result.
- Refuse an unmapped suite. When there is no line map and more than 30 mutants are pending, stop before the first mutant.
- Do not restore a hidden test.
- Do not patch a project's files or environment to make one repository pass. `.probatio.json` is read for `suiteCommand`, `env`, `testLine`, and `passLine`.
- A golden table that kills what a unit test kills is evidence. `golden compare` deletes nothing, and neither do you without a reason in the commit.

A trustworthy loop, after the green baseline:

- Mutate the changed lines, or a bounded batch, with a line map.
- Keep a test that killed a real mutant.
- For each survivor, add one test that fails on that mutant and passes on the unmutated code. Then rerun. A gap with no new test is still a gap.
- Leave no coverage lines alone. There is no evidence either way.
- Keep golden and contract checks.

Trust a green suite, a line map, and a small or medium diff: the caught, missed, and no coverage counts. Trust a refusal. Do not trust a kill count from a red suite, a kill count that includes compile failures, a child process started with a cleared environment, a large batch with no line map, or a tally used as permission to delete tests.
