# Probatio

The user decided to replace the TypeScript program with a Rust rewrite. That rewrite is complete. This repository ships one program, the `probatio` binary, and the crate depends on Rust `std` only.

Probatio scores a test suite by the bugs that suite already catches. A kill is an assertion the suite had. The kill count is not a merge gate, and it is not proof that a change is safe.

Build it from this directory:

```bash
cargo build --release
./target/release/probatio schema
```

`cargo test` runs the suite that locks the decision rules, the process-group deadline, and the command envelopes.

## What a command prints

Every command prints one JSON object and exits 0 only when `ok` is true. The object always has `schemaVersion`, `ok`, `summary`, `next`, and `nextCall`. `schemaVersion` is 2. Stdout is flushed before the process exits, including when the object is large and the reader is a pipe.

The default stdout is that short object. Kill lists and the full not-started id list are written under the out directory, not required on stdout.

```bash
probatio mutate generate --package . --out .probatio/generate
probatio mutate run --package . --patches .probatio/generate --out .probatio/runs --suite-command 'cargo test'
probatio mutate tally --out .probatio/runs
```

`mutate generate` walks the package and writes `mutants.json`. Operators are text replacements for conditions, comparisons, and booleans. Strings and comments are left alone. JavaScript and TypeScript use the same operators as the other languages. Ids come from the file, the offset, the operator, and the replacement.

`mutate run` needs `--package`, `--patches`, and `--out`. Pass `--suite-command` for the suite you already trust. With no command, a `go.mod`, `Cargo.toml`, `pom.xml`, `package.json`, or pytest marker only names the command. The binary does not fetch a toolchain.

## What a result means

- **killed**: a test the suite already had failed an assertion on the mutant.
- **survived**: a test ran the change and did not notice. That is a gap.
- **no coverage**: nothing executed that line. That is not a pass and not a gap.
- **did not build** (`unviable`): the mutant failed to compile or link. That is not a kill and not a gap.
- **timed out**: the suite clock ran out. That is not a kill and not a gap.
- A red baseline, or a baseline with no test report, stops before the first mutant. That run has no kill count.

`mutate tally` reads `results/*.json`. `deletedTests` is 0. `noKillsYet` is not a list of tests to remove. Tally does not delete a file.

## The run stops that came with the rewrite

The suite runs in its own process group. When the deadline hits, the binary kills that group, including a grandchild that ignores `SIGTERM`. The recorded duration stays within the cap plus a short grace.

One mutant selects at most 30 tests unless `--max-selected` raises the cap. The mutant result says when the cap applied (`selectionCapped`, `selectedCount`).

More than 30 pending mutants and no line map stop the run before the first mutant. A baseline under five seconds does not waive that stop. Pass `--line-map` with a non-empty map file when you have one. The ids that were not started go to `not-started.txt`.

On resume, an existing green `baseline.json` is kept, a mutant that already has `results/<id>.json` is not run again, and a leftover `probatio-<run>-*` worktree for that out dir is removed before a new one is added. The checkout you started in is left as it was. Workers stay at 1.

## Other commands

These all return the same envelope: `mutate sealed`, `ledger build`, `ledger check`, `matrix report`, `golden check`, `golden record`, `golden compare`, `gap fix`, `gap revert`, `guard check`, `findings add`, `status`, `seal`, `queue seed`, `queue claim`, `queue reap`, `check-kill`, `verify-change`, and `schema`.

`schema` with no argument lists them. `schema mutate.generate` names the contract file under [schemas/](schemas/) when that file is present. `golden compare` does not delete a golden. A red `ledger check` is not re-recorded away. `mutate sealed` does not restore a hidden test. `gap fix` does not write the missing test for you.

`git`, Maven, Go, Cargo, Node, Python, and the other subject tools stay outside this binary. Small sample suites still live in [examples/](examples/). Older notes in `docs/` and `CHANGELOG.md` describe the TypeScript program this rewrite replaced.
