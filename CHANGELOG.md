# Changelog

The Rust binary is the program. `README.md` and `AGENTS.md` are the contract. Entries from 0.3.0 down describe the TypeScript program.

Versions on npm, newest first. `0.4.0` was the crate version in git and was not published. `0.1.1` was committed and was not published. Git tags were not created for the TypeScript versions. A `v*` tag publishes the version named in that commit.

The version in git moved `0.1.0` at `922a301`, back to `0.0.5` at `c786540`, then `0.1.0` again at `4e0bd47`. npm never published the in-between `0.1.0` at `922a301`.

## 0.7.0 - 2026-11-05

`mutate diff` answers the question an agent has right after an edit, in one call: are the lines I changed tested? `schemaVersion` stays 2.

Added:

- `mutate diff` (MCP `mutate_diff`, first in the `core` group): diffs the working tree against `--since` (default `HEAD`, so uncommitted edits; untracked source files count whole), mutates only the changed lines, runs the suite on each, and returns `survivors` with `file`, `line`, `column`, `original`, and `replacement`. Each call clears `<package>/.probatio/diff`, so a test added since the last call is scored. Measured through MCP on Windows: a one-line edit to a JS sample, 3 mutants, 1.5 s; a one-line edit to the Relatum crate, 4 mutants, 38 s with confirm, 36 s without. The Relatum survivor was the `==` the edit added, at column 50.
- Rust kills are named from libtest's `test <name> ... FAILED` lines, with no `cargo llvm-cov`. The confirm reruns `-- --exact <name>` instead of the whole suite twice. A per-line test map for Rust still needs llvm-cov: libtest reports no coverage.
- `.github/workflows/mcp-registry.yml` publishes the tag's `server.json` to the MCP Registry with GitHub Actions OIDC after npm, and can be run by hand for a tag already on npm. The registry had no Probatio listing: nothing had published `server.json`.

Fixed:

- Killing only the MCP server process left its call's suite running. On Windows the server joins a kill-on-close job object (std-only FFI to kernel32), so the job ends every process under it. On Unix each call's child checks for its parent every 200 ms and, when the server is gone, kills the suite groups it started and exits.
- Windows: a worktree or copy got no `node_modules`, so a Node suite there could not resolve its dependencies. It is a directory symlink, or a junction, and every such link is removed as a link before its tree is deleted.
- Windows: `nextCall`, `full`, and `results` paths mixed `/` and `\`. Windows paths there use `/`; a suite command is not rewritten.
- Every clippy warning, including the old ones; CI now runs clippy with `-D warnings`.

## 0.6.0 - 2026-11-05

Windows now works end to end, the MCP surface checks its arguments and comes in groups, and Cargo campaigns are faster. Verified on Windows 11 through the MCP server against `examples/pytest`, a JavaScript sample, and a copy of a std-only Rust crate (Relatum). `schemaVersion` stays 2: fields were added, none renamed or removed.

Added:

- MCP tool groups. `core` (`mutate_generate`, `mutate_run`, `mutate_tally`) is always on, `diagnostics` is `schema`, `cli` is `probatio`. `PROBATIO_TOOLS` and `PROBATIO_DISABLE` shape the list; an unknown name, or `core`, exits 2 with one stderr line. A `probatio: tools ...` or `probatio: disable ...` line in an `agent-tools` block of `AGENTS.md` in the server's directory does the same per key, below the env vars. A bad line there is a warning in the `initialize` instructions. The file is restatted on each request, and a changed tool set sends `notifications/tools/list_changed`. `tools/list` is 3553 bytes with every group on and 2815 with only `core`.
- MCP argument checks: an unknown argument, a missing required one, or a wrong type is one `isError` line naming the tool, the argument, and what is expected. `mutate_run` gains `workers` and `confirm`; `schema` gains `command`.
- `mutate tally` during a run: it counts the mutants that have a result and the ones still waiting, so a second agent can read progress cold.
- `initialize` carries short instructions and the active groups.

Changed:

- MCP text replies are compact JSON, about a third smaller. `initialize` no longer repeats the tool list.
- `mutate generate` reads the commit through one `git cat-file --batch` process and stops walking at `--max-mutants`. On a copy of Relatum on Windows, through MCP: 2786 ms to 35 ms.
- A `cargo` suite shares one `CARGO_TARGET_DIR` per worker for the run, so mutants after the first are incremental builds. Relatum, 4 mutants with confirm on: 113 s to 105 s, 13-20 s per mutant (was 16-28 s). A `CARGO_TARGET_DIR` you set wins.
- Worktrees and copies skip `.probatio`.
- A refused baseline ends `next` with the last lines the suite printed.
- `suiteCommand` on the envelope is no longer home-scrubbed; it is run as written.
- `scripts/benchmark.py` is now `scripts/benchmark.rs`, run with `cargo run --release --example benchmark`. The rows and report format are the same.

Fixed:

- Windows: shell suites ran only when Git Bash's `sh` was already on `PATH`. Git for Windows' `sh.exe` is now found from `git.exe`; WSL's `System32\bash.exe` is never used.
- Windows: `PATH` and `PYTHONPATH` were joined with `:`, which splits `C:\`, so `node_modules/.bin` and `src/` were lost.
- Windows: a bare `python3` ran the Microsoft Store placeholder, and the baseline read "no test report". A real interpreter on `PATH` is used, and the placeholder is reported as a missing tool.
- Windows: with `core.autocrlf` (Git's default there), mutant offsets read from the commit did not match the `\r\n` checkout. They are mapped.
- Windows: `node --test` refused the reporter path (`ERR_UNSUPPORTED_ESM_URL_SCHEME`), and each refusal counted as a kill: every JS mutant read killed. The reporter is a `file:` URL, and a reporter run that prints no report is an error, not a kill.
- Windows: `scripts/validate-server-json.js` and the npm pack smoke could not start `npm`/`npx` (`.cmd` shims). `scripts/npm-pack-local.sh` packs the Windows package too.
- `NODE_OPTIONS` scrubbing left a dangling `--import` behind the removed coverage hook.
- A `node --test` baseline that ran no tests (`# tests 0`) was green.
- A tally gap read `at :0`: result files did not carry the mutant's file and line.
- A node kill was named by its path in the temp worktree.
- Closing the MCP server, or cancelling a call, left the suite running; on Unix a cancel did not reach the suite's own process group.

## 0.5.1

`server.json` is the MCP Registry listing. The name is `io.github.IronAdamant/probatio`, and the npm package records that same name in `mcpName`. A client starts the server over stdin with `npx -y probatio mcp`. The crate version, the four platform packages, and the version the MCP server reports are 0.5.1.

## 0.5.0

The npm package installs the Rust binary. `npm install probatio` runs a Node launcher, which starts the binary in the matching platform package: `probatio-linux-x64`, `probatio-darwin-arm64`, `probatio-darwin-x64`, or `@iron_adamant/probatio-win32-x64`. A `v*` tag builds the GitHub release assets and publishes those packages. The MCP server reports this crate version.

Five runs of each command on one machine, median milliseconds with the min and max in parentheses. TypeScript is commit `c5cd175` (`node /tmp/probatio-ts/dist/cli.js`, Node 22.23.4). Rust is `target/release/probatio` from this checkout. The machine was Linux 6.12, x86_64, 4 cores, Intel Xeon, 16 GB.

Kill counts matched on every scored run. `--no-confirm` is on for the run rows so the clock is the suite, not the confirmation rerun. The JS run was measured again after that command gained `--test-concurrency` and `--test-timeout`; the count stayed 2 killed and 4 survived. itsdangerous is Pallets itsdangerous at the depth-1 checkout used here: 10 mutants in 8 files, so `--max-mutants 20` did not drop any.

| Command | TypeScript | Rust |
| --- | --- | --- |
| startup, `schema` | 287 (287–297) | 11 (11–11) |
| generate, JS sample, 6 mutants | 307 (307–308) | 11 (11–11) |
| generate, Python sample, 6 mutants | 307 (297–308) | 11 (11–11) |
| run, JS sample, 2 killed / 4 survived | 832 (823–853) | 743 (722–762) |
| run, Python sample, 2 killed / 4 survived | 1429 (1418–1450) | 1130 (1090–1171) |
| run, Python sample, `--workers 4`, same 2 killed | 805 (796–851) | 491 (471–495) |
| generate, itsdangerous `src`, 10 mutants | 369 (360–371) | 21 (21–21) |
| run, itsdangerous, 7 killed / 3 survived | 7568 (7528–7625) | 7003 (6995–7049) |

## 0.3.0

- Published 2026-10-08. gitHead `f969b984b9aeb35dcec383b48db36e11ac65d680`, tagged `v0.3.0`. The first version cut through `docs/releasing.md`: `test` (run `37742850434`) and `self-score` (run `37742850444`, 3 of 3 ledger bugs caught) were green on that commit before the tag.
- The first `npm publish` was refused. Pushing the tag started a third `test` run on the same commit, and its `core` job hung until its 20-minute limit. The check read that newest run. The failed job was rerun green, and the publish went through. Since then the check reads the run from the push to `main`, and every CLI call in the tests has a 300-second limit, so a hang fails one named test instead of the job.
- `npx probatio@0.3.0` from an empty folder: `schema` lists 24 commands, and generate (`--operators wide`), run, and tally work on `examples/node-ts`.

Read before upgrading an agent:

- `schemaVersion` is 2. 0.2.0 renamed `drop` to `noKillsYet` and moved compile and link failures from `killed` to `unviable` without bumping it. Every envelope now says 2.
- `schemas/` has one JSON Schema per command, shipped in the package, and `probatio schema <command>` prints one. A new field does not change `schemaVersion`. A renamed or removed one does.

Added:

- `ledger check`: runs every ledger bug against the suite and compares each outcome with `ledger/<id>.golden.json`. A caught bug that is no longer caught fails it. Re-recording a changed outcome needs `Golden-Change: <id>: <why>`.
- `.github/workflows/self-score.yml`: Probatio runs `ledger check` on its own ledger on every push to `main` and nightly.
- A hand-made ledger mutant whose header names its fix (`source=hand fix=<commit>`) replaces that fix's history revert. Probatio's own `950dea0` revert did not build, and its hand-made mutant keeps `surefireArg`.
- `golden record`: records the plain-data calls a module's unit tests make into a golden table and a self-contained replay test. `golden compare`: scores the same mutants with only the unit tests and with only the replay. Nothing is deleted.
- `--operators wide` on `generate`, `verify-change`, and `golden compare`: arithmetic, integer constants, and a dropped statement (TypeScript calls; in Java, C, C++, C#, JavaScript, and Rust, a call, assignment, or increment alone on its line). String concatenation is left alone. `core` stays the default, so earlier ids and counts do not move.
- `mutate sealed --fix <commit>`: the fix commit whose diff the fix-edited check reads, when later commits sit on top of it.
- `docs/demo-commons-csv.md`: a blind run of gaps, new tests, and a hidden bug on Commons CSV. The result is negative and says why.
- `scripts/release-check.mjs` runs as `prepublishOnly`. `npm publish` needs a clean `main`, a CHANGELOG entry, a pushed `v<version>` tag on HEAD, and green `test` and `self-score` runs on that commit. See `docs/releasing.md`.

Fixed:

- A kill in a nested test folder (`tests/unit/x.test.ts`) was confirmed against `tests/x.test.ts` on macOS, because `/var` and `/private/var` did not compare equal, so it read as flaky. Confirm now compares real paths.

## 0.2.0

- Published 2026-10-08. gitHead `405d15bb2ab1421cebc2295c828916f3c48d4f94`.
- GitHub Actions is green on `4c5f205` (run `37715743205`) and on `405d15b` (run `37717510566`): core, pack on Node 22, pack on Node 24, toolchain, and macos.
- `npx probatio@0.2.0` from an empty folder ran generate, run, and tally on `examples/node-ts`: 1 killed, the keep id listed once, and `nextCall` writing to `<out>.rescore`.

Changed output (schema 1, read before upgrading an agent):

- `mutate tally`: `drop` is renamed `noKillsYet`, and `pruning.advice` is always empty. One batch is not evidence for a deletion.
- `mutate tally`: `nextCall` writes to a fresh out dir (`<out>.rescore` or `<out>.after-gaps`). Before, it reused the old out dir, skipped every finished mutant, and repeated the old result.
- `mutate tally`: a Node test is kept once, not once per spelling.
- A mutant the compiler rejects is `unviable`, not `killed`. `mutate run` adds `unviable` and `unviableIds`, and the summary says `N did not build` when N is not 0. `verify-change` lists it under `other`, not `caught`. `check-kill` closes it with `cause: "build"` and no test asked for. Old result files that stored a compile failure as `killed` with cause `build` are read as `unviable`.

Fixed:

- Node: covered mutants ran every test in the file, because `run()` with `isolation: "none"` ignores `testNamePatterns` on Node 22. Only the requested tests, and their subtests, can kill now. A name that matches no test is an error, not a survivor. `--only-test` accepts `tests/gate.test.ts::title`.
- Node: a test file that failed before any test ran was dropped on the batch path, so the mutant read as survived. Both paths now split it. A file that does not link or parse (removed export, missing module) ran no code, so the mutant is `unviable`, the same verdict a compiled language gives. A file whose top-level code throws is a kill by that file, and confirm reruns the whole file.
- Node batch: a TypeScript source that imports a sibling as `./x.js` (the NodeNext convention) did not load, so every test file that reached it was dropped. The batch loader now resolves `./x.js` to `./x.ts` the way tsx does. Probatio's own sources are written that way.
- A kill id compares real paths, so a macOS `/var` and `/private/var` pair no longer falls back to the basename.
- MCP: a tool call no longer blocks the server. `ping` and other calls are answered while it runs. `notifications/cancelled` stops the command and its suite and removes its worktrees. `protocolVersion` follows the client when it is supported.
- A Node child process started by a test is mapped under that test with no import in the child, and its lines are written even after `process.exit`. Each test process has its own child dump dir.
- An inner Probatio run no longer inherits an outer run's coverage env.
- Discovery leaves out a nested Go module or Python project. Go needs a `go.mod` at or above the package. Probatio's own suite was discovered as Go because of `examples/go`.
- `ledger build` does not call a version-bump commit a fix unless it has `Fixes-bug:`.
- A run removes its worktrees on SIGTERM and SIGINT.

Removed:

- The tini-only Docker shim that rewrote `ci/install_deps.sh` pip pins, the tini-only `ARCH_NATIVE=1`, the tini-only shell test-line format, and the Swift overlay that renamed Foundation's `Predicate`. A project that needs an env flag or a test-line format puts it in `.probatio.json`. A project that does not build on this toolchain is a refusal.

Added:

- `.probatio.json`: `env`, `testLine`, `passLine`. A shell suite with no `testLine` is read as TAP, or as one command.

## 0.1.3

- Published 2026-10-06. gitHead `760c672914e069d10b6a5a71575a5ad1209f3075`.
- Actions run `37440425701` on `950dea0` is green: core, pack on Node 22, pack on Node 24, toolchain, and macos.
- Surefire reads the method name when `classname` comes first, reruns a parameterized method as `method*`, and joins methods of one class with `+`.

## 0.1.2

- Published 2026-10-06. gitHead `29301bc3839d3012fb2009222cef768bccd27a6b`.
- Four runner limits, a tracked ledger, and goldens for two history patches that are honest `no coverage`.

## 0.1.1

- Not on npm. `ea65193` set this version in git. The next published version is `0.1.2`.

## 0.1.0

- Published 2026-10-06. gitHead `4e0bd4736fbdaed6895e61fc7d8bc955f6bb6e4e`.
- First version an agent can install and believe. The packed README in that tarball still says the npm copy is `0.0.5`.

## 0.0.5

- Published 2026-10-05. gitHead `595793e04e1e4281c4ce4bac656bc0813b10b255`.
- Fit for an agent to try on a green suite and a small diff.

## 0.0.4

- Published 2026-10-05. gitHead `96e4a072738f123d003d00a57706895d26c231ba`.

## 0.0.3

- Published 2026-10-05. gitHead `e801fac405635ddbb56e35049fe50c65f7d2fa4d`.

## 0.0.2

- Published 2026-10-05. gitHead `e776699ffb90f146e54c0394e58f96b2e929e94b`.

## 0.0.1

- Published 2026-10-05. gitHead `4617a7e7cf3217432a54e7223ad3171ce1f83ffc`.

## 0.0.0-stage

- Published 2026-10-05. npm recorded no gitHead.
