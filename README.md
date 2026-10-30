# Probatio

Probatio asks one question. If this code were a little bit wrong, would the tests you already have notice?

It keeps the test command the project already uses. It makes one small change, runs that command, and writes down whether a test failed. It does not write a new test, and it does not take over how the project runs its suite.

A catch means a test failed on that small change. The suite saw it. That is news about the tests. It does not say the original code is ready to ship, and it is not a gate on a merge.

If the tests are already failing, or they never print a report, Probatio stops before the first change. There is no score until the suite is green.

Hand the project to an agent when you want a run. The instructions that agent follows are in [AGENTS.md](AGENTS.md). The rest of this page is the detail.

## Detail

### The program

This repository ships one program, the `probatio` binary. The crate uses the Rust standard library and nothing else. `cargo build` writes the binary. `cargo test` runs the suite that locks these rules.

```bash
cargo build --release
./target/release/probatio schema
```

An earlier TypeScript program lived here, and that rewrite is finished. Notes in `docs/` and `CHANGELOG.md` still describe the TypeScript program. The binary, the files in [schemas/](schemas/), and [AGENTS.md](AGENTS.md) are the current contract. Small samples live in [examples/](examples/).

An agent talks to the binary over stdin with `probatio mcp`, or it calls the same commands itself.

### What a command prints

Every command prints one JSON object and exits 0 only when `ok` is true. The object has `schemaVersion`, `ok`, `command`, `summary`, `next`, and `nextCall`. `schemaVersion` is 2. Read `summary` and `next`. Run `nextCall` as printed. It continues in the out directory it names.

Kill lists and the full list of mutants that were not started are files under the out directory. They are not required on stdout. A home directory that appears in `summary` or `next` is written as `~`. The argv inside `nextCall` keeps its real paths.

```bash
probatio mutate generate --package . --out .probatio/generate
probatio mutate run --package . --patches .probatio/generate --out .probatio/runs --suite-command 'cargo test'
probatio mutate tally --out .probatio/runs
```

### Generate, then run, then tally

`mutate generate` walks the package and writes `mutants.json`. It does not run the suite. A mutant is a text replacement in a condition, a comparison, or a boolean. Strings and comments are left alone. The id comes from the file, the offset, the operator, and the replacement. The walk skips directories named `target`, `node_modules`, and `dist`, and anything whose name starts with `.`.

`mutate run` needs `--package`, `--patches`, and `--out`. It runs the suite once on the unchanged code. That is the baseline. Then it scores mutants that do not already have a result file.

`mutate tally` reads `results/*.json`. `deletedTests` is 0. `noKillsYet` lists names that saw no kill in this batch. It is not a list of tests to delete. Tally does not delete a file.

### Where the suite command comes from

The first match wins:

1. `--suite-command`
2. A non-empty `suiteCommand` string in `.probatio.json`
3. A marker file at the package root

Any other key in `.probatio.json` is ignored. If that file is not valid JSON, the run stops, and discovery is not used. The same is true of a `package.json` that is not valid JSON.

Discovery does not launch the tool. It only names the command, from the package root:

| Marker | Command |
| --- | --- |
| `go.mod` | `go test ./...` |
| `Cargo.toml` | `cargo test` |
| `pom.xml` | `mvn -B test` |
| `package.json` with a non-empty `scripts.test` string | that string, as written |
| any other `package.json` | `node --test` |
| `pytest.ini` or `pyproject.toml` | `python3 -m pytest` |

`Cargo.toml` is checked before `package.json`, so a package with both uses `cargo test`. If none of these files is present, the run stops and asks for `--suite-command`.

`{tests}` puts each selected name in single quotes. `{tests,}` joins the names with commas. A name may contain ASCII letters, digits, and `_` `-` `.` `/` `:`. Anything else is refused, and so is an empty name. The `suiteCommand` field on the envelope is the template, before the names are filled in. `baseline.json` stores that same template. A result written after the suite started stores the rendered line in `command`. A no-coverage result that never started the suite has no `command` field.

When the package has `node_modules/.bin`, that directory is placed on the suite process `PATH`. That includes a mutant run from a git worktree or a copy. Both skip `node_modules` when they are created, and both get a symlink to the package `node_modules` when they do not already have one. The command string itself is not rewritten. A shell exit 127 whose text says `command not found` means the tool is missing on the baseline, and it is an error on a mutant. The binary does not download a toolchain.

### What a result means

- **killed**: a test the suite already had failed an assertion.
- **survived**: a test ran the change and the suite did not notice. That is a gap.
- **no coverage**: nothing executed that line. Leave it. There is no evidence either way.
- **unviable**: the mutant failed to compile or link. The summary text says "did not build". A Node test file that no longer links counts here.
- **timeout**: the suite clock ran out. The clock is the baseline's clock.
- **error**: the suite process failed in some other way. Exit 127 with `command not found` is this.

A killed mutant is an assertion failure. Golden checks and contract checks are what say the expected result is still the expected result.

A red baseline, a baseline with no test report, a baseline timeout, or a missing tool stops before the first mutant. That envelope has no kill count.

### When a run stops

The suite runs in its own process group. When the deadline hits, the binary kills that group, including a grandchild that ignores `SIGTERM`. The default deadline is 120000 milliseconds. A Maven suite needs 600000 or more.

Two limits both use the number 30. They are different limits.

**Mutants in one call.** With no line map, more than 30 mutants waiting stops the run before the first mutant. A baseline under five seconds does not waive that stop. The ids go to `not-started.txt`. With a line map, one call still scores at most 30 mutants that have no result yet. `nextCall` continues in the same out directory. `--confirm-count` scores every mutant still waiting only when the number you pass equals that waiting count. The resume argv leaves `--confirm-count` off, so the next call chunks again.

**Names recorded for one mutant.** One mutant records at most 30 test names unless `--max-selected` raises the cap. The result says when the cap applied (`selectionCapped`, `selectedCount`). A line map that chooses tests still feeds every one of its names into the baseline command. The cap applies per mutant, after that.

Workers stay at 1. `--workers` above 1 is accepted, and the run still scores one mutant at a time. The `next` line says "Workers stay at 1."

A green `baseline.json` is kept when its command template matches the new call. A different template runs the baseline again. A mutant that already has `results/<id>.json` is not run again. The checkout you started in is left as it was.

### Naming tests with a line map

A line map whose top-level value is a `files` object names the tests for a covered line:

```json
{
  "files": {
    "src/A.java": [
      {"start": 3, "end": 5, "tests": ["FooTest", "BarTest"]}
    ]
  }
}
```

`start` and `end` are inclusive, counting from line 1 of the unmutated file.

`{"lines":[1]}` is a permission slip. So is any other non-empty file that is not a `files` object, including a file that is not JSON. A permission slip allows a large batch. It does not choose tests. On the MCP tool, `serial: true` writes that slip at `<out>/lines.json` when you did not pass a map.

An empty `{"files":{}}` does choose tests, and it names none of them. Every mutant in that run is no coverage, and the suite is not started for them.

When the map names at least one mutant, the suite command needs `{tests}` or `{tests,}`. Without a token, the run refuses after the green baseline and before it writes a result. These are the usual shapes:

```bash
mvn -B test -Dtest={tests,}
cargo test {tests}
python3 -m pytest {tests}
node --test {tests}
```

A command that already contains a token, and that has no names, and whose map does not choose tests, refuses before the baseline.

### The other commands

These return the same envelope: `mutate sealed`, `ledger build`, `ledger check`, `matrix report`, `golden check`, `golden record`, `golden compare`, `gap fix`, `gap revert`, `guard check`, `findings add`, `status`, `seal`, `queue seed`, `queue claim`, `queue reap`, `check-kill`, `verify-change`, and `schema`. `help` does too.

`schema` with no argument lists them. `schema mutate.generate` names the contract file under [schemas/](schemas/) when that file is present. Read that file for flags. An example is not the contract.

`golden compare` does not delete a golden. A red `ledger check` is not re-recorded away. `mutate sealed` does not restore a hidden test. `gap fix` does not write the missing test.

`git`, Maven, Go, Cargo, Node, and Python stay outside this binary.
