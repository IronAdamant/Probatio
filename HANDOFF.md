# Probatio — handoff

Probatio (Latin: a testing, a proof) is a planned free, open-source testing toolkit built for AI
agents (Claude, Grok, ChatGPT, others), working alone or as swarms. It grew out of the test-slimming
experiment in the Auspex repo (branch `experiment/slim-tests`, folder `experiments/slim-tests/`;
https://github.com/IronAdamant/auspex/tree/experiment/slim-tests/experiments/slim-tests). This page is for the first chat
in the new Probatio folder: what was learned here, and what to build. Builder brief (Grok builds,
Claude refines): `FOR-GROK.md`.

## Why a new tool

Mutation testing (Stryker, mutmut, PIT), snapshot/approval tests, and test-impact selection all
exist, but they assume a human writes every test and reads every report. Agents change that:

- An agent can write a test for every gap, if it is told exactly which gap ("line 92 is untested").
- An agent can re-record a snapshot and erase the regression it caught. Tools must make the
  important parts impossible to re-record silently.
- A swarm needs tasks with an objective "done": "write a test that kills mutant s155" is done when
  the mutant dies. Mutants shard cleanly across agents and machines.
- An agent's context is precious: it should read and run the 6 tests that matter, not 800.

## Evidence from Auspex (TypeScript, node:test, ~18k src lines)

Round 1 (33 real bugs from the repo's history, each put back into today's code):

| suite | tests | test code | real bugs caught |
|---|---|---|---|
| full | 770 | 21,064 lines | 33/33 |
| cut source-grep, doc-phrase, duplicate tests | 715 | 19,915 | 32/33 |
| also drop pure unit tests | 466 | 14,799 | 29/33 |
| plus golden tables recorded from those unit tests | 466 + 868 rows | 14,917 + tables | 31/33 |

Round 2 (validated; full write-up and tables in the Auspex experiment's `README.md`):

| suite | test code lines | holdout real bugs (sealed) | unseen synthetic |
|---|---|---|---|
| full | 21,064 | 18/18 | 98.1% |
| greedy kill-matrix cover, k=2 ("slim C") | 8,912 | **10/18** | 88.8% |
| rules + golden tables + mutation-found gaps ("slim D") | 16,232 (−23%) | **18/18** (19/19 after one fix) | **99.3%** |

- **Pruning by kill evidence overfits.** A cover fitted to one synthetic batch caught 100% of it
  and 87–89% of the other; on the sealed real-bug holdout it caught 10 of 18. Of 55 tests with
  zero training kills, 49 caught something in the next batch. "Zero kills" was sampling noise.
- **What generalised:** cut tests by kind (source greps, doc phrases, duplicates), turn pure unit
  tests into golden tables recorded from them, keep tests with hidden inputs as code, and use
  mutation testing to **find gaps** (25 untested branches, 2 weak assertions; one ~100-line golden
  table closed them, each row verified to kill its mutant).
- **The holdout found a bug every suite missed**, the full one included (a printed path naming the
  home folder). A growing real-bug ledger is worth more than any single audit.
- Bigger cuts with no loss will need new kinds of test, not pruning: record-replay flow goldens and
  docs generated from code.

Lessons that cost time (build them into the tool):

- node's TAP output for several files does not name the file of a failing test; a remembered
  "# Subtest" heading stamps the wrong file. Attribute failures by title (titles must be unique).
- Under heavy parallel load (load 66 on 12 cores) timing tests flake. Cap per-suite concurrency,
  and confirm every kill by rerunning the failing test twice in isolation.
- Store mutant patches in one direction and say which; a reversed reading inverted conclusions.
- A patch can span several files; select tests for every file it touches, not the first.
- Score a suite only against bugs the full test union can catch, never against itself.
- Golden normalisation must touch only this run's own folders (package, home, per-run temp), never
  literal paths such as `/tmp/x.png`, or rows differ between macOS and Linux CI.
- A recorder that captures call arguments cannot see hidden inputs (stubbed globals, fakes, the
  clock not passed in). Those tests stay as code.
- A test is "integration" if it reaches a process or file through a helper, not only directly.
- Hand-written diff hunks break at end of file; use `diff -u`.
- Fuzzy patching (`patch -F3`) can report success on a dry run that grep misreads; check exit codes.

## Design (v0)

TypeScript/JavaScript first, adapters for other languages later. CLI and MCP server, same contract
(JSON out, one next step), so any agent or swarm can drive it.

1. `ledger` — every fix commit (trailer `Fixes-bug:` or detected) becomes a mutant by reverting its
   src change; hand-craft when it conflicts. The suite must catch every ledger bug, forever. A real,
   growing holdout that nobody tunes to.
2. `mutate` — operator mutants (negate conditions, && <-> ||, === <-> !==, boundaries, booleans,
   drop !) in code only, never strings. Diff-scoped on every change (~20 mutants, minutes); full
   audit on a schedule. Sharded across workers or agents; kills confirmed by isolated reruns.
3. `matrix` — kill matrix, per-test unique kills, and above all **gaps** (mutants nothing catches,
   with `file:line`). Pruning is advisory only: a test is a prune candidate when it has zero kills
   across the ledger and *dense* mutation (every mutation point, not a sample), and deleting it
   still needs a reason in the commit. Never prune from one cover (round 2: 10 of 18 real bugs).
4. `golden` — tables of real inputs with recorded outputs, split into **contract** (verdict fields:
   ok, reason, status, nextCall.tool…) and **wording**. Wording re-records freely; a contract change
   fails CI unless the commit says `Golden-Change: <row>: <why>`. **Invariants** (the project's rules
   as code, e.g. "ok implies reason matched", "no output names the home folder") run over every row,
   so no re-record can break them. A recorder converts existing unit tests into rows.
5. `verify-change` — the agent report: affected tests (from a stored impact map), diff-scoped
   mutants caught/missed, invariants, golden diff split into contract and wording.

Later: record-replay flow goldens (real HTTP/page recordings replayed offline, so async flows get
end-to-end tests without fakes); docs generated from code instead of tested against it.

Prove it on at least two more projects (one other TypeScript repo, one Python repo) before calling
it general.

## Seed code (`seed/`)

Copied from the Auspex experiment on 2026-10-03. Starting points, not a library: paths inside still
assume the Auspex layout (`examples/auspex-ts/...`), and the mutant patches and result tables stay
in the Auspex repo.


`synth-mutants.ts` (operator mutants via the TypeScript compiler API), `run-mutants.mjs` (sharded
runner with affected-test selection), `confirm-kills.mjs`, `affected.mjs`, `matrix-lib.mjs`,
`kill-matrix.mjs`, `cover.mjs`, `record-goldens.ts` (unit tests → golden rows),
`prune-unused.ts`, `craft-mutants.py` / `craft-holdout.py`, `score.mjs`, `drop-tests.mjs`, and the
golden harness in `seed/golden-harness/` (from Auspex `examples/auspex-ts/tests/`).
