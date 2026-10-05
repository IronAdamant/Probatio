# Probatio

Probatio (Latin: a testing, a proof) is a free, open-source testing toolkit built for AI agents
(Claude, Grok, ChatGPT, others), working alone or as swarms. Every claim it makes is checkable;
it remembers what was fixed so agents with small context do not redo or undo work; and it finds
the bugs a test suite would miss instead of asking anyone to read more tests.

Status (2026-10-04): `mutate` passes the Auspex check at b38e85f (51 killed, 1 survived, the known miss). `ledger build` rebuilds the 13 cleanly reverting train fixes and reports the other 20 as needing a hand-made mutant. `matrix report` reproduces the round-2 table from the recorded scores, with slim C at 10/18 and the gap list as the headline. `golden check` splits contract from wording. `verify-change` on one Auspex function reported 2 caught, 1 missed, rest 0, in 5.34s and 5.38s. The MCP server returns the same JSON as the CLI.

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
npx probatio mutate run --package . --patches .probatio/generate/mutants --out .probatio/runs --no-build
```

`generate` writes operator mutants (conditions, `&&`/`||`, `===`/`!==`, boundaries, booleans, a dropped `!`). The same command also writes that operator class for COBOL (`.cob`, `.cbl`), Rust, C, C++, Java, Go, and Python: `AND`/`OR` or a relational swap in COBOL, `and`/`or` in Python, and `&&`/`||` or a relational swap in the others. Strings and comments are not mutated. A COBOL `*>` comment and a fixed-format line whose column 7 is `*` or `/` are comments. Each patch is forward: apply it to introduce the bug, and the file says so. `run` executes the Node test runner. A package whose only tests are Python (`test_*.py` or `*_test.py`) runs those with `python3`. COBOL, Rust, C, C++, Java, and Go stay generate-only. A worktree that has no `node_modules` uses the main checkout's. It counts a kill only when the killing test fails twice on its own. A file that finishes its named tests and then does not exit is the same kind of kill: that file is rerun twice with no name filter. A timeout of the whole suite is not a kill. Add `--affected` to run only the test files that can see the change. The default runs the whole suite.

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

Lists the tests that can see the diff, runs mutants on the changed lines, and reports caught, missed, mutants that were not run, and any golden contract change. The mutants are the same operator class `mutate generate` would write, including a non-TypeScript diff. A string or a comment is not a mutant. The default runs every test file that imports a changed file. `--max-tests` caps that list and names the ones left out.

## queue

An item is `.probatio/queue/<id>.json`. `queue claim` renames it to `claimed/<agent>-<id>.json` with a lease. `queue reap` moves an expired lease back. `check-kill <id>` runs only the tests that directly import the mutated file, the same set `verify-change` would run. It is done only when that mutant dies, the killing test fails again on its own, and that suite is still green.

The MCP server speaks stdio JSON-RPC. Its one tool runs the CLI and returns that command's JSON.
