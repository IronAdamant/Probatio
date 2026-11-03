# Campaign narrowing plan

This is the next change after `docs/agent-run-plan.md`. `plan.md` stays the record of the Rust rewrite. `scripts.test` and the files line map are already in the binary. This file is not the contract.

`AGENTS.md` keeps the scoring rules. This plan does not restate them and does not weaken them.

Thirty stays `LARGE_BATCH`. It is still the edge of an unmapped batch and the cap on selected test names. It is not a memory budget. Workers stay at 1. `schemaVersion` stays 2. A new field may appear. A field that already exists must not change meaning.

`env`, `testLine`, and `passLine` stay unread.

## 1. Read `scripts.test` from `package.json`

Discovery order stays `go.mod`, `Cargo.toml`, `pom.xml`, then `package.json`, then `pytest.ini` or `pyproject.toml`.

The `package.json` branch changes.

- `--suite-command` still wins. A `suiteCommand` string in `.probatio.json` still wins when the flag is absent. Neither path reads `package.json`.
- When discovery reaches `package.json`, parse it. A non-empty string `scripts.test` is the suite command, used as written. `mocha`, `npm test`, and `node --test test/*.js` are all that string.
- Valid JSON with no `scripts.test`, or a `scripts.test` that is empty or not a string, stays `node --test`.
- A `package.json` that exists and is not valid JSON is a refusal. Discovery does not substitute `node --test`. The envelope has no `suiteCommand`. `next` says discovery was not used.
- No other `scripts` entry is read. The walk stays at the package root.

## 2. A line map that names tests

A non-empty `--line-map` file still allows a batch of more than 30 mutants. Two shapes are allowed.

A permission slip is any non-empty file whose JSON is not the `files` object below. `{"lines":[1]}` stays a permission slip. `serial: true` still writes that slip and still does not choose tests. `next` keeps the sentence that the line map allowed the batch and did not choose tests.

A test map is a JSON object with a `files` object. Each key is the mutant `file` path, with `\` normalized to `/`. Each value is an array of ranges:

```json
{
  "files": {
    "src/A.java": [
      {"start": 3, "end": 5, "tests": ["FooTest", "BarTest"]}
    ]
  }
}
```

`start` and `end` are 1-based line numbers, inclusive. `tests` is an array of strings. Any other shape is a permission slip, including valid JSON that has no `files` object.

The line of a mutant is the count of newlines before its `start` byte in the package source, plus one. The source is the unmutated file. Ranges that cover that line contribute their names. Names stay in file order, then range order, with duplicates removed.

Name order for one mutant:

1. The mutant's own `tests` array when it is non-empty.
2. The names from a test map for that line.
3. The `--tests` file, and only when the line map is a permission slip or absent.

A test map covers the batch. The `--tests` file is not added on top of it.

A mutant with no names under a test map is `no coverage`. The run writes `results/<id>.json` and does not spawn the suite. That result counts as started, so the next chunk moves on. It is not a kill and not a survivor.

A test map that names tests for a mutant, and a suite command with neither `{tests}` nor `{tests,}`, stops before that mutant. `next` tells the caller to put one of those tokens in the command. A green baseline already written for a different command string is not reused. `baseline.json` records the template that produced it, and a later call with another template runs the baseline again.

The baseline name list is the `--tests` file when that file is non-empty. Otherwise it is every name in the test map, in map order, without the 30-name cap. The per-mutant selection stays capped at `--max-selected` (default 30). The baseline is how the suite command looks with that list substituted. A command with a token and an empty baseline list, and no test map, is a refusal before any mutant: the command asked for names and none were given.

An unsafe selected name still refuses the call, names that name, and starts no mutant with that command. The same check applies to the baseline list.

`{tests}` and `{tests,}` stay as they are. The map only supplies the names those tokens receive.

## 3. Record the command that ran

Each `results/<id>.json` written after a spawn gains a `command` string. The string is the shell line `sh -c` received, after token substitution. A `no coverage` result that did not spawn has no `command` field.

`baseline.json` gains `command` set to the suite template, the string before substitution. Resume compares that string to the template of the new call.

`schemaVersion` stays 2. Tally keeps reading `outcome`. It does not need `command`.

## 4. Say this in the README, the skill, and the tool text

`README.md` "The run stops that came with the rewrite" gains the chunk rules already in `AGENTS.md`: one mapped call scores at most 30 mutants that have no result, `nextCall` continues in that same out dir, and `--confirm-count` scores the rest only when it matches the waiting count. The same section names `suiteCommand`, `{tests}`, `{tests,}`, and the result `command` field.

A short "Narrowing a run" section shows the test-map shape and four commands an agent can copy:

- `mvn -B test -Dtest={tests,}`
- `cargo test {tests}`
- `python3 -m pytest {tests}`
- `node --test {tests}`

The project skill and `~/.grok/skills/probatio/SKILL.md` say the same things: `serial` still writes the permission slip and does not choose tests; a `files` map does; `package.json` `scripts.test` is the discovered Node command when it is a non-empty string. The `mutate_run` tool text matches that. `lineMap` is no longer described as a file whose contents are never read.

`AGENTS.md` gains one sentence: a `files` map names tests, and any other non-empty line map only allows the batch.

## Tests

Drive the shipped binary in `tests/cli.rs`. Parsing the map can stay a unit test next to `render_suite`.

- A `package.json` whose `scripts.test` is `echo from-script` yields that `suiteCommand`. A `package.json` without `scripts.test` stays `node --test`. The flag and `.probatio.json` still win. Invalid `package.json` refuses and does not mention `node --test`.
- `{"lines":[1]}` with 31 mutants still scores 30, returns `nextCall`, and does not choose tests.
- A `files` map and `echo {tests}` runs a command that contains the names for that line. The result file's `command` is that rendered line. A second mutant outside every range is `no coverage`, and the log does not gain a line for it.
- The same map with `echo ok` stops before the mutant and says the command has no `{tests}` token.
- A mapped name `a'b` refuses and writes no result file.
- `schemaVersion` on these envelopes is 2.

## Not in this plan

- Reading `env`, `testLine`, or `passLine`.
- Importing JaCoCo, lcov, coverage.py, or cargo-llvm-cov. The `files` map is the file this binary reads. Something else can write it.
- Raising `LARGE_BATCH`, or adding workers.
- A second discovery walk below the package root.
- Another four-root sample rerun. The shipped-binary tests are the proof. Express is the example the discovery change is aimed at, and a red baseline after `scripts.test` is still a red suite.
- Publishing a release.
