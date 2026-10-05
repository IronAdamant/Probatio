# Probatio

Probatio (Latin: a testing, a proof) is a free, open-source testing toolkit built for AI agents
(Claude, Grok, ChatGPT, others), working alone or as swarms. Every claim it makes is checkable;
it remembers what was fixed so agents with small context do not redo or undo work; and it finds
the bugs a test suite would miss instead of asking anyone to read more tests.

Status (2026-10-05): `mutate run` discovers the project suite and scores it. Python, JavaScript, and TypeScript narrow each mutant with a line map collected on the baseline. C does the same when the binary was built with LLVM coverage. A suite with no line map, a baseline of 5 seconds or more, and more than 30 mutants stops before the first mutant. `verify-change` mutates the changed lines and uses that same map. `mutate tally` reads a finished run, names tests that killed nothing, and does not delete a test. The MCP server speaks one JSON object per line and returns the same JSON as the CLI.

Read in this order:

1. `HANDOFF.md` — why, the evidence from Auspex, the design, and the lessons that cost time.
2. `FOR-GROK.md` — the builder brief: build order with acceptance tests, the agent contract,
   memory for agents, anti-gaming, swarm protocol, traps.
3. `seed/` — code copied from the Auspex experiment to start from (Auspex paths inside).

Roles: Grok builds most of it; Claude refines and finishes. Grok keeps `NOTES-FOR-CLAUDE.md` here
(decisions, guesses, shortcuts with file:line, acceptance output actually run).

## mutate

One JSON object on stdout. `schemaVersion`, `ok`, `summary`, `next`, `nextCall`. Exit 0 only when `ok` is true. Human text is `--human`. A run checks the suite out in a throwaway worktree, so the checkout you are in stays as it is.

```bash
npx probatio mutate generate --package . --out .probatio/generate
npx probatio mutate run --package . --patches .probatio/generate/mutants --out .probatio/runs
npx probatio mutate tally --out .probatio/runs
```

`generate` writes operator mutants (conditions, `&&`/`||`, `===`/`!==`, boundaries, booleans, a dropped `!`). The same class is written for COBOL (`.cob`, `.cbl`), Rust, C, C++, Java, Go, Python, JavaScript, and C#. Strings and comments are not mutated. A COBOL `*>` comment and a fixed-format line whose column 7 is `*` or `/` are comments. Each patch is forward: apply it to introduce the bug, and the file says so. When mutants were written, `next` states the count, the suite command if one was discovered, and whether the first run collects a line map.

`run` discovers the suite: `run_tests.sh`, Cargo, Go, Swift, Maven, dotnet, pytest, unittest, node:test, Mocha, then `make test` when COBOL tests sit under that Makefile. An unknown layout stops and asks for `--suite-command`. It does not compile one file and call that the suite. `make test` that would curl or wget a missing file stops, and nothing is fetched. A worktree that has no `node_modules` uses the main checkout's. Omitting `--build` runs no build. Pass `--build` with a command when the suite needs one first.

Python, JavaScript, and TypeScript collect a line map on the baseline. C collects one when LLVM coverage was instrumented. Go and Maven/Java collect file, line, and the test names that hit that line. A later mutant runs only those tests (`go test -run`, Maven `-Dtest`). A line no test executed is `no coverage`, and the suite is not started. Rust and C# can take a test name and do not collect a line. A baseline of 5 seconds or more, with no line map and more than 30 mutants still pending, stops before the first mutant. `next` names that baseline and tells you to narrow `--src` or pass a smaller patch directory. A one-file C, C++, Java, COBOL, or assembly launcher is not a suite discovery returns.

The mutant timeout is the baseline duration times 5, and at least 20 seconds, capped by `--suite-timeout-ms`. A timeout is not a kill. `--budget-ms` stops mutant work after the baseline. The first mutant still runs, except for that no-line-map stop. A kill is the test that failed, or a compiler token when the mutant did not build. Confirm is on by default and reruns the failing names. Pytest names that `-k` cannot express are passed as node ids. A usage error, including pytest exit 4, is not a kill. `verify-change` does not confirm a second time. Add `--affected` to limit the file list to tests that can see the change. The default, once a line map exists, runs the tests on the changed line.

`tally` reads that run directory. The summary leads with the no-coverage count, then the survivors. It names tests that killed a mutant and tests that killed nothing. A gap is a survivor. An unseen line is not a gap and not a pass. A timeout is neither a kill nor a gap. `deletedTests` is 0. It does not delete a file.

## ledger

```bash
npx probatio ledger build --package . --commit HEAD --out .probatio/ledger
```

A fix commit has a `Fixes-bug:` trailer, or it changes both `src` and a test. `ledger build` reverts that commit's src diff onto the tree you name. A diff that applies is written as a forward patch (`source=history`): apply it to put the bug back. A diff that does not apply is reported, and left for a hand-made patch in the same directory (`source=hand`, `fix=<commit>`). A rebuild keeps those hand-made files. Nothing is fuzzy-applied. A src diff over `--max-lines` (default 300) is skipped. `--max-commits N` reads only the newest N non-merge commits and says when older history was not scanned. The default reads the whole history. A tree is checked out only when a fix inside that cap has to be applied.

## matrix

```bash
npx probatio matrix report --scores round2-union-scores.tsv --direct round2-direct-scores.txt --patches mutants/holdout --patches mutants/train --out .probatio/matrix
```

Reads those recorded tables. It does not rerun the mutants. The headline is the gap list: `file:line`, the top 10, how many are left, and the path to the full JSON. A test that killed nothing can be named. The file is not deleted.

## golden

```bash
npx probatio golden check --recorded table.json --actual now.json --update wording
```

`next` and `nextLead` are wording. `ok`, `reason`, `status`, `hostChanged`, and `nextCall` are the contract. `--update wording` re-records a changed `next` or `nextLead` and leaves the contract fields alone. A file whose case names are the top-level keys is written back in that shape. A changed `ok` or `reason` also needs `Golden-Change: <row>: <why>` in `--message`. A home path, or `ok` without a reason, fails and nothing is written.

## status

```bash
npx probatio status --state .probatio --page PROBATIO.md
```

Rewrites `PROBATIO.md` from the same JSON: open gaps, recent fixes and their guards, protected guards, decisions. A sealed id is left out of the page, the JSON, and `queue seed`. If visible kills rise and sealed kills do not, the summary is `these tests fit the yardstick, not the code.`

## verify-change

```bash
npx probatio verify-change --package . --base HEAD~1 --out .probatio/verify
```

Lists the tests that can see the diff, runs mutants on the changed lines, and reports caught, missed, mutants that were not run, lines with no coverage, and any golden contract change. The mutants are the same operator class `mutate generate` would write, including a non-TypeScript diff. A string or a comment is not a mutant. `ran` is still the direct-importer list. The tests that execute are chosen by the line map, the same way `mutate run` chooses them, including when that importer list is empty. No discovered suite leaves those mutants in `notRun` and does not start a run. The suite timeout is 10 minutes. `--max-tests` only shortens the reported importer list. `--max-mutants` (default 4) caps how many changed lines are run.

## queue

An item is `.probatio/queue/<id>.json`. `queue claim` renames it to `claimed/<agent>-<id>.json` with a lease. `queue reap` moves an expired lease back. `check-kill <id>` runs the tests that directly import the mutated file and confirms the killing test again. It is done only when that mutant dies and that suite is still green. `verify-change` is the edit-sized run: it uses the line map and does not confirm a second time.

The MCP server speaks stdio JSON-RPC. Its one tool runs the CLI and returns that command's JSON.
