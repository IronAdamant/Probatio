# Probatio rewrite in Rust, std only

The user decided on this rewrite, and it is complete. The shipped program is the Rust binary. The notes below are the design that led there. They are not a second contract.

The binary links Rust `std` and nothing else. The speed comes from process control, not from the language. A jsoup mutant that selected about 1,100 tests ran for 84 minutes inside Maven while the suite timeout was 60 seconds. Mutants with no covered line finished immediately. The orchestrator was already fast.

## Zero-dep

`Cargo.toml` has an empty `[dependencies]` and an empty `[dev-dependencies]`. No `build.rs` that fetches, no vendored C, no `serde`, `clap`, `tokio`, `regex`, `syn`, or `libc` crate.

Unix process groups are available on stable `std` (`CommandExt::process_group`). Threads are `std::thread`. Tests are `cargo test`.

JSON read and write is our code, limited to the envelope, the out-dir files, and the coverage files we already know. It is not a general JSON library.

`git`, `mvn`, `go`, `cargo`, `node`, `python`, `pytest`, `dotnet`, `cobc`, and `clang` stay outside the binary. Zero-dep means no library dependencies. It does not mean Probatio reimplements those tools.

The `typescript` package is dropped. JS and TS mutants use the same text operators as Java, Go, Rust, Python, C, C#, Swift, COBOL, and the asm kinds. Mutant ids for JS and TS will change. The other languages can follow `src/mutate/text-operators.ts` closely.

## What stays

- Command names and flags stay: `mutate generate|run|sealed|tally`, `ledger build|check`, `golden check|record|compare`, `gap`, `guard`, `findings`, `status`, `seal`, `queue`, `check-kill`, `verify-change`, `schema`, `mcp`.
- `schemaVersion` stays 2 until a top-level field changes meaning. Required envelope fields stay `schemaVersion`, `ok`, `summary`, `next`, `nextCall`.
- A red baseline or a run with no test report is not given a kill count.
- A mutant that did not build is `unviable`. It is not a kill and not a gap.
- A timeout is the baseline clock. It is not a kill and not a gap.
- `mutate tally` keeps `deletedTests` at 0.
- A kill is an assertion failure. It is not proof a change is safe.
- Home paths are scrubbed to `~` except inside `nextCall`, which is executed.
- The three sample roots stay the regression set: `Sample_Probatio_Beyond`, `Sample_Probatio_Separate`, `lang_cobol_sample_projects_for_testing`. Their git porcelain must match a fresh before-snapshot. Nothing is downloaded. No sample file is edited to make a suite pass.

## What changes on purpose

These are in the first `mutate run`, not a later pass.

1. The suite timeout kills the process group. Grandchildren die with the child. The result records the seconds actually taken. A 60-second cap cannot take 84 minutes.
2. One mutant selects at most 30 tests. The per-mutant result records `selectedCount` and `selectionCapped`. A capped run says so in `next`. The cap is a flag, `--max-selected`, default 30.
3. A large unmapped batch stops before the first mutant when more than 30 mutants are pending and there is no usable line map. The baseline duration does not waive that stop. Express had no map and a baseline under 5 seconds, and it ran 474 whole-suite mutants (165 flaky, 76 timed out). Under this rule that subject is a refusal.
4. Stdout is the short envelope: summary, next, nextCall, and the counts. Kill lists, gap lists, and the full `notStarted` id list stay in the out directory. The process flushes and closes stdout before it exits. The current CLI writes pretty JSON and then calls `process.exit`, which cuts a pipe at 64 KB. The jsoup envelope on disk is 52 MB. That shape is not the default.
5. Resume removes leftover `probatio-*` worktrees for that run id before it adds new ones. A killed run left `/probatio-3a3fea10-0` registered, and the next `git worktree add` failed until it was removed by hand.
6. Workers stay at 1 until a test proves a hung grandchild dies at the deadline. Raising workers before that multiplies stuck Maven processes.

The existing slow-unmapped rule also stays: baseline at least 5 seconds, no line map, and more than 30 pending, stop. Rule 3 above is wider. Both are refusals, not kill counts.

## Layout

New crate at `rust/`. The TypeScript tree stays until the Rust binary meets the decision table below. Do not delete `src/` in the first cut.

```
rust/
  Cargo.toml          # package probatio, no dependencies
  src/
    main.rs           # argv, dispatch, exit after flush
    json.rs           # subset parser and writer
    contract.rs       # envelope, scrub, schema version
    args.rs           # flag table copied from the command list
    process.rs        # spawn, process group, deadline, kill
    git.rs            # git CLI only
    mutate/
      operators.rs    # text operators, core and wide
      generate.rs
      suites.rs
      coverage.rs     # go, jacoco, rust, csharp maps we already write
      run.rs
      sealed.rs
      tally.rs
    ledger/
    golden/
    queue.rs
    verify.rs
```

`schemas/*.json` stay the contract source. A Rust test reads each file with the subset parser and checks that the command name and the flag table agree. A full JSON Schema validator is not ported.

## Phases

### 0. Envelope

Crate, subset JSON, scrub, help, and `schema`. A test pipes a multi-megabyte write and reads back one complete JSON object. `cargo metadata` shows no dependencies.

### 1. Supervisor

`process.rs` with a fixture child that ignores `SIGTERM`, sleeps, and spawns a grandchild. The deadline kills the group. The recorded seconds are the wall time, and they are under the cap plus a short grace. This is the Maven bug, tested without Maven.

### 2. Generate

Port the text operators. `mutate generate` for Java, Go, Rust, Python, JS, TS, C, C#, COBOL, and asm. Compare points on the fixtures already in `tests/`, not on the 3,350 jsoup patches. JS and TS counts may differ from the TypeScript-compiler mutants. Record the delta. Do not force the old ids.

### 3. Run, one suite at a time

Order: `node`, `maven`, `go`, `cargo`, `pytest` and `unittest`, then `dotnet`, `make`, `mocha`, C, C++, Swift, COBOL, asm. Each kind gets discovery, the baseline, the line map when that kind has one, the selection cap, the unmapped stop, resume from `results/<id>.json`, and worktree cleanup.

A red baseline writes a refusal and no kill count. A mutant that fails to compile is `unviable`.

### 4. The rest of the CLI

`ledger`, `golden`, `sealed`, `tally`, `verify-change`, `queue`, `gap`, `guard`, `findings`, `status`, `matrix`, `check-kill`. Same refusal rules as today. `mcp` is last: a stdio loop using the subset parser. If a phase-3 binary already scores the samples, MCP can wait.

### 5. Sample rescore

Run the Rust binary on the three roots. No minute cap. Workers 1. Same offline rules as the last run. Fresh out dirs under each root's `probatio-findings-rs/`, not inside a subject checkout.

Decision table the run must meet:

- Every subject that was refused for no tests, no mutants, an unknown suite, a red baseline, a missing tool, a commit that does not resolve, or a download, is refused again for that reason.
- Express is refused on the unmapped large-batch stop.
- `commons-csv`, `commons-csv-271`, `semver`, `go`, `rust`, and `java` finish a campaign: `ok` true, `notStarted` empty, no budget stop.
- Each of those six records a suite timeout that holds. No mutant's seconds exceed the cap plus grace.
- jsoup finishes in hours, not a day, because selections are capped and hung Maven groups die.
- Counts will move where the operator set or the cap changed. The report lists the old and new summary lines. A matching kill count is not required and is not proof.
- Git porcelain of every sample repo matches the before snapshot.

## Acceptance for the crate itself

- `rg` over `rust/` finds no `crates.io` dependency and no `extern crate` beyond `std`.
- The hung-grandchild test fails if the grace is exceeded.
- The pipe test fails if stdout is truncated.
- `mutate tally` on a finished out dir reports `deletedTests` 0.
- A fixture with a failing baseline produces a refusal and no kill count.

## Non-goals

- Do not reimplement Maven, the TypeScript compiler, git, or a JSON Schema validator.
- Do not add a crate to make JSON or CLI parsing shorter.
- Do not delete tests in a subject, and do not add a prune command.
- Do not publish the Rust binary over the current npm package until phase 5 holds.
- Do not edit the sample clones. Do not initialize Defects4J, BugsCpp, or Docker.
- Windows process trees are out of scope. The target is macOS and Linux.

## Risks

The suite adapters are the work. The language of the CLI is the small part. A hand-rolled JSON parser will be wrong on inputs it was not built for, so it only has to accept the files we write and the coverage formats we already parse. Text operators will not reproduce the TypeScript compiler's mutant set for semver and Express. Express is a refusal under the new stop anyway. Forcing the old jsoup kill count would hide the selection cap.

## First cut after a go-ahead

1. Add `rust/Cargo.toml` with no dependencies and the phase-0 envelope test.
2. Add the supervisor and the hung-grandchild test.
3. Port text operators and generate for Java only, then compare a small fixture.
4. Stop and show that evidence before `mutate run` and before any sample rescore.
