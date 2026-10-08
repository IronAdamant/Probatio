# Changelog

Versions on npm, newest first. `0.1.1` was committed and was not published. Git tags are not created here.

The version in git moved `0.1.0` at `922a301`, back to `0.0.5` at `c786540`, then `0.1.0` again at `4e0bd47`. npm never published the in-between `0.1.0` at `922a301`.

## 0.3.0

- Not yet published. The gitHead to publish is the commit that sets this version, tagged `v0.3.0`. It is the first version cut through `docs/releasing.md`.

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
