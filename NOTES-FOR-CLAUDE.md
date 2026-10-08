# Notes for Claude

Newest first. Start a review by rerunning the commands in the latest section. The build log from 2026-10-04 to 2026-10-06 is in [docs/history/notes-2026-10-04-to-06.md](docs/history/notes-2026-10-04-to-06.md). Home and temp paths are written as `~` and `$TMPDIR`. This repository is public.

## Current state

- npm latest is `0.2.0`, published 2026-10-08 by the maintainer at gitHead `405d15b`. `CHANGELOG.md` lists the changes, including the output fields that changed. Git tags are still not created.
- `npm test` on this Mac, 2026-10-08: 146 tests, 143 pass, 0 fail, 3 skipped, about 142s. Skips name the missing tool (`llvm-cov`, `arch -x86_64`). GitHub Actions run `37715743205` on `4c5f205` (these changes) is green on all five jobs.
- Sealed record: no hidden real bug has been caught by older tests. Seven subjects are in the README table. The CSV-271 kill came from a test the fix commit edited, and with the line map it is now a survivor.

## 2026-10-08 — Claude review, then fixes

Each fix below has a builder test that failed before the change and passes after, except where noted.

Bugs found by running the CLI, not by reading:

- Node `--only-test` was ignored on the batch path. `run({ isolation: "none" })` ignores `testNamePatterns` on Node 22.23 (checked with a five-line script). `node-batch.mjs` now keeps only results from the requested tests and their subtests. A name that matches nothing is an error. Test: `tests/node-keep.test.ts`.
- `mutate tally` `nextCall` reused the old `--out`, so it skipped every mutant and repeated the old result. `--only-test "no such test at all"` into that dir said `1 killed`. Now `<out>.rescore` or `<out>.after-gaps`. Tests: `tests/node-keep.test.ts` runs the printed `nextCall` verbatim, and `tests/tally-resume.test.ts`.
- A Node keep id was listed twice (`gate.test.ts::title` and `title`). Tally dedupes by `namesMatch`. `--only-test` accepts `file::title` for node.
- MCP `tools/call` used `spawnSync`. A ping sent at 0.5s was answered at 17.2s. Now async, with `notifications/cancelled` killing the process group and the runner removing its worktrees on SIGTERM. Test: `tests/mcp.test.ts`, third case. A manual probe showed the worktree list back to one entry after a cancel.
- The Node batch path dropped a test file that failed to load (`node:test` names that failure after the file, and the batch skipped file names). The mutant read as survived, which is how ledger patch `950dea0` first survived the self-score: it removes `surefireArg`, and `tests/surefire-attr.test.ts` imports it. Now it is a failure of the file, marked `fileLoad`, and confirm reruns the whole file. The kill id compares real paths, so `/var` and `/private/var` agree. Test: `tests/node-keep.test.ts`, second case, with confirm on and off.
- The batch loader did not resolve `./x.js` to `./x.ts`. Every Probatio test file that imports `src/` failed to load on the batch path, while the tsx baseline was green. With the load-failure fix above, those files read as failures, and confirm called the mutant flaky. That was a scratch-only commit in the second self-score. The loader now tries the TypeScript sibling. Test: `tests/node-keep.test.ts`, third case. It links tsx into the fixture, because such a project runs its tests with tsx.
- Discovery picked Go for Probatio itself because of `examples/go/gate_test.go`. Go now needs a `go.mod` at or above the package and skips nested modules. Python skips nested projects with their own `pytest.ini`, `pyproject.toml`, `setup.py`, `setup.cfg`, or `tox.ini`. Node, Maven, Cargo, and dotnet still walk into nested dirs on purpose (workspaces and multi-module builds). Test: `tests/ordinary-layout.test.ts`.

Honesty of the counts:

- A mutant the compiler rejects is `unviable`, not `killed`. The diary's per-language proofs included compile kills: tini (`clang`), mitt (`tsc`, both kills), and Java (`javac`). Tests: `kill-label`, `java-run`, `make-cobol`, `script-compile`, updated to expect `unviable`.
- `drop` is `noKillsYet`, and the tally gives no prune advice. One batch is the evidence that overfit on Auspex (10 of 18 sealed bugs).
- `ledger build` skips a commit that bumps a manifest version unless it has `Fixes-bug:`. `922a301` was a 1,243-line release bundle called a fix. Test: `tests/ledger.test.ts`.

Repo-specific code removed:

- tini: the Docker shim that rewrote `ci/install_deps.sh` pip pins, `ARCH_NATIVE=1`, and the shell test-line regex (`Testing …`, `Running reaping…`, `All done, tests as expected`).
- Publish (Swift): the overlay that renamed Foundation's `Predicate` in the SDK interface.
- Replacement: `.probatio.json` with `env`, `testLine`, and `passLine`. A shell suite without `testLine` is read as TAP, or as one `::command`. Test: `tests/script-compile.test.ts`, third case.

Child processes:

- A node child of a mapped test loads `child-lines.mjs` through `NODE_OPTIONS`, which `exposeChildLines` sets inside the test process. It dumps synchronously on `exit`, so `process.exit` does not lose the lines. Each test process reads its own dump dir (`PROBATIO_CHILD_DIR`). An inner Probatio run drops the outer run's map env (`childEnv`). Test: `tests/summary-order.test.ts`, second case. A child with `env: {}` stays `no coverage`.

Sealed:

- The CSV-271 kill was rechecked in a scratch clone. The killing test was edited by the fix commit. With its pre-fix assertion, all 92 `CSVFormatTest` tests pass on the reverted code. A full pre-fix suite run had 5 errors, all `Could not initialize plugin: MockMaker`, and the same 5 errors happen on the clean tree with this JDK.
- `mutate sealed` now reports `fixEdited` and `olderTestCatch`. When every kill came from a test the fix edited, the summary says so and `next` says to record a miss. Test: `tests/sealed-run.test.ts`, third case.
- A fresh `mutate sealed` on CSV-271 (scratch clone, confirm on, `--workers 1`) now writes a Java line map and reports `0 no coverage, 1 survived, 0 killed`. The map leaves out `testFormatThrowsNullPointerException` at `CSVPrinter.java:284`, because JaCoCo does not mark a line executed when a call on it throws. That is a limit of the Java map, written in the README.

Decided, 2026-10-08: a Node test file that does not link or parse under a mutant is `unviable`. A file whose top-level code throws is a kill by that file.

- Same verdict across languages. Java, Rust, Go, and C# reject a removed method at compile time, and that was already `unviable`.
- No test code ran. Any file that imports the name would fail, however weak its assertions. Calling that a kill would put an import-only file on the keep list, and it would reward agents for importing instead of asserting.
- For a ledger fix, `unviable` points to the next step: a hand-made mutant that keeps the API and restores the old behaviour.
- `src/mutate/load-failure.mjs` decides. The error must be a link or parse error (`SyntaxError`, `ERR_MODULE_NOT_FOUND`, and similar), with no stack frame in the project's own code. A `SyntaxError` from `JSON.parse` at the top of a test has a frame in that file, so it stays a kill. A tsx frame under `node_modules` is the loader. The process path reads the file's stderr from `test:stderr` events, and the batch path reads the error object. Tests: `tests/node-keep.test.ts`, the link case and the throw case with confirm on and off, plus a classifier case.

Self-score:

The tracked `ledger/` was rebuilt at `4c5f205` (`ledger build --package . --commit HEAD --out ledger --max-lines 300`). It holds 3 clean fixes (`950dea0`, `38c6f42`, `4eda582`), one hand fix (`93589bf` no longer applies), and three over the cap, including `4c5f205` itself at 1,105 lines. No version bump was counted. `922a301` and its golden are gone.

```bash
probatio mutate run --package . --patches ledger --out <fresh dir> --workers 1 --concurrency 4 --suite-timeout-ms 3000000 --test-timeout-ms 600000
```

Confirm on. Baseline green, 233s under the map. Wall time 20.5 minutes on this Mac. Summary: `0 no coverage, 1 survived, 1 killed, 1 did not build, 0 flaky, 0 timed out, 0 errored, of 3 finished.` Before this pass, both self patches were `no coverage`, because the tests start the CLI as a child process. One golden per patch is in `ledger/<id>.golden.json`.

- `38c6f42` was killed by `tests/keep-id.test.ts::junit parametrized keep id keeps the invocation Surefire recorded`.
- `950dea0` is `unviable`. The reverted fix removes `surefireArg`, which `tests/surefire-attr.test.ts` imports, so that file does not link. No test checks the behaviour the fix added. Next step: a hand-made mutant that keeps `surefireArg` and joins with a comma again.
- `4eda582` (`coverage-map.ts:105`, LLVM profile) survived here, because the LLVM tests skip on this Mac (`llvm-cov` is missing). CI's toolchain job has llvm and runs them.

Earlier self-scores in this pass read `950dea0` as survived (the batch dropped load failures), then flaky (the batch loader could not resolve `./x.js`), then killed (a load failure counted as a kill). Each was a runner bug or an open rule, fixed above.

Open:

- JaCoCo exception lines (above). A fix would need method-entry probes, or a whole-suite confirm of a Java survivor.
- `sealed.json` is a plain list an agent can read. The README says to keep it outside the scored agents' workspace.
- The commit `bb0dcc2` carries a machine-local author email. Changing that needs a history rewrite. Not done.
- Git tags for the npm versions are still not created. That needs the maintainer.
- `seed/` is Auspex code that nothing imports. Left in place.
