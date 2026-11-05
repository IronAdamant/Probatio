# Migrating from the TypeScript version

This list is the difference between the last TypeScript program and the Rust binary in this tree.

| | |
| --- | --- |
| TypeScript | Commit `c5cd175` (full hash `c5cd1755146082282332a2f6098981917c5bc03a`). npm package `probatio` 0.3.0. Branch `typescript-legacy` points at that commit. |
| Rust | Crate 0.7.0 on `main`. The MCP server reports that version. |

Evidence used: `src/` and `schemas/` at `c5cd175`, the same paths in this tree, pull requests 2, 3, and 5, `CHANGELOG.md`, and the benchmark notes in `README.md`. Pull request 2 is the parity pass. A row there marked fixed is listed here only when this tree still differs from `c5cd175`. Pull request 1 fixed a Rust baseline check for dash (`: not found`). The TypeScript runner already treated that phrase as a missing tool, so it is not a difference.

Each row is marked:

- **intentional change**: the Rust program does this on purpose.
- **not supported**: the TypeScript program did this, and the Rust program does not.

No row is marked **bug fixed from TypeScript**. Fixes recorded in the changelog through 0.3.0 are already in `c5cd175`.

`schemaVersion` stays 2. A new field does not change it. A renamed or removed field would.

## Install and platforms

| Item | Mark | TypeScript 0.3.0 | Rust 0.7.0 |
| --- | --- | --- | --- |
| npm package contents | intentional change | `dist/` (compiled TypeScript) and `schemas/`. Dependency `typescript`. `engines.node` is `>=22.6`. | `bin/probatio.js` and `README.md`. No `typescript` dependency. `engines.node` is `>=18`, and that bound is only for the launcher. |
| What `probatio` runs | intentional change | `node dist/cli.js`. | The launcher spawns the binary from the matching platform package. `cargo build` writes the same binary. |
| `probatio-mcp` bin | not supported | `package.json` `bin` also mapped `probatio-mcp` to `dist/mcp.js`. | The only bin name is `probatio`. The server is `probatio mcp`. |
| Node version gate | not supported | Before any command, Node older than 22.6 printed a JSON envelope and exited 1. The `mcp` schema text describes that envelope. | The binary does not check the Node version. See the leftover schema sentence at the end of this file. |
| Platform packages | intentional change | Any machine that could run Node 22.6 or newer. | npm optional dependencies: `probatio-linux-x64` (musl), `probatio-darwin-arm64`, `probatio-darwin-x64`, `@iron_adamant/probatio-win32-x64`. A `v*` tag builds `x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, and `x86_64-pc-windows-msvc`. |
| Other platforms | not supported | Covered by Node, when Node itself ran there. | The launcher writes a plain text line to stderr and exits 1: `no binary for <platform>-<arch>`. That line is not a JSON envelope. Build from source with Cargo on a platform the launcher does not ship. |
| Windows package name | intentional change | Not a native package. | npm returned 403 for the unscoped name `probatio-win32-x64` (spam detection, pull request 5). The published name is `@iron_adamant/probatio-win32-x64`. |
| Versions between 0.3.0 and 0.5.0 | intentional change | Last published TypeScript version is 0.3.0. Git tags were not created for the TypeScript versions. | `0.4.0` was the crate version in git and was not published. npm `probatio` from 0.5.0 onward is the Rust binary. 0.5.1 adds `server.json` for the MCP Registry as `io.github.IronAdamant/probatio`. 0.6.0 adds MCP tool groups and argument checks, and the Windows fixes in the changelog. 0.7.0 adds `mutate diff` and an opt-in registry publish in the release workflow. |
| Launcher failure | intentional change | A missing Node or a failed `node` start was the process error from Node itself. | A missing platform package, a missing binary file, or a binary that does not start is a stderr line from `npm/bin/probatio.js` and exit 1. It is not the JSON envelope. |

## Commands and flags

The command names are the same 24, and Rust adds a 25th, `mutate diff` (intentional change: mutate the lines changed since a ref and name the survivors). The 24: `mutate generate`, `mutate run`, `mutate sealed`, `mutate tally`, `ledger build`, `ledger check`, `matrix report`, `golden check`, `golden record`, `golden compare`, `gap fix`, `gap revert`, `guard check`, `findings add`, `status`, `seal`, `queue seed`, `queue claim`, `queue reap`, `check-kill`, `verify-change`, `schema`, and the no-command envelope `mutate.help`.

Flag parsing that both programs share: `--key value`, `--key=value`, a bare `--flag` stored as true, `--no-flag` stored as false, and a repeated flag keeping every value. Unknown flags are refused before work, exit 1, summary `Unknown flag --name.` (or the plural). `--human` is known on every command.

| Item | Mark | Detail |
| --- | --- | --- |
| `help`, `--help`, `-h` | intentional change | Rust exits 0. `command` is `help`. `ok` is true. TypeScript parsed `--help` as a flag and did not treat `help` as a command, so those three invocations fell through to the no-command envelope: exit 1, `command` `mutate.help`. |
| No command | same | Exit 1, `command` `mutate.help`, summary names the command groups. |
| `--version`, `-V` | intentional change | Rust prints one envelope, `command` `version`, `ok` true, `summary` the crate version, exit 0. TypeScript had no version flag. |
| `mutate run` and `mutate sealed` flags added | intentional change | `--line-map`, `--timeout-ms`, `--tests`, `--max-selected` (default 30), `--confirm-count`. TypeScript's allowlist did not include them. The other run flags (`--workers`, `--no-confirm`, `--hide`, `--only-test`, `--budget-ms`, `--max-minutes`, `--suite-timeout-ms`, `--test-timeout-ms`, `--history`, `--affected`, `--direction`, `--build`, `--agent`, `--unset`, `--concurrency`, `--timeout-multiple`, `--timeout-floor-ms`, `--repo`, `--commit`, `--tests-dir`, and the rest of that allowlist) are on both. |
| `mutate tally` | intentional change | Rust also accepts `--state` and `--root`. TypeScript accepted `--out` only. |
| `golden record` | intentional change | Rust also accepts `--out`. |
| `golden compare` | intentional change | Rust also accepts `--actual`. |
| `findings add` | intentional change | Rust also accepts `--out` and `--text`. |
| `status`, `seal` | intentional change | Rust also accepts `--out`. |
| `queue seed`, `queue claim`, `queue reap` | intentional change | Rust also accepts `--out` and `--root`. |
| `check-kill` id | intentional change | Both take the id as the word after `check-kill` (`probatio check-kill <id> --package ...`). Rust also accepts `--id`. TypeScript refused `--id` as an unknown flag. |
| `check-kill` environment | intentional change | TypeScript always removed `SOLARI_API_KEY` and `AUSPEX_LIVE` from that inner suite. Rust does not. Pass `--unset` for a name that must not be inherited. |
| `verify-change` | intentional change | Rust also accepts `--suite-command` and `--timeout-ms`. With no `--suite-command`, it runs nothing and introduces no mutant. With one, and no git diff, it runs that command as a baseline. |
| `{tests}` and `{tests,}` | intentional change | Rust fills `{tests,}` first, then `{tests}`. `{tests}` single-quotes each name. `{tests,}` joins with commas. A name must be ASCII letters, digits, or `_` `-` `.` `/` `:`. TypeScript did not have these tokens. |
| Schema files list flags | intentional change | Each Rust schema file has a top-level `flags` array (name and whether it is required). TypeScript schema files had no `flags` key. `probatio schema <command>` prints the embedded schema, including that array. `schemaVersion` is unchanged. |

`probatio schema` with no name lists the commands from data compiled into the binary. It does not read `schemas/` from the working directory. TypeScript read the schema files shipped next to the package.

## JSON output

The six envelope fields are the same: `schemaVersion` (2), `ok`, `command`, `summary`, `next`, `nextCall`. Exit 0 only when `ok` is true, and exit 1 otherwise, for a command the binary itself runs. The launcher failures above are the exception: they are not JSON.

| Item | Mark | Detail |
| --- | --- | --- |
| Success `if`/`then` in the schema | intentional change | At `c5cd175`, a success envelope (`ok` const true) required the command's extra fields (`mutantCount` on generate, `killed` on run, and the matching lists on the other commands). Commit `268b459` changed that `then.required` list to the six envelope fields. The message on that commit says success objects require the short envelope. The extra fields are still in `properties`, and the binary still writes them on success. A validator that required the TypeScript success list will not require it against these schema files. |
| Generate extras | same on success | `mutantCount`, `operators`, `stringLiteralMutants` (0), `filesVisited`, `budgetHit`, `mutants` (first ten), `rest`, `full` (path of `mutants.json`). `nextCall` is `mutate run` with `--patches <out>/mutants` and `--out <out>/runs`, or `runs-2`, `runs-3`, when that directory already holds results. |
| Generate `source` | intentional change | Rust adds `source` (`commit` or `working-tree`) on the generate envelope. It is not a schema property. TypeScript used `source` only to word `next`. |
| Generate summary when `--max-mutants` stops the walk | intentional change | Rust's summary and `next` say the walk was bounded by `--max-mutants`. TypeScript's summary did not add that sentence. `budgetHit` and the time-budget summary are on both. |
| Run counts | same | Flat `killed`, `survived`, `flaky`, `timeouts`, `errors`, `noCoverage`, `unviable`, plus `counts`. A refusal before any mutant omits a kill count. |
| Home paths | intentional change | `summary` and `next` may show `~`. `nextCall.argv` and the `suiteCommand` template keep the real path, because both are run as written. On Windows the home folder is `USERPROFILE` when `HOME` is unset, and a forward-slash spelling of it is scrubbed too. |
| Rust kill names | intentional change | A `cargo test` kill names the tests libtest printed as `FAILED`, and the confirm reruns `-- --exact <name>`. TypeScript named Rust kills only from an LLVM map. |
| `nextCall` paths on Windows | intentional change | A Windows path in `nextCall`, `full`, and `results` is written with `/`. A suite command is not rewritten. |
| Result files and tally | intentional change | Each `results/<id>.json` has the mutant's `file` and `line`, so a tally gap names `file:line`. The tally summary counts kills, and says how many name no test. A node kill name is relative to the package, not the temp worktree. A tally while a run is going counts the mutants still waiting, from `run.json`, which `mutate run` writes before its first mutant. |
| Result files | same | A scored mutant can store `killedBy`, `direction`, `mutantHash`, and `agent`. A history reuse stores `reused: true`, duration 0, and an empty command. Those fields are on the TypeScript result object as well. |

## Errors and exit codes

| Item | Mark | Detail |
| --- | --- | --- |
| Exit 0 and 1 | same | 0 when `ok` is true, 1 otherwise, for the binary. |
| Unknown flag, missing required flag | same | Exit 1. Unknown flags name the flag. A missing required flag names it. |
| Red baseline, no report, timeout, missing tool | same | Stop before the first mutant. No kill count. Missing tool is exit 127 and the text `command not found` (bash) or `: not found` (dash). An exit other than 127 that happens to contain those words can still be a kill. |
| Help and version | intentional change | See the command table. TypeScript help invocations were exit 1. |
| Launcher text | intentional change | See install. Exit 1, stderr, no envelope. |
| Default `--suite-timeout-ms` on `mutate run` and `ledger check` | intentional change | TypeScript defaulted both to 600000. Rust defaults both to 120000. That value is the baseline clock. `golden record` and `golden compare` default to 600000 on both. `check-kill` defaults to 60000 on both. `--test-timeout-ms` defaults to 60000 on both. `--timeout-multiple` defaults to 5 and `--timeout-floor-ms` defaults to 20000 on both. |
| Mutant clock | intentional change | When `--timeout-ms` is omitted, both set the mutant clock to the minimum of the suite timeout and `max(floor, baseline times multiple)`. Rust adds `--timeout-ms`. When it is passed, that value is the baseline clock and the mutant clock, and the formula is not used. TypeScript had no `--timeout-ms`. `--test-timeout-ms` is Node's `--test-timeout`, raised when a test file declares a longer `timeout:`, and capped by the suite clock. |
| Process group | intentional change | On Unix the suite is a process group, and the deadline kills the group. On Windows the deadline uses `taskkill /T` so a grandchild cannot keep the output pipes open. |
| Windows shell | intentional change | A shell suite runs under `sh -c`. On Windows that is the `sh` on `PATH`, else Git for Windows' `bin/sh.exe` found from `git.exe`, else `cmd /C`. `System32\bash.exe` (WSL) is never used. A bare `python3`/`python` skips the Microsoft Store placeholder, and that placeholder's output is a missing tool. |
| A reporter that printed nothing | intentional change | A node or Python run scored by Probatio's reporter that prints no report is `error`, not `killed`: no assertion ran. |
| A baseline that ran no tests | intentional change | `node --test` that finds no test file prints `# tests 0` and exits 0. That is a no-report baseline, not green. |
| Refusal text | intentional change | A red, no-report, or missing-tool baseline ends `next` with the last lines the suite printed. |
| `error:` on a shell-scored mutant | intentional change | When the score comes from the process exit, output containing `error:` is unviable (the summary says did not build), not a kill. TypeScript applied an `error:` check on its make and COBOL paths, not on every shell exit. |

## MCP

`probatio mcp` reads JSON-RPC on stdin and writes one JSON object per line.

| Item | Mark | Detail |
| --- | --- | --- |
| Tool `probatio` | same | `argv` is an array of strings, the CLI words and flags. `isError` is true when `ok` is not true. `structuredContent` is the envelope. |
| Tools `schema`, `mutate_generate`, `mutate_run`, `mutate_tally` | intentional change | Rust advertises these four as well. Argument names are camelCase. `mutate_generate` takes `package`, `out`, and optional `src` and `maxMutants`. `mutate_run` takes `package`, `patches`, `out`, and optional `suiteCommand`, `lineMap`, `serial`, `timeoutMs`, `workers`, `confirm`, `tests`, `maxSelected`, `confirmCount`. `schema` takes an optional `command`. `serial: true` with no `lineMap` writes `<out>/lines.json` as `{"lines":[1]}` when that file is empty. TypeScript advertised only `probatio`, and `tools/call` read `argv` without a tool name. |
| Argument checks | intentional change | An unknown argument, a missing required one, or a wrong type is one `isError` line naming the tool, the argument, and what is expected. Each `inputSchema` sets `additionalProperties: false`. Integer strings and `"true"`/`"false"` are accepted. TypeScript passed `argv` through unchecked. |
| Reply text | intentional change | The text block is the envelope as compact JSON; `structuredContent` carries the same object. `initialize` has short `instructions` and no copy of the tool list, and advertises `tools.listChanged: true`. |
| Tool groups | intentional change | `core` (the three `mutate_*` tools, always on), `diagnostics` (`schema`), `cli` (`probatio`). `PROBATIO_TOOLS` and `PROBATIO_DISABLE` shape the list; an unknown name or `core` exits 2 with one stderr line. A `probatio:` line in an `agent-tools` block of `AGENTS.md` in the server's directory does the same, below the env vars, and a change sends `notifications/tools/list_changed`. TypeScript had one tool. |
| Client leaves | intentional change | Closing stdin, or cancelling a call, stops every process under that call, including the suite's own process group. Killing only the server does too: a kill-on-close job object on Windows, and on Unix each call's child watches for its parent to go. TypeScript's server exited and a running suite kept going. |
| `mutate_diff` | intentional change | First in the `core` group: the lines changed since `since`, scored in one call, with `survivors` returned. TypeScript had no MCP equivalent. |
| Protocol versions | same | `2024-11-05`, `2025-03-26`, `2025-06-18`. Anything else is answered as `2024-11-05`. |
| `ping`, cancel, bad JSON | same | `ping` is answered while a call runs. `notifications/cancelled` kills that call's process group and drops the reply. A line that is not JSON is skipped. An unknown method is `method not found: ...`. |
| Server version | intentional change | `serverInfo.version` is the crate version (`CARGO_PKG_VERSION`). TypeScript read `version` from `package.json` (0.3.0 at that commit). |
| Node-too-old envelope | not supported | TypeScript printed the `mcp` envelope and exited 1 when Node was older than 22.6. Rust does not. |

## Config and files

| Item | Mark | Detail |
| --- | --- | --- |
| `.probatio.json` | same | `suiteCommand` (non-empty string) wins over discovery. `env` must be an object of strings and is passed to the suite. `testLine` and `passLine` must be regular expressions and apply to a shell suite. Any other key is ignored. Invalid JSON, or a pattern that does not compile, stops the run. Discovery is not used. |
| Dropped environment | intentional change | Both drop `NODE_TEST_*` and a `child-lines.mjs` entry in `NODE_OPTIONS`. Rust also always drops `PROBATIO_COVERAGE_MAP`, `PROBATIO_PACKAGE_ROOT`, and `PROBATIO_CHILD_DIR`. `--unset` removes further keys. |
| `mutants.json` and `mutants/<id>.patch` | same | Under the generate out directory. The id is `m` plus the first 12 hex digits of SHA-256 over `file`, start, end, and operator, separated by NUL. Seed default is 20261003. Default `--src` is `src`. Default source is the commit (`HEAD` unless `--commit`), unless `--working-tree` or the tree is not a git checkout. |
| `baseline.json`, `results/<id>.json`, `run.json`, `history.json` | same names | A green `baseline.json` is kept when its command template matches. A mutant that already has a result file is not run again. |
| `lines.json`, `not-started.txt` | intentional change | Rust writes `lines.json` from Go, Node, JaCoCo, `cargo llvm-cov`, or pytest `coverage.py` when that report exists and you did not pass `--line-map`. It does not download a tool. `not-started.txt` receives the ids when the unmapped large-batch stop fires. |
| `node_modules` | intentional change | Both symlink `node_modules` into a worktree and do not rewrite the command string. TypeScript symlinked each `node_modules` it found under the package, including a nested one, and also looked beside the main checkout when the run started in a worktree. Rust links the package-root `node_modules` when the worktree or copy does not already have one, and puts `node_modules/.bin` on the suite `PATH`. Removing the worktree or the copy does not delete the package `node_modules`. |
| Non-git package | intentional change | Rust scores a package that is not a git checkout by copying it. TypeScript's run path was built around git worktrees. |

## Discovery and scoring

TypeScript `discoverSuite` walked the tree and required test files before it named most commands. The order started with `run_tests.sh`, then Cargo (only with a Rust test), Go (only with a `_test.go`, and a `go.mod` at or above the package), Swift, Maven (only with a Java test), dotnet, pytest, unittest, Node test files, Mocha, and a Makefile `test` target that linked COBOL tests.

Rust names a command from a marker at the package root and does not launch the tool. Order: `go.mod`, `Cargo.toml` (this wins over `package.json`), `pom.xml`, `package.json` (`scripts.test` when that string is non-empty, otherwise `node --test`), `pytest.ini` or `pyproject.toml`, `Package.swift`, a root `.sln`, `.csproj`, or `.fsproj`, `Makefile` or `makefile` or `GNUmakefile`, a Mocha rc file, `CMakeLists.txt`, `test_*.py` in the root or in a direct child `tests/`, a root COBOL file, a root C or C++ file, a root assembly file.

| Item | Mark | Detail |
| --- | --- | --- |
| `run_tests.sh` | not supported | TypeScript ran `bash run_tests.sh` when that file was at the package root. Rust does not look for it. Pass `--suite-command`. |
| Parent `go.mod` | not supported | TypeScript accepted a `go.mod` above the package and skipped a nested module. Rust only sees `go.mod` in the package root. The discovered command is `go test ./...`, not `go test -json ./...`. The coverage step is `go test -coverprofile=<out>/coverage-raw.out ./...`, and only for that discovery string. |
| `package.json` `scripts.test` | intentional change | Rust uses that string as the suite command. TypeScript discovery did not read `scripts.test`. |
| `ctest`, `cobc -xj`, `make test` for C and assembly | intentional change | Rust discovery names `ctest --output-on-failure`, `cobc -xj`, and `make test` for a root C, C++, or assembly file. TypeScript did not name those from a marker alone. |
| Pytest and unittest commands | intentional change | Rust discovery is `python3 -m pytest` and `python3 -m unittest discover`. A reporter shipped in the binary scores them. TypeScript's discovered command strings were `python3 pytest` and `python3 unittest`, with reporters shipped as files under `dist/mutate/`. |
| `dotnet run` | not supported | TypeScript could name `dotnet run` for an Exe project under a test directory. Rust names `dotnet test` from a root `.sln`, `.csproj`, or `.fsproj`. |
| Marker without test files | intentional change | Rust names the command from the marker even when no test file is present. TypeScript returned no suite until it found tests. |
| Makefile that would `curl` or `wget` a missing file | not supported | TypeScript refused that `make test` before the suite ran. Rust has no such refusal. |
| More than 30 mutants and no line map | intentional change | Rust stops before the first mutant. A baseline under five seconds does not waive the stop. The ids go to `not-started.txt`. TypeScript waived that stop when the baseline was under 5000 ms, and also skipped it for suite kinds it treated as mapped (`node`, `pytest`, `c`, `go`, `maven`, `cargo`, `dotnet`, `mocha`) before a map file existed. |
| One call with a line map | same outcome, different gate | Both score the mutants that have no result yet in that call, until a budget stops the call. Rust's budget flags are `--budget-ms`, or `--max-minutes` when `--budget-ms` is omitted, and `--max-mutants`. `--confirm-count` must equal the waiting count or the run stops before a mutant. TypeScript did not have `--confirm-count`. |
| `--workers` | intentional change | Default 1 on both. A higher value runs that many worktrees, or copies when there is no git checkout. TypeScript refused a value below 1. Rust uses 1 for a value below 1. A kill is rerun twice unless `--no-confirm`. A rerun that passes is `flaky`. A rerun that errors is not treated as a pass. |
| Strings and comments | same on the success path | The scanner does not emit a mutant inside a string or a comment. The generate summary says `No string-literal mutant.` and `stringLiteralMutants` is 0. |
| Generate refusal for a string or comment hit | not supported | If the TypeScript scanner reported a point inside a string or a comment, generate exited 1 and did not write the batch. `stringLiteralMutants` was the violation count. Rust does not have that refusal. |

## Operators

Core operators stay the default, so earlier ids do not move for a core run. Wide operators (`--operators wide`) are the arithmetic, integer-constant, and dropped-statement set from 0.3.0. String concatenation is left alone. `.ts`, `.tsx`, and `.jsx` also get `negate-condition`, `negate-ternary`, and `drop-not` from the standard-library scan. `.d.ts` is not scanned as TypeScript.

| Item | Mark | Detail |
| --- | --- | --- |
| `.jsx` | intentional change | Rust scans `.jsx` with the text operators and those three AST operators. TypeScript's text scanner did not list `.jsx`. |
| `.mjs`, `.cjs`, `.cxx`, `.hpp`, `.hh`, `.hxx` | same | Both scanners accept them. |

## Benchmarks

Five runs of each command on one machine are in the `README.md` benchmark table and in `CHANGELOG.md` under 0.5.0. TypeScript was `c5cd175` under Node 22.23.4. Rust was `target/release/probatio` from this checkout. Linux 6.12, x86_64, 4 cores, Intel Xeon, 16 GB. Kill counts matched on every scored run. `--no-confirm` was on for the run rows. Peak RSS and the raw envelopes are in `scripts/benchmark_results.md`. That table is a measurement of those commands on that machine. It is not a claim about any other suite.

## Schema text that was not updated

`schemas/mcp.schema.json` still says the envelope is printed when Node is too old to run the server. The Rust binary does not print that envelope. `probatio mcp` starts the stdin server. This file records that mismatch. The schema text was not changed in the same edit.
