# Changelog

Versions on npm, newest first. `0.1.1` was committed and was not published. Git tags are not created here.

The version in git moved `0.1.0` at `922a301`, back to `0.0.5` at `c786540`, then `0.1.0` again at `4e0bd47`. npm never published the in-between `0.1.0` at `922a301`.

## 0.2.0

- Not yet published. The gitHead to publish is the commit that sets this version. GitHub Actions run `37715743205` on `4c5f205` is green: core, pack on Node 22, pack on Node 24, toolchain, and macos.

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
