# Notes for Claude

Newest first. Start a review by rerunning the commands in the latest section.

## 2026-10-06 — sealed catch, maps, and a packed binary

A usable map is a non-empty file for node, pytest, c, go, maven, cargo, or dotnet. Rust and C# collect a line. A dark line is `no coverage` and does not start the suite. A child process the map cannot see is `no coverage`. `verify-change` puts the map's test names on `executed` and leaves `ran` as the importer list. Confirm stays off for `verify-change`. `deletedTests` stays 0. A keep id is rescored with `--only-test`, not `--suite-command`. `::command` is refused. Default `--workers` is 1.

C, C++, and COBOL are scored on the project's own suite, or the run refuses. A one-file compiler is not discovery. bugscpp images are not pulled on this Mac. SmartEnum is the small C# example. A few hundred C# tests would still pay one Coverlet process per name, so a repo that size is outside the section 2 timing.

The packed check is `tests/packaged-cli.test.ts`. It builds, packs, and installs outside this repo. The binary resolves to `dist/cli.js`, not `src/cli.ts`. Stdout is one JSON object. A red pytest baseline is `ok: false` and kills nothing. A green kill's id is `tests/test_gate.py::test_low` once. `npm pack --dry-run` lists `dist/cli.js`, `dist/mutate/sealed.js`, `dist/mutate/rust-coverage.js`, and `dist/mutate/csharp-coverage.js`. CI runs `npm test` on Ubuntu and does not skip that file.

### Four map runs, then the clones were reset

Each probe commit was removed with `git reset --hard` back to the clean upstream HEAD. None of these clones is dirty. The numbers below are from `tsx src/cli.ts`. No `PYTHONPATH` pointed at a scratch directory. No port 8080.

Pytest, Jinja, `~/Documents/coding_projects/Sample_Probatio_Separate/python`, commit `5ef7011`, BSD-3-Clause. `verify-change --package <python> --base HEAD~1 --commit HEAD --max-minutes 20` after a one-line commit. Suite `python3 pytest`, 911 tests, baseline 8297 ms, green. Summary `1 no coverage, 0 missed, 1 caught, 0 not run`. `executed` is `tests/test_api.py::TestExtendedAPI::test_expressions`. `confirmed` is false. The dark line is the uncalled `open_if_exists` helper. Two fresh `--only-test` rescores of that one id both killed the same mutant. Tally `deletedTests` 0. The covered edit was `or False` on a line the suite already runs. The dark edit was the same shape inside a function nothing calls.

Node, Commander.js, `~/Documents/coding_projects/Sample_Probatio_Separate/javascript`, commit `ba6d13d`, MIT. Discovery is `node --test` (no mocha string in package.json). 1373 tests, baseline 10038 ms, green, wall about 12 seconds. No `npm ci`. The harness color variables were unset, because `NO_COLOR` makes two Commander color tests fail before any mutant. A covered edit in `lib/error.js` survived and selected 206 of 1373 tests. A dark line in an uncalled helper was `no coverage`, command empty. The helper was only on the probe commit, which was reset.

Go, httprouter, `~/Documents/coding_projects/Sample_Probatio_Separate/go`, commit `4840180`, BSD-3-Clause. Suite `go test -json ./...`, 33 tests, baseline 2307 ms. Every existing statement was already covered, so the dark line was an uncalled helper added on a probe commit and then reset. The covered edit survived. The command was `go test -json ./... -run` with 6 names, not the full package list. The dark line did not start the suite. Go did not kill, so it was not rescored.

Java, jsoup, `~/Documents/coding_projects/Sample_Probatio_Separate/java`, commit `088614f`, MIT. `verify-change` with the default `--max-mutants` of 4. Suite `mvn -B test`, 2392 tests, baseline 12547 ms, wall about 111 seconds. Summary `0 no coverage, 1 missed, 3 caught, 5 not run`. The 5 not run are the dark `startsWithNewline` line and were not started. `executed` has 1447 names. A separate patch of that dark line was `no coverage` with an empty command. The keep list from the covered kill was rescored twice with `--only-test`. Both runs killed the same mutant. The second tally kept 1318 tests. `deletedTests` 0. Surefire received the long `-Dtest` list, including parameterized names such as `method(Parser)[1]`.

Express was not the Node proof. Mocha is not a line-map kind.

### Sealed catch

Commons CSV, Apache-2.0, worktree `~/Documents/coding_projects/Sample_Probatio_Beyond/commons-csv-271`, commit `2c83a308` (`Fixed CSV-271`). The fixed tree is green: 398 tests. The patch puts `CSVPrinter.printRecord(Object...)` back to `format.printRecord(out, values)` and `newRecord = true`. The label file stays outside the clone. Its three strings are absent from the source, the patch, and both JSON files.

Two `mutate sealed` runs, two `--out` directories, `--workers 1`, confirm left on (no `--no-confirm`), `--hide src/test/java/org/apache/commons/csv/issues/JiraCsv271Test.java`. Both `ok: true`. Same summary: `0 no coverage, 0 survived, 1 killed, 0 flaky, 0 timed out, 0 errored, of 1 finished.` `killedBy` is `org.apache.commons.csv.CSVFormatTest.testFormatThrowsNullPointerException`. `files` names `src/main/java/org/apache/commons/csv/CSVPrinter.java`. Baseline about 8.3 seconds. Each mutant step was about 20 seconds. The hidden test file is still in the clone. Both tallies: `deletedTests` 0, `pruning.mode` `advisory`, keep is that one older test.

The line map was not written on that run. Coverage reported `selectMethod(Class, String, Class[])` missing on JUnit platform 1.7. The first pass was the whole suite. Confirm then reran the older test twice, and it still failed. That is the kill. A later builder test pins `junit-jupiter` 5.7.2, failed on that missing method, and passed after the runner falls back to `selectMethod(Class, String)` only when the three-argument form is absent. JUnit 5.11 still uses the three-argument form.

Two earlier CSV subjects were not sealed. `f9e7d792` is ineligible: the revealing test is a method inside `CSVParserTest.java`, and `--hide` deletes a whole file. An older test did fail. `c15a06ee` stayed green (421 tests) after `JiraCsv288Test.java` was removed and `Lexer.java` was reverted. The search stopped at the eligible catch.

A JUnit 4 parameterized keep id such as `testShut[0]` is recorded in full. `-Dtest` drops the `[0]` index, because Surefire collects nothing for that index form, and keeps a `(Type)` list when the report has one.

Patch file `m-271.patch`. The hunk replaces `printRecord(Arrays.asList(values));` with `format.printRecord(out, values);` and `newRecord = true;`. The label file stays beside the run, not in the clone. Confirm stays on. Repeat with a fresh `--out` directory:

```bash
probatio mutate sealed \
  --package ~/Documents/coding_projects/Sample_Probatio_Beyond/commons-csv-271 \
  --repo ~/Documents/coding_projects/Sample_Probatio_Beyond/commons-csv-271 \
  --patches <directory that contains m-271.patch> \
  --label <label file> \
  --hide src/test/java/org/apache/commons/csv/issues/JiraCsv271Test.java \
  --out <fresh directory> \
  --workers 1
```

The first new CSV subject, `f9e7d792`, was ineligible. The next, `c15a06ee`, stayed green. The search then stopped on the eligible catch above. BugsInPy, and the small Go or Java fallback, were not opened.

`npm test` passed twice. Each run is 115 tests, 0 fail, exit 0.

From a directory that is not this repo, `npx --package probatio-0.1.0.tgz` ran the packed binary on a tiny pytest fixture. Version `0.1.0`. Summary `0 no coverage, 0 survived, 1 killed, 0 flaky, 0 timed out, 0 errored, of 1 finished.` Exit 0. The kill id is `tests/test_gate.py::test_low` once. `npm publish` was not run.

### Five sample roots

Each directory under the five roots was one bounded `mutate run`, an already scored line, or a written refusal. Huge trees under `cloned_sample_projects` were not walked: linux, llvm-project, rust (the compiler), Babylon.js, airflow, dotnet-dotnet, llama_index. redox was not started. bugscpp and defects4j were not initialized and no image was pulled. CobolCraft refused a `make test` that would download `server.jar`. Nothing was fetched. Auspex is one project. Its baseline timed out, so no mutant was scored. A timeout is its own count. No discovered suite is `no tests` or an unknown suite command. The four map repos above were not run again. A probe patch that names a missing file finishes as `1 errored`. That is not a kill.

## 2026-10-05 — edit size, then stop a slow unmapped batch

`verify-change` runs the discovered suite. It does not require a direct importer. Mutants are still only the changed lines, capped by `--max-mutants` (default 4). The suite timeout passed to the runner is 600000 ms. `ran` is still the direct-importer list. The tests that execute come from the line map, the same way `mutate run` selects them. No suite at all leaves the mutants in `notRun`.

`mutate run`, `verify-change`, and `check-kill` stop before the first mutant when the baseline has any failing test or returns no report. The summary names those tests, or the missing tool. Nothing is killed by a test that was already failing.

`mutate run` also stops before the first mutant when there is no usable line map, the baseline took 5000 ms or more, and more than 30 mutants are still pending. A usable map is a non-empty file for `node`, `pytest`, `c`, `go`, or `maven`. Go and Maven record file, line, and test name. Rust and C# stay name-filtered. A failed or missing map is the same stop. `next` says there is no line map and names the baseline. A faster suite still runs.

`mutate tally --out <run>` reads `results/` and `coverage-map.json`. It lists tests to keep, tests that killed nothing, and one gap per survivor. An unseen line is not a gap. `deletedTests` is 0. It does not delete a file.

The README mutate section matches these commands. A kill is the failing test, not a second failure of that test. Pytest node ids that `-k` cannot express are positional arguments. `tests/scale.test.ts` drives the CLI for the line map, the stop, the fast suite, and tally.

## 2026-10-04 — fourteen sample clones

`npm test` is 39 pass, 0 fail, exit 0. Each directory in `Sample_Probatio_Separate` was launched once with `mutate generate` and once with `mutate run` from `node dist/cli.js`. Generate used `--src . --max-mutants 4 --max-minutes 1`. Run used `--no-build --workers 1 --max-mutants 4 --max-minutes 1 --suite-timeout-ms 20000`, and its patches were that generate directory. Different clones overlapped in four batches. Two worktrees were never opened on the same clone.

Every stdout was one JSON object with `schemaVersion`, `ok`, `next`, `nextCall`, and `summary`. No home path. A non-zero exit is the limit named below.

### Summaries

- `cobol` generate: `4 mutants in 6 files. No string-literal mutant.` Run: `no tests in tests`. The four mutants are COBOL operators in `cow.cbl`.
- `rust` generate: `4 mutants in 10 files. No string-literal mutant.` Run: `no tests in tests`.
- `c` generate: `4 mutants in 12 files. No string-literal mutant.` Run: `no tests in tests`.
- `cpp` generate: `4 mutants in 25 files. No string-literal mutant.` Run: `no tests in tests`.
- `java` generate: `4 mutants in 205 files. No string-literal mutant.` Run: `no tests in tests`.
- `go` generate: `4 mutants in 6 files. No string-literal mutant.` Run: `no tests in tests`.
- `python` generate: `4 mutants in 60 files. No string-literal mutant.` The four are in `docs/examples/inline_gettext_extension.py`. Run, after the fix below: `baseline suite did not return a test report`.
- `javascript` generate: `4 mutants in 2 files. No string-literal mutant.` `.js` is not a source. The four are in `typings/index.test-d.ts`, which counts as TypeScript because the extension is `.ts` and the filename does not end in `.d.ts`. Run: `duplicate test title at runtime: when no extra arguments specified for program then variadic arg is empty array`. Node did start. That title is already in the clone's suite.
- `typescript` generate: `4 mutants in 3 files. No string-literal mutant.` The four are in `src/index.ts`. Run: `no tests in tests`.
- `csharp` generate: `0 mutants in 0 files. No string-literal mutant.` Run: `no patches`. `.cs` is not a source.
- `swift` generate: `0 mutants in 0 files. No string-literal mutant.` Run: `no patches`. `.swift` is not a source. The `tests` directory is empty.
- `asm-x86_64` generate: `0 mutants in 0 files. No string-literal mutant.` Run: `no patches`. `.asm` is not a source.
- `asm-aarch64` generate: `4 mutants in 9 files. No string-literal mutant.` Those nine files are C. The four mutants are in `Chapter 15/upper.c`. `.s` was left alone. Run: `no tests in tests`.
- `asm-riscv` generate: `4 mutants in 5 files. No string-literal mutant.` Those five files are C. The four mutants are in `aes128ctrbs/aes.c` and `aes128tables/aes.c`. `.S` was left alone. Run: `no tests in tests`.

### Fix

The first `python` run summary was `baseline suite did not return a test report (Traceback (most recent call last):)`. Those tests are pytest. The shipped reporter is unittest (`python3` and `py-reporter.py`). The import printed a traceback and no JSON line, and `prepare` pasted the first stderr line into the summary. A stack fragment in stdout is not an accepted limit.

`baselineReportError` in `src/mutate/run.ts` keeps a short ordinary detail and drops a traceback header, a `File "...", line N` frame, or a Node `at` frame. The summary is the limit itself: `baseline suite did not return a test report`. The runner stays unittest.

`tests/python-run.test.ts` launches `mutate run` on a tiny git repo whose `tests/test_raises.py` raises at import. Before the change the summary contained `Traceback` and the test failed. After the change the summary is exactly `baseline suite did not return a test report`, stdout has no `Traceback`, and the test passes. `dist` was rebuilt and only the `python` run JSON was saved again. The other thirteen pairs are from the first launch.

### Still a limit

- COBOL, Rust, C, C++, Java, and Go still generate only. C#, Swift, and the three assembly extensions are still unrecognized. The assembly-tree mutants in this pass are C operators on `.c` files in those trees.
- `python` still does not execute its pytest suite. The reporter asked for a unittest JSON line and got none.
- `javascript` stopped on a duplicate title that belongs to the clone.
- No new operators. No clone file was edited. Depth-1 history was not fetched. `verify-change`, ledger, matrix, and golden were not run on these clones.
- Every checkout's porcelain was empty before and after. Each clone has one worktree. `cargo`, `go test`, `pytest`, `javac`, the Swift compiler, `cobc`, `nasm`, and `yasm` were not started. The processes that did start are the runner's own `python3` and Node.

### Shortcut

- Baseline summary: `src/mutate/run.ts` `baselineReportError`. Test: `tests/python-run.test.ts`.
- Rerun from the builder: `node dist/cli.js mutate generate --package <clone> --src . --out <out> --max-mutants 4 --max-minutes 1`, then `node dist/cli.js mutate run --package <clone> --repo <clone> --patches <out>/mutants --out <run> --no-build --workers 1 --max-mutants 4 --max-minutes 1 --suite-timeout-ms 20000`.

## 2026-10-04 — four follow-ups, then Auspex again

`npm test` is 38 pass, 0 fail, exit 0. Every shipped command was launched twice on Auspex from `dist`. Pairs agree on `summary` and on ids. Auspex is back on `34896d39b65396e77ce02acbef5b1fc9b2be14d3`, branch `experiment/slim-tests`, porcelain empty, one worktree.

### What held

- `check-kill` runs only the tests that directly import the mutated file, the same set `verify-change` runs, and it does not launch the rest of the suite. A kill counts only when that failing test fails again on its own. A failure that passes on the rerun is `flaky` and is not done (`src/swarm/check-kill.ts:52`, `src/mutate/run.ts:111`).
- `verify-change` uses the same finder as `mutate generate` (`src/verify/change.ts:45`). A non-TypeScript diff gets that language's operators. A point in a string or a comment is not a mutant. TypeScript operators are unchanged.
- On the known `hostsAlign` edit, both runs exit 0 in 4.98s and 4.99s. Caught `me9d47a1cee3f` and `mbb5cb18587a1`. Missed `m4696f16d3c5d`. `rest` is 0. 40 affected tests. 0 golden contract changes.
- `golden check` on a copy of Auspex `tests/golden/out/live-host-change.json` treats `nextLead` as wording. `--update wording` rewrites that prose and leaves `hostChanged` and `nextCall` alone. The recorded file stays a flat map of case names, and `nextLead` is the only changed value. A `hostChanged` edit without a `Golden-Change:` trailer is refused and the recorded bytes stay put. The same edit with the trailer and `--update all` writes, and that file stays flat too. The Auspex file was not modified (`src/golden/check.ts:8`, `src/golden/check.ts:110`).
- Python is the one non-Node language the runner executes. A small `unittest` suite, command `python3`, killed the `and` to `or` mutant. Both launches: `1 killed, 0 survived, 0 flaky, 0 timed out, 0 errored, of 1 finished.` COBOL, Rust, C, C++, Java, and Go stay generate-only (`src/mutate/run.ts:307`, `src/mutate/py-reporter.py`).

### What failed

- Nothing in the suite crashed, and no Auspex command returned a non-JSON stdout.
- Auspex `check-kill` of `mf03fd0fc7b63` (`negate-ternary` at `src/agent-receipt.ts:46`) finished and is not done. Outcome `survived`. The direct-importer suite was green. Summary: `mf03fd0fc7b63 is not done. 0 killed, 1 survived, 0 flaky, 0 timed out, 0 errored, of 1 finished.` That is the mutant, not a timeout.

### Still a limit

- The runner still does not execute COBOL, Rust, C, C++, Java, or Go. The Python suite was a fixture, not a second project's own tests. Probatio is not general until the same contract works on a second TypeScript repo and one non-TypeScript repo whose tests run.
- The operator class is unchanged. No arithmetic replacement, statement deletion, or constant mutation.
- `verify-change` does not confirm a kill with an isolated rerun. That rerun belongs to `check-kill`.
- A C++ raw string and a Java text block are still not special-cased.
- `check-kill` runs that one patch, not every other patch sitting in the same directory.

### Shortcuts

- Direct importers and the single patch: `src/swarm/check-kill.ts:52`, `src/mutate/run.ts:111`. Test: `tests/swarm.test.ts`.
- Shared finder: `src/verify/change.ts:45`. Test: `tests/verify.test.ts`.
- `nextLead` is wording, and a flat file stays flat: `src/golden/check.ts:8`, `src/golden/check.ts:110`. Test: `tests/golden.test.ts`.
- Python suite command: `src/mutate/run.ts:307`. Test: `tests/python-run.test.ts`.

### Auspex verify-change

Detached worktree of `34896d39b65396e77ce02acbef5b1fc9b2be14d3`. One commit appended ` || a.length < 0` to `return siteFamily(left) === siteFamily(right)`. No `--max-tests`. No `--max-mutants 2`. The worktree was removed.

```bash
node dist/cli.js verify-change \
  --package <detached-worktree>/examples/auspex-ts \
  --repo <detached-worktree> \
  --out <scratch>/verify \
  --base HEAD~1 \
  --commit HEAD
```

Both summaries: `2 caught, 1 missed, 0 not run, 40 affected tests, 0 golden contract changes.`

### Other Auspex commands

- `mutate generate`: `1 mutants in 78 files. No string-literal mutant.` Id `mf03fd0fc7b63`.
- `mutate run`: `0 killed, 1 survived, 0 flaky, 0 timed out, 0 errored, of 1 finished.`
- `ledger build`: `39 fixes reverse cleanly. 115 need a hand-made mutant. 32 are over the line cap.`
- `matrix report`: `slimC holdout is 10/18. Train real is 32/33. Cover collapse on batch 2: coverK1 synthetic-1 279/279 against synthetic-2 330/420; coverK2 synthetic-1 279/279 against synthetic-2 374/420. 9 gaps. Pruning is advisory and deleted 0 tests.`
- `golden check`: `Golden rows match.`
- `gap fix`: `gap-dogfood is fixed, guarded by examples/auspex-ts/src/live-host-change.ts:1.`
- `gap revert`: `fixed in 34896d39b65396e77ce02acbef5b1fc9b2be14d3, guarded by examples/auspex-ts/src/live-host-change.ts:1; the guard broke`
- `guard check`: `examples/auspex-ts/src/live-host-change.ts:1 guards nothing.`
- `findings add`: `Recorded find-dogfood as equivalent.`
- `status`: `0 open gaps, 0 fixes, 0 decisions.`
- `seal`: `Sealed 1 ids.`
- `queue seed`: `Seeded 1 queue items.`
- `queue claim`: `probe claimed q-dogfood until 6000.`
- `queue reap`: `Returned 1 expired claims.`
- `check-kill`: `mf03fd0fc7b63 is not done. 0 killed, 1 survived, 0 flaky, 0 timed out, 0 errored, of 1 finished.`

## 2026-10-04 — dogfood on Auspex and the sample trees

Every shipped command was launched twice on Auspex and twice on each of the 52 sample projects. Pairs agree on `summary` and on reported ids. `npm test` is 34 pass, exit 0. The per-project lines are in the vault note `Dogfood 2026-10-04.md`.

### Decisions

- `--max-commits N` reads the newest N non-merge commits (`git rev-list --no-merges -n N+1`). When more exist, the summary says `Stopped after N commits. History was not fully scanned.` The default is still the whole history (`src/cli.ts:211`, `src/ledger/build.ts:127`). A tree is checked out only when a fix inside that cap has to be applied.
- Sample ledgers passed `--max-commits 20` and `--max-lines 300`. Auspex was a full scan: `39 fixes reverse cleanly. 115 need a hand-made mutant. 32 are over the line cap.`
- `mutate run` returns `no tests in <dir>` before it creates a worktree (`src/mutate/run.ts:110`).
- A worktree package with no `node_modules` is linked to the main checkout's (`src/mutate/run.ts:472`). That is what let the Auspex `verify-change` baseline load.
- A COBOL line whose column 7 is `*` or `/` is a comment (`src/mutate/text-operators.ts:230`). `*>` is unchanged. Area A/B code is still free-format.
- Generate on samples used `--src src --max-mutants 4 --max-minutes 1`. Run used `--no-build --max-mutants 1 --max-minutes 1`. No cargo, go test, pytest, JUnit, or COBOL compiler was started. State, page, and out dirs were outside the trees. `queue claim` and `queue reap` used a fixed `--now`. Golden check did not pass `--update`.

### Unsure

- A C++ raw string and a Java text block are still not special-cased. A `/*` inside one can still be read as a comment.
- `verify-change` still parses only TypeScript (`src/verify/change.ts`). The sample diffs were `HEAD` against `HEAD`, so this did not show.
- Auspex `check-kill` stopped with `m09e1bb4350ce is not done. baseline suite timed out` at a 90s suite cap. The kill was not confirmed.
- A COBOL history shorter than 20 commits is read to the end. The summary does not claim a stop. A one-commit repository cannot be partially scanned.
- Fixed-format continuation (`-` in column 7) and sequence numbers are not otherwise special-cased.

### Shortcuts

- Commit cap: `src/ledger/build.ts:127`. CLI flag: `src/cli.ts:211`. Test: `tests/ledger.test.ts`.
- No checkout without tests: `src/mutate/run.ts:110`. Test: `tests/mutate.test.ts`.
- Main-checkout `node_modules`: `src/mutate/run.ts:472`. Test: `tests/mutate.test.ts`.
- Fixed-format comment: `src/mutate/text-operators.ts:230`. Test: `tests/languages.test.ts`.

### Auspex verify-change

Detached worktree of `34896d39b65396e77ce02acbef5b1fc9b2be14d3` on `experiment/slim-tests`. One commit appended ` || a.length < 0` to `return siteFamily(left) === siteFamily(right)`. No `--max-tests`. The worktree was removed. Porcelain stayed clean. One worktree remained.

```bash
node dist/cli.js verify-change \
  --package <detached-worktree>/examples/auspex-ts \
  --repo <detached-worktree> \
  --base HEAD~1 \
  --commit HEAD \
  --max-mutants 4 \
  --max-minutes 3 \
  --out <scratch>/auspex/verify-change-1
```

Run 1 exit 0 in 5.24s. Run 2 exit 0 in 5.21s. Both summaries:

`2 caught, 1 missed, 0 not run, 40 affected tests, 0 golden contract changes.`

Caught `me9d47a1cee3f` and `mbb5cb18587a1`. Missed `m4696f16d3c5d`. `notRun` is empty, so rest is 0.

Both launches of the other commands:

- `mutate-generate` exit 0/0. 4 mutants in 78 files. No string-literal mutant.
- `mutate-run` exit 0/0. 0 killed, 1 survived, 0 flaky, 0 timed out, 0 errored, of 1 finished.
- `ledger-build` exit 0/0. 39 fixes reverse cleanly. 115 need a hand-made mutant. 32 are over the line cap.
- `matrix-report` exit 0/0. slimC holdout is 10/18. Train real is 32/33. Cover collapse on batch 2: coverK1 synthetic-1 279/279 against synthetic-2 330/420; coverK2 synthetic-1 279/279 against synthetic-2 374/420. 9 gaps. Pruning is advisory and deleted 0 tests.
- `golden-check` exit 0/0. Golden rows match.
- `gap-fix` exit 0/0. gap-dogfood is fixed, guarded by examples/auspex-ts/src/live-host-change.ts:1.
- `gap-revert` exit 0/0. fixed in 34896d39b65396e77ce02acbef5b1fc9b2be14d3, guarded by examples/auspex-ts/src/live-host-change.ts:1; the guard broke
- `guard-check` exit 0/0. examples/auspex-ts/src/live-host-change.ts:1 guards nothing.
- `findings-add` exit 0/0. Recorded find-dogfood as equivalent.
- `status` exit 0/0. 0 open gaps, 0 fixes, 0 decisions.
- `seal` exit 0/0. Sealed 1 ids.
- `queue-seed` exit 0/0. Seeded 1 queue items.
- `queue-claim` exit 0/0. probe claimed q-dogfood until 6000.
- `queue-reap` exit 0/0. Returned 1 expired claims.
- `check-kill` exit 1/1. m09e1bb4350ce is not done. baseline suite timed out

## 2026-10-04 — mutation generation for COBOL, Rust, C, C++, Java, Go, and Python

### Decisions

- One generate path. The walk picks a finder from the extension (`src/mutate/find.ts:5`). TypeScript stays on the TypeScript parser. The other languages never enter it.
- COBOL, Rust, C, C++, Java, Go, and Python get the same operator class: boolean swaps and relational swaps, plus `true`/`false` where that word exists. COBOL is free-format (`*>` comments, `AND`/`OR`). Fixed-column COBOL is not handled.
- A string or a comment is masked before a point is proposed (`src/mutate/text-operators.ts:179`), so a `/*`, `*>`, or `#` inside a string does not hide a later operator and does not become a violation. The whole-batch refusal is unchanged for a TypeScript violation.
- Bare `<` and `>` are comparisons only when both sides are whitespace (`src/mutate/text-operators.ts`). That leaves `#include <stdio.h>`, `vector<int>`, and `List<String>` alone. `<` inside `<-`, `>` inside `=>`, and `<=`/`>=` inside `<<=`, `>>=`, or `<=>` are not comparisons.
- `mutate run` and `verify-change` still use the Node test runner. These generators only write patches.

### Unsure

- A C++ raw string (`R"(...)"`) and a Java text block are not special-cased. A `/*` inside one of those can still be read as a comment.

### Shortcuts

- Recognized extensions are in `textKind` (`src/mutate/text-operators.ts:25`): `.py`, `.cob`, `.cbl`, `.cobol`, `.rs`, `.go`, `.java`, `.c`, `.h`, `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`, `.hxx`. `.d.ts` stays out.
- The CLI check is `tests/languages.test.ts`. It spawns `tsx src/cli.ts` once per language, checks the language's own token, applies each patch, and checks the sentinel is not on a changed line.

## 2026-10-04 — sample trees and another Auspex verify-change

### Decisions

- The sample launch is `mutate generate` with `--src src`, `--max-mutants 4`, and `--max-minutes 1`. Output directories stay under the scratch dir, not inside the sample checkouts.
- `--max-minutes` stops `generate` during the source walk (`src/mutate/generate.ts:43`, and again at `src/mutate/generate.ts:97`). `node_modules` and `.git` are not entered (`src/mutate/generate.ts:112`).
- A `//` or `/*` that starts inside a string, a template, or a regex is not a comment (`src/mutate/operators.ts:133`, `src/mutate/operators.ts:152`). The first dotnet launch refused the batch because `jiterpreter.ts:930` has `` `// ${... || ...}` ``. The saved stdout is the re-run after that fix: 4 mutants in 354 files.
- Seventeen sample git checkouts were already dirty. None of them were cleaned. A launch is accepted when `git status --porcelain` is unchanged. Two trees are not git repos (`Python Translated Example`, `cobol-24-hours`); nothing was written inside them.
- No sample project has a `tests/*.test.ts` file in the top `tests` directory, which is the only place the runner looks (`src/mutate/run.ts:474`). `verify-change` and `mutate run` were not launched on those trees.

### Unsure

- Default `--src src` visits 0 files on Babylon.js, linux, llvm-project, airflow, llama_index, and the COBOL trees. A two-level look found no `.ts` under those except Babylon's root Playwright configs and, deeper, `packages/`. I did not point `--src` at `.` for linux or llvm-project.
- A follow-up on Babylon.js with `--src .` is not the saved default envelope. It exited 0 in 3.78 seconds: `4 mutants in 3783 files. No string-literal mutant.` The first attempt, before the regex fix, refused `packages/tools/playground/src/tools/monaco/run/runner.ts:305` because `/\/*$/` looked like a block comment (71 violations).

### Shortcuts

- `verify-change` still counts a kill from one suite run (`src/verify/change.ts:95`) and does not build the package.
- The sample `generate` calls did not run the projects' own tests.
- Python, Rust, C, C++, Java, Go, and COBOL are not mutated. An empty TypeScript result is the whole report.

### Numbers

`npm test` (`tsx --test tests/*.test.ts`): 27 pass, 0 fail, exit 0. `tests/generate-budget.test.ts` failed before the time-budget fix (`budgetHit` was missing) and passes after. `tests/operators.test.ts` failed before the template-comment fix and before the regex fix, and passes after.

Auspex `verify-change` on a detached worktree of `34896d3`. One commit changed `hostsAlign` to `return siteFamily(left) === siteFamily(right) || a.length < 0`. No `--max-tests`. Run 1 exit 0 in 5.34 seconds. Run 2 exit 0 in 5.38 seconds. The JSON objects match except `full`. Stdout below is run 1. The worktree was removed. Auspex stayed on `experiment/slim-tests` at `34896d39b65396e77ce02acbef5b1fc9b2be14d3`, status clean, one worktree.

```bash
node dist/cli.js verify-change \
  --package <detached-worktree>/examples/auspex-ts \
  --repo <detached-worktree> \
  --base HEAD~1 \
  --commit HEAD \
  --max-minutes 3 \
  --out <scratch>/verify-out-1
```

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "verify-change",
  "summary": "2 caught, 1 missed, 0 not run, 40 affected tests, 0 golden contract changes.",
  "next": "m4696f16d3c5d at src/live-host-change.ts:112",
  "nextCall": null,
  "affected": [
    "tests/agent-receipt.test.ts",
    "tests/agents-sync.test.ts",
    "tests/any-host.test.ts",
    "tests/assert-receipt-origin.test.ts",
    "tests/budget.test.ts",
    "tests/cli-help.test.ts",
    "tests/connect.test.ts",
    "tests/content.test.ts",
    "tests/cookie-save.test.ts",
    "tests/demo.test.ts",
    "tests/desktop.test.ts",
    "tests/dist.test.ts",
    "tests/door-await-contract.test.ts",
    "tests/editor-fold.test.ts",
    "tests/fold-steer.test.ts",
    "tests/haystack.test.ts",
    "tests/job.test.ts",
    "tests/lifecycle.test.ts",
    "tests/live-host-change.test.ts",
    "tests/live-smoke.test.ts",
    "tests/login-trace.test.ts",
    "tests/login.test.ts",
    "tests/mcp-check-verify.test.ts",
    "tests/mcp-schema.test.ts",
    "tests/next-call.test.ts",
    "tests/operator-doors.test.ts",
    "tests/page-action-ceiling.test.ts",
    "tests/profile-host-advice.test.ts",
    "tests/profile-persist.test.ts",
    "tests/profile-slug.test.ts",
    "tests/profile-status.test.ts",
    "tests/receipt-schema.test.ts",
    "tests/receipt.test.ts",
    "tests/save-window.test.ts",
    "tests/security-guards.test.ts",
    "tests/shutdown.test.ts",
    "tests/solari-health.test.ts",
    "tests/stream-deadline.test.ts",
    "tests/sweep.test.ts",
    "tests/vwp-refuse.test.ts"
  ],
  "ran": [
    "tests/fold-steer.test.ts",
    "tests/live-host-change.test.ts",
    "tests/login.test.ts"
  ],
  "notRan": [],
  "caught": [
    {
      "id": "me9d47a1cee3f",
      "file": "src/live-host-change.ts",
      "line": 112,
      "op": "eq-to-neq"
    },
    {
      "id": "mbb5cb18587a1",
      "file": "src/live-host-change.ts",
      "line": 112,
      "op": "or-to-and"
    }
  ],
  "missed": [
    {
      "id": "m4696f16d3c5d",
      "file": "src/live-host-change.ts",
      "line": 112,
      "op": "lt-to-le"
    }
  ],
  "other": [],
  "notRun": [],
  "goldenContract": [],
  "rest": 0,
  "full": "/var/folders/yq/wkr09jbx5qn_gtc9p7hrq4sw0000gn/T/grok-goal-e3b9f813e66b/implementer/verify-out-1/verify.json"
}
```

### Sample projects

Each line is the command, the exit, and the summary from the saved stdout.

- `cloned/Babylon.js` — `mutate generate --package <project> --src src --out <scratch>/external/cloned/Babylon.js-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cloned/airflow` — `mutate generate --package <project> --src src --out <scratch>/external/cloned/airflow-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cloned/dotnet-dotnet` — `mutate generate --package <project> --src src --out <scratch>/external/cloned/dotnet-dotnet-out --max-mutants 4 --max-minutes 1` — exit 0 — 4 mutants in 354 files. No string-literal mutant.
- `cloned/linux` — `mutate generate --package <project> --src src --out <scratch>/external/cloned/linux-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cloned/llama_index` — `mutate generate --package <project> --src src --out <scratch>/external/cloned/llama_index-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cloned/llvm-project` — `mutate generate --package <project> --src src --out <scratch>/external/cloned/llvm-project-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cloned/redox` — `mutate generate --package <project> --src src --out <scratch>/external/cloned/redox-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cloned/rust` — `mutate generate --package <project> --src src --out <scratch>/external/cloned/rust-out --max-mutants 4 --max-minutes 1` — exit 0 — 4 mutants in 25 files. No string-literal mutant.
- `cobol/BankDemo` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/BankDemo-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/COBOL-Examples-shamrice` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/COBOL-Examples-shamrice-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/COBOL-Legacy-Benchmark-Suite` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/COBOL-Legacy-Benchmark-Suite-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/COBOL-Samples` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/COBOL-Samples-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/COBOL-to-Java-Conversion-Samples` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/COBOL-to-Java-Conversion-Samples-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Cobol` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Cobol-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Cobol-Code` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Cobol-Code-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Cobol-Martinfx` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Cobol-Martinfx-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Cobol-Programming-Collection` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Cobol-Programming-Collection-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Cobol-Projects` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Cobol-Projects-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/CobolCraft` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/CobolCraft-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/EnterpriseCOBOLv6.3` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/EnterpriseCOBOLv6.3-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Micro-Focus-Unit-Testing-Framework-Samples` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Micro-Focus-Unit-Testing-Framework-Samples-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Modern-mainframe-development` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Modern-mainframe-development-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Open-COBOL-ESQL` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Open-COBOL-ESQL-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/Python Translated Example` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/Python Translated Example-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/TypeCobol` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/TypeCobol-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/aws-mainframe-modernization-carddemo` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/aws-mainframe-modernization-carddemo-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/beg-cobol-for-programmers` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/beg-cobol-for-programmers-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/che-che4z-lsp-for-cobol` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/che-che4z-lsp-for-cobol-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cics-banking-cbsa` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cics-banking-cbsa-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cics-genapp` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cics-genapp-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cobol-24-hours` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cobol-24-hours-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cobol-examples` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cobol-examples-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cobol-examples-michelou` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cobol-examples-michelou-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cobol-examples-writ3it` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cobol-examples-writ3it-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cobol-is-fun` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cobol-is-fun-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cobol-programming-course` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cobol-programming-course-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/cobol-unit-test` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/cobol-unit-test-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/db2-samples` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/db2-samples-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/dbb-zappbuild` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/dbb-zappbuild-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/gem-gnu-DB2` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/gem-gnu-DB2-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/gnucobol` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/gnucobol-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/gnucobol-examples` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/gnucobol-examples-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/hello_business` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/hello_business-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/idz-utilities` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/idz-utilities-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/koopa-nist` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/koopa-nist-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/mapa` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/mapa-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/opensourcecobol4j` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/opensourcecobol4j-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/proleap-cobol-parser` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/proleap-cobol-parser-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/sample-ims-large-data` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/sample-ims-large-data-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/taxe-fonciere` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/taxe-fonciere-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/vsamcobol` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/vsamcobol-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.
- `cobol/zBANK` — `mutate generate --package <project> --src src --out <scratch>/external/cobol/zBANK-out --max-mutants 4 --max-minutes 1` — exit 0 — 0 mutants in 0 files. No string-literal mutant.

## 2026-10-04 — verify-change, MCP, queue, memory, golden, matrix

### Decisions

- The matrix parses `round2-union-scores.tsv` and `round2-direct-scores.txt`. It does not rerun cover or regenerate mutants. The headline gaps are the missed ids of the direct suite with the weakest holdout (`src/matrix/report.ts:193`). On this file that is slim C, 10/18, train real 32/33.
- A gap's `file:line` is the first hunk from `filesInDiff`. Patch names are tried as the id, then `r-`, `h-`, the bare sha, then `ho-`.
- Pruning names zero-kill tests and always reports `deletedTests: 0` (`src/matrix/report.ts:42`). Nothing is unlinked.
- Golden wording is `next`. Every other field is the contract. `--update wording` rewrites `next`. A contract write also needs `Golden-Change: <row>: <why>`. A home path, or `ok` without a reason, fails and the file is not written (`src/golden/check.ts:104`).
- Findings are append-only JSONL (`src/memory/state.ts:166`). A decision's hash is checked when `status` runs. A changed line is `stale` without rewriting the log.
- A guard change is refused unless `Guard-Change: <guard>: <why>` names it. The why is separated by colon and a space, so `tests/gate.test.ts:2` stays one token.
- Reverting a fixed gap keeps the id and says `fixed in <commit>, guarded by <guard>; the guard broke`.
- Sealed ids are omitted from `status`, `PROBATIO.md`, and `queue seed`. If visible kills rise and sealed kills do not, the summary is exactly `these tests fit the yardstick, not the code.`
- `verify-change` generates mutants only on the diff's new lines. It runs every direct importer unless `--max-tests` is set (`src/verify/change.ts:73`, default in `src/cli.ts:458`). Mutants past `--max-mutants` go in `notRun`, and `rest` is that count (`src/verify/change.ts:147`). One suite failure counts as caught (`confirm: false` at `src/verify/change.ts:95`).
- The queue claim writes the lease, then `renameSync` from `queue/<id>.json` to `claimed/<agent>-<id>.json`. `reap` moves a claim back when `leasedUntil` is at or before `--now`.
- `check-kill` calls `runMutants`. It is done only when that id's outcome is `killed` and `baseline.json` says the suite passed.
- MCP is a stdio server. `tools/call` on the `probatio` tool spawns the CLI and returns that stdout.

### Unsure

- `ho-1cfec4a` is reported at `examples/auspex-ts/src/text.ts:1`. That is the first hunk (a new import). The behaviour change in the same patch is later in `maskSecrets`. I did not pick a later hunk.
- slim C and slim D have `lines: null`. The direct score file has no line counts. I did not copy the README's 8,912 or 16,232.
- `v063`, `v097`, and `v419` stay on the slim D row. There is no patch, so they are not given a `file:line`.
- `lt-to-le` on `a.length < 0` survived both Auspex runs. That line is only reached when `a` is non-empty, so `< 0` and `<= 0` behave the same. I did not mark the mutant equivalent by hand. `eq-to-neq` and `or-to-and` were killed.
- `status` does not draw a random sealed slice. `probatio seal --id` writes the list. `chooseSealed` exists and is deterministic, and this acceptance did not need it.
- Golden check does not execute `seed/golden-harness` cases. It compares two JSON tables.
- The `check-kill` fixture passes `--no-confirm`. The CLI default is still two isolated reruns.

### Shortcuts

- `verify-change` does not confirm kills twice (`src/verify/change.ts:95`). It also does not build the package (`build: null` in that same call).
- Direct importers only (`src/mutate/affected.ts:78`). On the Auspex edit those were `tests/fold-steer.test.ts`, `tests/live-host-change.test.ts`, and `tests/login.test.ts`. The other affected tests were listed and not run.
- `onlyTests` on `runMutants` (`src/mutate/run.ts:152`) is how that narrow run is selected. The default of `mutate run` is still the whole suite.
- A ledger entry still has no guard field. Guards live in `.probatio/state.json`, not in `ledger.json`.
- Python is not started.

### Numbers

`npm test` (`tsx --test tests/*.test.ts`): 24 pass, 0 fail, exit 0. That run is the acceptance output for golden (`tests/golden.test.ts`), memory (`tests/memory.test.ts`), the queue and `check-kill` (`tests/swarm.test.ts`), and the fixture `verify-change` (`tests/verify.test.ts`). The new cap assertion failed before the `rest` fix (`notRun` was undefined) and passed after.

## 2026-10-04 — matrix acceptance

Matrix, twice, stdout byte-identical. Both exit 0. slim C holdout 10/18, train real 32/33, cover k=1 and k=2 are the batch-2 collapse, 9 gaps, `rest` 0, `deletedTests` 0.

```bash
node dist/cli.js matrix report \
  --scores ~/Documents/coding_projects/auspex/experiments/slim-tests/round2-union-scores.tsv \
  --direct ~/Documents/coding_projects/auspex/experiments/slim-tests/round2-direct-scores.txt \
  --patches ~/Documents/coding_projects/auspex/experiments/slim-tests/mutants/holdout \
  --patches ~/Documents/coding_projects/auspex/experiments/slim-tests/mutants/train \
  --out <scratch>/matrix-out
```

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "matrix.report",
  "summary": "slimC holdout is 10/18. Train real is 32/33. Cover collapse on batch 2: coverK1 synthetic-1 279/279 against synthetic-2 330/420; coverK2 synthetic-1 279/279 against synthetic-2 374/420. 9 gaps. Pruning is advisory and deleted 0 tests.",
  "next": "ho-1cfec4a at examples/auspex-ts/src/text.ts:1",
  "nextCall": null,
  "rows": [
    {
      "set": "full",
      "name": "full",
      "source": "union",
      "lines": 18127,
      "holdout": {
        "caught": 18,
        "total": 18
      },
      "real": {
        "caught": 33,
        "total": 33
      },
      "synthetic1": {
        "caught": 266,
        "total": 279
      },
      "synthetic2": {
        "caught": 412,
        "total": 420
      },
      "missed": []
    },
    {
      "set": "slimA",
      "name": "slimA",
      "source": "union",
      "lines": 17028,
      "holdout": {
        "caught": 18,
        "total": 18
      },
      "real": {
        "caught": 32,
        "total": 33
      },
      "synthetic1": {
        "caught": 266,
        "total": 279
      },
      "synthetic2": {
        "caught": 411,
        "total": 420
      },
      "missed": []
    },
    {
      "set": "slimB0",
      "name": "slimB0",
      "source": "union",
      "lines": 12234,
      "holdout": {
        "caught": 14,
        "total": 18
      },
      "real": {
        "caught": 30,
        "total": 33
      },
      "synthetic1": {
        "caught": 196,
        "total": 279
      },
      "synthetic2": {
        "caught": 313,
        "total": 420
      },
      "missed": []
    },
    {
      "set": "slimB",
      "name": "slimB",
      "source": "union",
      "lines": 13378,
      "holdout": {
        "caught": 18,
        "total": 18
      },
      "real": {
        "caught": 31,
        "total": 33
      },
      "synthetic1": {
        "caught": 277,
        "total": 279
      },
      "synthetic2": {
        "caught": 417,
        "total": 420
      },
      "missed": []
    },
    {
      "set": "coverK1",
      "name": "coverK1",
      "source": "union",
      "lines": 4539,
      "holdout": {
        "caught": 6,
        "total": 18
      },
      "real": {
        "caught": 31,
        "total": 33
      },
      "synthetic1": {
        "caught": 279,
        "total": 279
      },
      "synthetic2": {
        "caught": 330,
        "total": 420
      },
      "missed": []
    },
    {
      "set": "coverK2",
      "name": "coverK2",
      "source": "union",
      "lines": 7014,
      "holdout": {
        "caught": 11,
        "total": 18
      },
      "real": {
        "caught": 31,
        "total": 33
      },
      "synthetic1": {
        "caught": 279,
        "total": 279
      },
      "synthetic2": {
        "caught": 374,
        "total": 420
      },
      "missed": []
    },
    {
      "set": "slimC",
      "name": "slim C (run directly)",
      "source": "direct",
      "lines": null,
      "holdout": {
        "caught": 10,
        "total": 18
      },
      "real": {
        "caught": 32,
        "total": 33
      },
      "synthetic1": null,
      "synthetic2": {
        "caught": 373,
        "total": 420
      },
      "missed": [
        "ho-1cfec4a",
        "ho-4404714",
        "ho-472fdd1",
        "ho-6a9852a",
        "ho-983893c",
        "ho-b4525eb",
        "ho-c1cf241",
        "ho-c96a8a2",
        "r-f05408b"
      ]
    },
    {
      "set": "slimD",
      "name": "slim D (run directly, before the post-holdout connect fix)",
      "source": "direct",
      "lines": null,
      "holdout": {
        "caught": 18,
        "total": 18
      },
      "real": {
        "caught": 33,
        "total": 33
      },
      "synthetic1": null,
      "synthetic2": {
        "caught": 417,
        "total": 420
      },
      "missed": [
        "v063",
        "v097",
        "v419"
      ]
    }
  ],
  "collapse": {
    "batch2": [
      {
        "set": "coverK1",
        "synthetic1": {
          "caught": 279,
          "total": 279
        },
        "synthetic2": {
          "caught": 330,
          "total": 420
        }
      },
      {
        "set": "coverK2",
        "synthetic1": {
          "caught": 279,
          "total": 279
        },
        "synthetic2": {
          "caught": 374,
          "total": 420
        }
      }
    ],
    "holdoutBelowReference": [
      {
        "set": "slimB0",
        "holdout": {
          "caught": 14,
          "total": 18
        },
        "reference": {
          "caught": 18,
          "total": 18
        }
      },
      {
        "set": "coverK1",
        "holdout": {
          "caught": 6,
          "total": 18
        },
        "reference": {
          "caught": 18,
          "total": 18
        }
      },
      {
        "set": "coverK2",
        "holdout": {
          "caught": 11,
          "total": 18
        },
        "reference": {
          "caught": 18,
          "total": 18
        }
      },
      {
        "set": "slimC",
        "holdout": {
          "caught": 10,
          "total": 18
        },
        "reference": {
          "caught": 18,
          "total": 18
        }
      }
    ]
  },
  "gaps": [
    {
      "id": "ho-1cfec4a",
      "file": "examples/auspex-ts/src/text.ts",
      "line": 1,
      "locations": [
        {
          "file": "examples/auspex-ts/src/text.ts",
          "line": 1
        }
      ]
    },
    {
      "id": "ho-4404714",
      "file": "examples/auspex-ts/src/text.ts",
      "line": 45,
      "locations": [
        {
          "file": "examples/auspex-ts/src/text.ts",
          "line": 45
        }
      ]
    },
    {
      "id": "ho-472fdd1",
      "file": "examples/auspex-ts/src/job-store.ts",
      "line": 209,
      "locations": [
        {
          "file": "examples/auspex-ts/src/job-store.ts",
          "line": 209
        }
      ]
    },
    {
      "id": "ho-6a9852a",
      "file": "examples/auspex-ts/src/profiles.ts",
      "line": 369,
      "locations": [
        {
          "file": "examples/auspex-ts/src/profiles.ts",
          "line": 369
        },
        {
          "file": "examples/auspex-ts/src/reap.ts",
          "line": 144
        },
        {
          "file": "examples/auspex-ts/src/solari.ts",
          "line": 79
        }
      ]
    },
    {
      "id": "ho-983893c",
      "file": "examples/auspex-ts/src/profile-persist.ts",
      "line": 323,
      "locations": [
        {
          "file": "examples/auspex-ts/src/profile-persist.ts",
          "line": 323
        }
      ]
    },
    {
      "id": "ho-b4525eb",
      "file": "examples/auspex-ts/src/assert_receipt.py",
      "line": 169,
      "locations": [
        {
          "file": "examples/auspex-ts/src/assert_receipt.py",
          "line": 169
        }
      ]
    },
    {
      "id": "ho-c1cf241",
      "file": "examples/auspex-ts/src/solari.ts",
      "line": 21,
      "locations": [
        {
          "file": "examples/auspex-ts/src/solari.ts",
          "line": 21
        }
      ]
    },
    {
      "id": "ho-c96a8a2",
      "file": "examples/auspex-ts/src/cookie-save.ts",
      "line": 247,
      "locations": [
        {
          "file": "examples/auspex-ts/src/cookie-save.ts",
          "line": 247
        }
      ]
    },
    {
      "id": "r-f05408b",
      "file": "examples/auspex-ts/src/desktop.ts",
      "line": 344,
      "locations": [
        {
          "file": "examples/auspex-ts/src/desktop.ts",
          "line": 344
        },
        {
          "file": "examples/auspex-ts/src/sandbox.ts",
          "line": 339
        },
        {
          "file": "examples/auspex-ts/src/solari.ts",
          "line": 280
        }
      ]
    }
  ],
  "rest": 0,
  "pruning": {
    "mode": "advisory",
    "deletedTests": 0,
    "advice": []
  },
  "full": "/var/folders/yq/wkr09jbx5qn_gtc9p7hrq4sw0000gn/T/grok-goal-5bd9307357c9/implementer/matrix-out/matrix.json"
}
```

## 2026-10-04 — verify-change acceptance

`verify-change` on a detached worktree of Auspex `34896d3`. One commit there changed `hostsAlign` to `return siteFamily(left) === siteFamily(right) || a.length < 0`. The commit identity was `probatio@example.com`. The worktree was removed. Auspex stayed on `experiment/slim-tests` at `34896d39b65396e77ce02acbef5b1fc9b2be14d3`, status clean, and `git worktree list` showed only that checkout.

No `--max-mutants` and no `--max-tests`. The default runs every direct importer and keeps every diff-scoped mutant inside the default cap of 4. This edit has three mutants, so `rest` is 0 because all three ran. `or-to-and` was killed. `lt-to-le` survived.

Run 1 exit 0 in 5.11 seconds. Run 2 exit 0 in 5.05 seconds. The two JSON objects match except `full` (run 2 writes `verify-out-2/verify.json`). Stdout below is run 1.

```bash
node dist/cli.js verify-change \
  --package <detached-worktree>/examples/auspex-ts \
  --repo <detached-worktree> \
  --base HEAD~1 \
  --commit HEAD \
  --max-minutes 3 \
  --out /var/folders/yq/wkr09jbx5qn_gtc9p7hrq4sw0000gn/T/grok-goal-5bd9307357c9/implementer/verify-out-1
```

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "verify-change",
  "summary": "2 caught, 1 missed, 0 not run, 40 affected tests, 0 golden contract changes.",
  "next": "m4696f16d3c5d at src/live-host-change.ts:112",
  "nextCall": null,
  "affected": [
    "tests/agent-receipt.test.ts",
    "tests/agents-sync.test.ts",
    "tests/any-host.test.ts",
    "tests/assert-receipt-origin.test.ts",
    "tests/budget.test.ts",
    "tests/cli-help.test.ts",
    "tests/connect.test.ts",
    "tests/content.test.ts",
    "tests/cookie-save.test.ts",
    "tests/demo.test.ts",
    "tests/desktop.test.ts",
    "tests/dist.test.ts",
    "tests/door-await-contract.test.ts",
    "tests/editor-fold.test.ts",
    "tests/fold-steer.test.ts",
    "tests/haystack.test.ts",
    "tests/job.test.ts",
    "tests/lifecycle.test.ts",
    "tests/live-host-change.test.ts",
    "tests/live-smoke.test.ts",
    "tests/login-trace.test.ts",
    "tests/login.test.ts",
    "tests/mcp-check-verify.test.ts",
    "tests/mcp-schema.test.ts",
    "tests/next-call.test.ts",
    "tests/operator-doors.test.ts",
    "tests/page-action-ceiling.test.ts",
    "tests/profile-host-advice.test.ts",
    "tests/profile-persist.test.ts",
    "tests/profile-slug.test.ts",
    "tests/profile-status.test.ts",
    "tests/receipt-schema.test.ts",
    "tests/receipt.test.ts",
    "tests/save-window.test.ts",
    "tests/security-guards.test.ts",
    "tests/shutdown.test.ts",
    "tests/solari-health.test.ts",
    "tests/stream-deadline.test.ts",
    "tests/sweep.test.ts",
    "tests/vwp-refuse.test.ts"
  ],
  "ran": [
    "tests/fold-steer.test.ts",
    "tests/live-host-change.test.ts",
    "tests/login.test.ts"
  ],
  "notRan": [],
  "caught": [
    {
      "id": "me9d47a1cee3f",
      "file": "src/live-host-change.ts",
      "line": 112,
      "op": "eq-to-neq"
    },
    {
      "id": "mbb5cb18587a1",
      "file": "src/live-host-change.ts",
      "line": 112,
      "op": "or-to-and"
    }
  ],
  "missed": [
    {
      "id": "m4696f16d3c5d",
      "file": "src/live-host-change.ts",
      "line": 112,
      "op": "lt-to-le"
    }
  ],
  "other": [],
  "notRun": [],
  "goldenContract": [],
  "rest": 0,
  "full": "/var/folders/yq/wkr09jbx5qn_gtc9p7hrq4sw0000gn/T/grok-goal-5bd9307357c9/implementer/verify-out-1/verify.json"
}
```

## 2026-10-04 — MCP acceptance

MCP, twice, against the same golden fixture as the CLI. Both responses parsed to the same object as `node dist/cli.js golden check` on that fixture. Neither payload contains a home path.

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "golden.check",
  "summary": "Golden rows match.",
  "next": "Review the re-recorded rows.",
  "nextCall": null,
  "wording": [],
  "contract": [],
  "invariant": [],
  "updated": false
}
```

## 2026-10-04 — ledger acceptance

### Decisions

- A fix is a commit with a `Fixes-bug:` trailer, or a commit that changes both `src` and a test (`src/ledger/build.ts:133`). Rejected: hardcoding the 33 train ids.
- The src diff is reverted with `git apply --check` and no fuzz. A clean apply is written as a forward patch (`source=history`, apply it to put the bug back). Rejected: storing the experiment's reverse patches, and rejected: `patch -F3`.
- A diff that does not apply is `hand` in `ledger.json`. No patch is written for it. A file already in the output directory with `source=hand`, or with no `source=` line, is kept when that named direction applies (`src/ledger/build.ts:297`).
- A src diff over `--max-lines` (default 300) is `skipped`, including a `Fixes-bug:` commit. `43d15db` is 329 lines and skipped. Rejected: reverting a refactor that happens to touch a test.
- The git author name is not stored. `fixer` is `agent` when the commit has a `Made-by:` trailer, otherwise `human`.
- The run uses a throwaway worktree of `--commit` and removes it. Auspex stayed at `34896d3`.

### Unsure

- Detection is wider than the experiment's yardstick. This run found 39 clean reverts. The 13 train ids are all in that 39, and the other 26 were not in `mutants/train/`. Ten of those 26 are holdout commits that also reverse cleanly (`1cccf06`, `1cfec4a`, `4de516f`, `60c05a0`, `6a9852a`, `6d1ec22`, `983893c`, `9d3571d`, `b4525eb`, `c96a8a2`). The rest touched `src` and a test and are under 300 lines; the experiment did not call all of them bugs.
- 115 commits need a hand-made mutant. The 20 train conflicts are in that 115 (the 6 the experiment fuzzy-applied, and the 14 in `craft-mutants.py`). The other 95 are the same wide detection. I would not hand-write all 95.
- `fixer` does not name the person or the agent id. The `Made-by:` token is only used as a boolean.

### Shortcuts

- A ledger entry has no guard yet (`src/ledger/build.ts:20`). The guard is filled only after a kill run, and this step did not run the suite. The train set was already killed by `mutate run`.
- Matrix, golden, `verify-change`, MCP, and Python are not started.
- Sealed slice, findings log, stale decisions, and `Guard-Change:` are not built.

### Numbers

`npm test`: 17 pass, exit 0. Then `npm run build`, exit 0.

The 13 clean train patches, applied forward, produce the same `examples/auspex-ts/src` tree as `git apply -R` of `mutants/train/r-<id>.patch` on `b38e85f`. Compared on two throwaway worktrees, then removed. No mismatches.

Those 13 ids: `0129560`, `14ebd22`, `42d7059`, `46fde03`, `57ad8f2`, `61febaa`, `70a1078`, `840551b`, `b3b3975`, `b71dc78`, `d8fa568`, `f05408b`, `fcf67aa`. All `status: clean`.

The other 20 train ids are `status: hand`, reason `patch does not apply`: `0248893`, `0ecb8e6`, `473da1e`, `4907a9b`, `5174688`, `69792bf`, `7b85b59`, `992f512`, `9d824b9`, `a8e2a50`, `af5a0c2`, `b0deeed`, `c3b12c3`, `ed3498c`, `ef2f3a0`, `eff9dfa`, `f5f27ee`, `f979056`, `faea669`, `fb9a1d3`.

A second run wrote an identical `ledger.json` and identical stdout.

```bash
cd "/Users/aron/Documents/coding_projects/Probatio" && node dist/cli.js ledger build \
  --package "/Users/aron/Documents/coding_projects/auspex/examples/auspex-ts" \
  --repo "/Users/aron/Documents/coding_projects/auspex" \
  --commit b38e85f \
  --out /tmp/probatio-ledger-b38e85f \
  --max-lines 300
```

Exit 0. Stdout:

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "ledger.build",
  "summary": "39 fixes reverse cleanly. 115 need a hand-made mutant. 32 are over the line cap.",
  "next": "First fix that needs a hand-made mutant is d89e290 at examples/auspex-ts/src/check.ts:339.",
  "nextCall": null,
  "clean": 39,
  "hand": 115,
  "handmade": 0,
  "skipped": 32,
  "handNeeded": [
    { "id": "d89e290", "commit": "d89e290df90b5d45f00915612e7e9d6781456440", "file": "examples/auspex-ts/src/check.ts", "line": 339 },
    { "id": "b26bf9b", "commit": "b26bf9b4332867077c90dd1056b8123eb1a4fa01", "file": "examples/auspex-ts/src/check.ts", "line": 79 },
    { "id": "ef2f3a0", "commit": "ef2f3a09d09d20d6e4c301c5d98c9b0a78104c3f", "file": "examples/auspex-ts/src/page-actions.ts", "line": 40 },
    { "id": "f979056", "commit": "f979056d2114d5cf5cee2adaacbb94e232d1f2dc", "file": "examples/auspex-ts/src/page-actions.ts", "line": 208 },
    { "id": "1ff56b5", "commit": "1ff56b56d48c7ae42a648bc900feb1991d91d5a6", "file": "examples/auspex-ts/src/check.ts", "line": 74 },
    { "id": "536f226", "commit": "536f226e9585b805bf27eb4a24790b389aeb69dc", "file": "examples/auspex-ts/src/sandbox.ts", "line": 203 },
    { "id": "ec63d98", "commit": "ec63d98b73109a751d996f83227e279ea126d653", "file": "examples/auspex-ts/src/page-actions.ts", "line": 305 },
    { "id": "5097687", "commit": "5097687eec248a49036c4431fae56dc5441acb0e", "file": "examples/auspex-ts/src/check.ts", "line": 241 },
    { "id": "a8e2a50", "commit": "a8e2a502cebf9e0594000e86ffe507f9b3ca7c0f", "file": "examples/auspex-ts/src/check.ts", "line": 13 },
    { "id": "cea8f56", "commit": "cea8f566fdde52ace155a30f4947c832be059ea2", "file": "examples/auspex-ts/src/http-url.ts", "line": 1 }
  ],
  "rest": 105,
  "commit": "b38e85f4977fff152e0ea6e28e31634c557032ed",
  "full": "/tmp/probatio-ledger-b38e85f/ledger.json"
}
```

## 2026-10-04 — mutate acceptance

### Decisions

- Experiment patches are applied with `--direction reverse`. The generator writes forward patches and names that in the file (`direction=forward`, apply to introduce the bug). Rejected: rewriting the corpus into forward form.
- A named test is a kill only when that title fails two more times, alone, with concurrency 1. Rejected: counting the first failure under load.
- A file that finishes every named test and then does not exit is a kill when two reruns of that file, with no name pattern, time out the same way (`src/mutate/run.ts:243`, `src/mutate/fail-reporter.mjs:36`). Node reports this as `testTimeoutFailure` whose name is the file path, and the diagnostic `fail` count stays 0. Rejected: calling it flaky. Also rejected: confirming it with `--test-name-pattern`, because that skips the tests that left the handle open.
- A timeout of the whole suite is outcome `timeout`, and a failed build is `build-failed`. Neither is a kill (`src/mutate/run.ts:214`, `src/mutate/run.ts:219`).
- `--affected` defaults off. This acceptance ran the full suite. Rejected: selecting tests here, because a selection bug would change the kill rate.
- After each mutant the worktree is restored with `git checkout -- .` and `git clean -fd -e node_modules` (`src/mutate/patch.ts:88`). Diff labels strip both `a/` and `b/` (`src/mutate/patch.ts:51`). Rejected: restoring only the paths parsed from the diff. `git diff -R` swaps the labels, and the first full run left bugs in the worktree.
- Child processes drop `NODE_TEST_CONTEXT` and every `NODE_TEST_*` variable (`src/mutate/run.ts:333`). Rejected: inheriting them. Node then refuses to run the suite.
- The shuffle is an integer LCG (`src/mutate/ids.ts:10`). Rejected: copying the experiment's floating-point shuffle. Ids are a hash of file, span, and operator, so order does not change them.
- One generated point wholly inside a string, a comment, or template text refuses the batch (`src/mutate/generate.ts:51`). Rejected: skipping that point and writing the rest.
- No Auspex filenames are hardcoded as skips.
- Each run is a detached worktree with the package `node_modules` symlinked in. The Auspex checkout stayed at `34896d3` on `experiment/slim-tests`.
- The package is private `0.0.1`. No LICENSE file yet. Rejected: choosing a license without being asked.

### Unsure

- A named test that itself hits `--test-timeout` is still a `test:fail`, and it can count as a kill if that title fails both reruns. I did not re-score the earlier kills to see if any of them were only that. The file-level hang is the case this change covers.
- `failureId` falls back to the file basename when the reporter's path is not under the package path (`src/mutate/run.ts:568`). On macOS that is `/var` versus `/private/var`. `h-472fdd1` is stored as `job.test.ts::tests/job.test.ts` for that reason.
- The 51 results other than `h-472fdd1` were written by an earlier invocation of the same command with `--workers 2`, after the restore fix and before the file-timeout confirm. The invocation below used `--workers 1` and only re-ran `h-472fdd1` (204s). A new `--out` re-runs all 52. The same `--out` skips finished ids and refuses a directory whose `run.json` names another commit.
- The first full run, before `restoreTree`, is discarded. It said 51 killed and 1 errored, and it falsely killed `h-ba32203`, because a swapped diff label left the mutation in the worktree. Do not use that number.
- `h-ba32203` surviving matches the experiment at this commit. The connect test that would catch it was strengthened later, on the experiment branch, after the score.

### Shortcuts

- Ledger, matrix, golden, `verify-change`, and MCP are not started.
- Python is not started.
- `--affected` is implemented in `src/mutate/affected.ts` and was not used for this kill rate.
- The 5337 generated mutants were not run. The experiment's synthetic batches are not in the repo, and this shuffle will not reproduce their ids.
- No LICENSE file.

### Numbers

Unit tests after the file-timeout change: `npm test` in this repo, 15 pass, exit 0. Then `npm run build`, exit 0.

String-literal check, on a throwaway worktree of `b38e85f` (removed afterwards; Auspex stayed at `34896d3`):

```bash
git -C "/Users/aron/Documents/coding_projects/auspex" worktree add --detach /tmp/probatio-auspex-gen b38e85f
cd "/Users/aron/Documents/coding_projects/Probatio" && node dist/cli.js mutate generate \
  --package /tmp/probatio-auspex-gen/examples/auspex-ts \
  --out /tmp/probatio-auspex-generate \
  --src src
```

Exit 0. Stdout:

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "mutate.generate",
  "summary": "5337 mutants in 77 files. No string-literal mutant.",
  "next": "Run the batch.",
  "nextCall": {
    "argv": [
      "mutate",
      "run",
      "--package",
      "/tmp/probatio-auspex-gen/examples/auspex-ts",
      "--patches",
      "/tmp/probatio-auspex-generate/mutants",
      "--out",
      "/tmp/probatio-auspex-generate/runs"
    ]
  },
  "mutantCount": 5337,
  "stringLiteralMutants": 0,
  "mutants": [
    { "id": "mf03fd0fc7b63", "file": "src/agent-receipt.ts", "line": 46, "op": "negate-ternary" },
    { "id": "mfe572fb9b620", "file": "src/agent-receipt.ts", "line": 38, "op": "and-to-or" },
    { "id": "me528300d3ed9", "file": "src/agent-receipt.ts", "line": 23, "op": "or-to-and" },
    { "id": "m09e1bb4350ce", "file": "src/agent-receipt.ts", "line": 23, "op": "true-to-false" },
    { "id": "mcb6fb10d8ff1", "file": "src/agent-receipt.ts", "line": 24, "op": "false-to-true" },
    { "id": "me6348241c1cb", "file": "src/agent-receipt.ts", "line": 45, "op": "negate-condition" },
    { "id": "me56c887f9066", "file": "src/agent-receipt.ts", "line": 26, "op": "negate-ternary" },
    { "id": "m1d81ccf5b4f0", "file": "src/agent-receipt.ts", "line": 107, "op": "negate-condition" },
    { "id": "mb368c2f95508", "file": "src/agent-receipt.ts", "line": 39, "op": "drop-not" },
    { "id": "mbe23e6232d38", "file": "src/agent-receipt.ts", "line": 119, "op": "negate-condition" }
  ],
  "rest": 5327,
  "full": "/tmp/probatio-auspex-generate/mutants/mutants.json"
}
```

Kill check. This invocation resumed run `a9d6a3c2` and only re-ran `h-472fdd1`. Pass a new `--out` to run all 52 again.

```bash
cd "/Users/aron/Documents/coding_projects/Probatio" && env -u SOLARI_API_KEY -u AUSPEX_LIVE node dist/cli.js mutate run \
  --package "/Users/aron/Documents/coding_projects/auspex/examples/auspex-ts" \
  --repo "/Users/aron/Documents/coding_projects/auspex" \
  --commit b38e85f \
  --patches "/Users/aron/Documents/coding_projects/auspex/experiments/slim-tests/mutants/train" \
  --patches "/Users/aron/Documents/coding_projects/auspex/experiments/slim-tests/mutants/holdout" \
  --direction reverse \
  --out /tmp/probatio-auspex-b38e85f \
  --workers 1 --concurrency 3 --agent grok
```

Exit 0. Stdout:

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "mutate.run",
  "summary": "51 killed, 1 survived, 0 flaky, 0 timed out, 0 errored, of 52 finished.",
  "next": "First gap is h-ba32203 at examples/auspex-ts/src/connect.ts:431.",
  "nextCall": null,
  "killed": 51,
  "survived": 1,
  "flaky": 0,
  "timeouts": 0,
  "errors": 0,
  "gaps": [
    {
      "id": "h-ba32203",
      "file": "examples/auspex-ts/src/connect.ts",
      "line": 431
    }
  ],
  "rest": 0,
  "flakyIds": [],
  "budgetHit": false,
  "commit": "b38e85f4977fff152e0ea6e28e31634c557032ed",
  "runId": "a9d6a3c2",
  "full": "/tmp/probatio-auspex-b38e85f/results"
}
```

`h-472fdd1` is `killed`, direction `reverse`, file `examples/auspex-ts/src/job-store.ts:209`, killed by `job.test.ts::tests/job.test.ts`, 204.191 seconds. The only survivor is `h-ba32203`.
