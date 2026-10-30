# Agent contract

Probatio is the program in this repository. Agents run it. A person who is not going to run commands can stop after the top of `README.md`.

The shipped program is the Rust binary. `cargo build` writes it, and `cargo test` runs its suite. The TypeScript CLI was replaced by that rewrite, and the rewrite is complete. `docs/` and `CHANGELOG.md` describe the TypeScript program. Do not follow them as the current contract. Read a command from `schemas/` or `probatio schema <command>`. A changed `schemaVersion` means a field you read may have moved.

Probatio scores a test suite. It does not replace the suite, write the missing test, fetch a toolchain, or prove a change is safe. A kill is an assertion the suite already had.

## How to run it

Prefer the `probatio` MCP server when it is connected. The tools are `schema`, `mutate_generate`, `mutate_run`, and `mutate_tally`. Argument names are camelCase. The server is `probatio mcp` on stdin. `src/main.rs` handles that before dispatch. A normal command named `mcp` is a refusal, because the server is the process that reads stdin.

This checkout launches it from `.grok/config.toml` through `.grok/probatio-mcp.sh` (`cargo run --quiet --offline -- mcp`). A session that is already open does not see a server that was just added.

If the server is not connected, build once in this checkout and call the binary:

```bash
cargo build --offline
./target/debug/probatio <command>
```

Every command prints one JSON object and exits 0 only when `ok` is true. The fields are `schemaVersion` (2), `ok`, `command`, `summary`, `next`, and `nextCall`. `nextCall` is `{ "argv": [ ... ] }` or null. Run that argv as printed. Paths inside it are not rewritten to `~`. A home path in `summary` or `next` is.

Extra fields do not change `schemaVersion`. On `mutate run`, `suiteCommand` is the template, before `{tests}` is filled in. A refusal omits kill counts. The words "not a kill count" may still appear in `next`.

## A campaign

The package is the directory the user named. It can be this repo or another checkout. Do not edit that checkout's source, tests, or environment to make a suite pass. Do not install a tool the binary refused to fetch.

1. Call `mutate_generate` with `package` and `out`. Use `<package>/.probatio/generate` unless the user named a directory. Pass `src` when they named one directory. Pass `maxMutants` only when they asked for a slice. Generate does not run the suite. The summary states the count, and the bound when the walk stopped early.
2. Tell the user the mutant count and the suite command you will use. If the count is above 100 and they did not ask for the whole repository or a serial campaign, stop and wait.
3. Call `mutate_run` with `package`, `patches` set to the generate directory, and `out` set to `<package>/.probatio/run` unless they named one. Set `timeoutMs` to one full suite run with room to spare. The default is 120000. A Maven suite needs 600000 or more.
4. For 30 mutants or fewer, omit `lineMap` and `serial`. For more than 30, pass a real line map, or set `serial` to true. See "Line map" below. One call scores at most 30 mutants that have no result yet. The same call, with the same `out`, continues. `confirmCount` scores every mutant still waiting only when it equals that count. Workers stay at 1.
5. Read the envelope. `ok` false is a stop. Show `summary` and `next`. A red baseline, a missing report, a timeout, a missing tool, or "No line map" means nothing was mutated. Do not read a kill count from that run.
6. When `ok` is true, call `mutate_tally` on the same `out`. `deletedTests` is 0. `noKillsYet` is not a deletion list. Do not delete a test.

`mutate generate` sets `nextCall` to `mutate run` with `--patches` at the generate directory and `--out` at `<that-directory>-run`. That argv has no suite command, so resolution below supplies one. A campaign that already chose `<package>/.probatio/run` should keep using that directory.

A `nextCall` from `mutate run` continues the same out dir. It keeps `--suite-command` as the template, and it omits `--confirm-count`. A finished run sets `nextCall` to null. Reusing an out dir keeps the result files already there and scores the mutants that do not have one. A new campaign uses a new out dir.

A long `mutate_run` holds the MCP server until that call returns. If the tool call is cut off, run the same call again. A green `baseline.json` is kept when its `command` is the same template. A mutant that already has `results/<id>.json` is not run again.

## Suite command

Resolution order:

1. MCP `suiteCommand`, or CLI `--suite-command`. An empty string is an error.
2. A non-empty string `suiteCommand` in `<package>/.probatio.json`.
3. Discovery at the package root.

Any other key in `.probatio.json` is ignored, including `env`, `testLine`, and `passLine`. Invalid JSON stops the run. The next line says to fix the file. Discovery is not used. A flag or a `.probatio.json` command does not require `package.json` to parse.

Discovery does not launch the tool:

- `go.mod` then `go test ./...`
- else `Cargo.toml` then `cargo test` (this wins when `package.json` is also present)
- else `pom.xml` then `mvn -B test`
- else `package.json`: a non-empty string `scripts.test` is used as written, otherwise `node --test`. No other `scripts` key is read. Invalid `package.json` stops the run the same way.
- else `pytest.ini` or `pyproject.toml` then `python3 -m pytest`
- else stop and ask. Do not invent a command.

`{tests,}` is filled in first, then `{tests}`. A command that only has `{tests,}` does not also count as `{tests}`. `{tests}` single-quotes each selected name. `{tests,}` joins names with commas. A name must be ASCII letters, digits, or `_` `-` `.` `/` `:`. An empty name, or any other character, refuses the run. `::command` is not a test id.

There is no `--only-test` flag. A keep id is a test name the suite can collect. To score names, put `{tests}` or `{tests,}` in the suite command and supply the names from a `files` map or a `--tests` file. Do not invent a flag.

When the package has `node_modules/.bin`, that directory is on the suite process PATH for the baseline and for a mutant worktree or copy. The command string is not rewritten. The worktree (git) and the copy (no git) skip `.git`, `target`, `node_modules`, and `dist`. Each gets a symlink to the package `node_modules` when the destination does not already have one. Removing the worktree or the copy does not delete the package `node_modules`. A shell exit 127 whose stderr contains `command not found` is a missing tool on the baseline (`judgement` `missing-tool`) and outcome `error` on a mutant. An exit 1 whose text happens to contain those words can still be a kill. The binary does not fetch a toolchain.

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

A mutant the choosing map does not name is outcome `no coverage`, duration 0, no `command` field, and the suite is not started.

A choosing map that names at least one mutant in the batch, together with a suite command that has neither token, refuses after a green baseline and before any result is written. Summary: "The line map named tests and the suite command has no {tests} token."

No usable line map, and more than 30 mutants pending: stop before the first mutant. A baseline under five seconds does not waive that stop. The ids go to `not-started.txt`. With a line map, one call still scores at most 30 mutants that have no result yet, unless `confirmCount` equals the waiting count. Resume argv never includes `--confirm-count`. `--workers` above 1 is accepted and ignored. The next line says "Workers stay at 1." Peak work is one worktree or copy, plus one suite process.

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
- `error`: the process failed in a way that is not an assertion. Exit 127 with `command not found` is this.

A timeout, an unviable mutant, and an error are not kills and not gaps. For a ledger fix, write a hand-made mutant that keeps the API and restores the old behaviour.

A result file that already says `flaky` is counted as flaky. The scorer does not assign that outcome itself.

Baseline judgements, and the run stops before any mutant: `green`, `red` ("already failing"), `no-report`, `timed-out`, `missing-tool`. `baseline.json` stores the judgement, `durationMs`, and the template. It does not store stdout or stderr.

## Commands

MCP, in preference: `schema`, `mutate_generate`, `mutate_run`, `mutate_tally`.

CLI. Flags for each one live in `schemas/<name>.schema.json`. Ask with `probatio schema mutate.generate` (dots, not spaces). Do not invent a flag the schema does not list.

- `mutate generate`: write `mutants.json`. Does not run the suite. Needs `--package` and `--out`. Optional `--src`, `--max-mutants`.
- `mutate run`: baseline, then one chunk. Needs `--package`, `--patches`, `--out`. Optional `--suite-command`, `--line-map`, `--timeout-ms` (default 120000), `--tests`, `--max-selected` (default 30), `--confirm-count`.
- `mutate tally`: read `results/*.json`. `deletedTests` is 0. Needs the out dir.
- `mutate sealed`: writes `sealed.json` with the hidden test still hidden and no catch recorded. It does not restore a test. If a sealed run misses, record the miss.
- `ledger build`: records a commit id in `ledger.json`. It does not rewrite the checkout. Needs `--package`, `--out`, `--commit`.
- `ledger check`: a file that is red, or whose JSON status is red, means a fixed bug is back in reach of the suite. Do not re-record it away. Find the test that stopped guarding it, or give the reason in a `Golden-Change:` line.
- `matrix report`: counts from a finished out dir. The kill list is `matrix.json`, not stdout.
- `golden check`, `golden record`, `golden compare`: presence, a note, or a comparison. `golden compare` deletes nothing. A difference needs a `Golden-Change:` line before anyone records again.
- `gap fix`: does not write the missing test. Add a test that fails on that mutant and passes on the unmutated code, then rerun.
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

Do not publish around the release checks. A release needs a reason in the CHANGELOG.

## Rules

- Start from a green suite. If the baseline has any failing test, or it returns no test report, stop. Do not read a kill count from that run.
- Do not delete a test. You may propose a deletion. You must not delete a file from a score.
- Do not treat a kill as proof the change is safe. Golden and contract checks are what say the expected result is still the expected result.
- Refuse an unmapped suite. When there is no line map and more than 30 mutants are pending, stop before the first mutant.
- Do not restore a hidden test.
- Do not patch a project's files or environment to make one repository pass. `.probatio.json` is read for `suiteCommand` only.
- A golden table that kills what a unit test kills is evidence. `golden compare` deletes nothing, and neither do you without a reason in the commit.

A trustworthy loop, after the green baseline:

- Mutate the changed lines, or a bounded batch, with a line map.
- Keep a test that killed a real mutant.
- For each survivor, add one test that fails on that mutant and passes on the unmutated code. Then rerun. A gap with no new test is still a gap.
- Leave no coverage lines alone. There is no evidence either way.
- Keep golden and contract checks.

Trust a green suite, a line map, and a small or medium diff: the caught, missed, and no coverage counts. Trust a refusal. Do not trust a kill count from a red suite, a kill count that includes compile failures, a child process started with a cleared environment, a large batch with no line map, or a tally used as permission to delete tests.
