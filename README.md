# Probatio

Probatio asks one question: if this code were a little bit wrong, would the tests you already have notice?

It makes one small change to the code (a mutant: `>` becomes `>=`, `&&` becomes `||`, `true` becomes `false`), runs the project's own test command, and records whether a test failed. A change no test catches is a gap, reported as `file:line`. It is built for AI agents: every command prints one short JSON object with the next step, and an MCP server exposes the same commands.

It does not write tests, and a caught mutant is not proof the code is right. If the suite is already failing, Probatio stops before the first change.

## Install

```bash
npm install -g probatio      # a Node launcher plus the binary for this machine
probatio --version
```

Linux x64, macOS (Apple silicon and Intel), and Windows x64 are published. Elsewhere, build from source: `cargo build --release` writes `target/release/probatio`. It uses only the Rust standard library. At run time it needs `git` and the project's own test tools. On Windows it runs suite commands with Git for Windows' `sh`.

## Connect an MCP client

```json
{
  "mcpServers": {
    "probatio": {
      "command": "npx",
      "args": ["-y", "probatio", "mcp"]
    }
  }
}
```

In Claude Code: `claude mcp add probatio -- npx -y probatio mcp`. The MCP Registry name is `io.github.IronAdamant/probatio` ([server.json](server.json)); the `mcp registry` workflow publishes the listing (automatic on tags once `MCP_REGISTRY_PUBLISH` is set).

## Example

After editing code, an agent makes one call. It mutates only the changed lines and names the ones no test checks. This is a real run on a Rust crate after one line was edited:

```text
mutate_diff {"package": "/work/relatum"}
  -> "1 changed line in 1 file since HEAD: 4 mutants. 0 no coverage, 1 survived, 3 killed, ..."
     survivors: [{"file": "src/grammar.rs", "line": 46, "column": 50, "original": "==", "replacement": "!="}]
```

`since` is `HEAD` by default (uncommitted edits). `since: "main"` scores this branch's own changes: it diffs from the merge-base of `main` and HEAD, so lines that only changed on `main` are left out, even after `main` was merged into the branch. A range works too: `main..HEAD` starts at `main` itself, `main...HEAD` at the merge-base. Test code is not mutated: `tests/`, `benches/`, test files (`*_test.go`, `test_*.py`, `*.test.ts`, `FooTest.java`, ...), and Rust `#[cfg(test)]` and `#[test]` items. `testLinesSkipped` counts the changed lines left out, and `includeTests: true` mutates them too. A mutant that does not build is listed under `unbuildable`, never as a survivor.

Calls that run tests are jobs, so none is cut off by a client's idle timeout (Claude Code's is 30 minutes). The call waits `waitSeconds` (default 20, at most 600). If the run ends by then, the reply is its result. If not, the reply has a `job_id` and the progress so far, and the run goes on:

```text
mutate_diff {"package": "/work/game/crates/audio", "since": "main", "waitSeconds": 0}
  -> {"status": "running", "job_id": "job-18a...", "progress": {"phase": "baseline", "total": 12, ...}}
job_status  {"job_id": "job-18a...", "waitSeconds": 600}
  -> the mutate_diff result, as soon as the run ends ("status": "completed")
cancel_job  {"job_id": "job-18a..."}
  -> stops the run and every process under it, and removes its mutant tree
```

`job_status` also answers after the job ended, and from a new server after a restart (each job has a record in the cache folder). Output goes to `.probatio/` at the project root (the top of the git checkout), never inside a workspace member, and that folder ignores itself, so it is not committed by accident. `PROBATIO_DIR` moves it.

For a whole package, three calls. The package is never edited: each mutant is applied in a git worktree, or a temp copy when the package is not a git checkout.

```text
mutate_generate {"package": "/work/app", "out": "/work/app/.probatio/generate"}
  -> "6 mutants in 1 files."
mutate_run      {"package": "/work/app", "patches": "/work/app/.probatio/generate", "out": "/work/app/.probatio/run"}
  -> "0 no coverage, 4 survived, 2 killed, 0 flaky, 0 timed out, 0 errored, of 6 finished."
mutate_tally    {"out": "/work/app/.probatio/run"}
  -> "First gap is me96bde61bf73 at src/calc.js:2. ... Add one test that fails on that mutant and passes on the code."
```

The same commands from a shell:

```bash
probatio mutate diff --package .            # changed lines since HEAD
probatio mutate generate --package . --out .probatio/generate
probatio mutate run --package . --patches .probatio/generate --out .probatio/run
probatio mutate tally --out .probatio/run
```

Each reply has `ok`, `summary`, `next`, and `nextCall` (the argv to run next, or null). `ok: false` is a stop: show `summary` and `next`.

## How it works

1. **Generate** walks `src/` (or `src` you name) and writes `mutants.json`: text operators on conditions, comparisons, and booleans, never inside strings or comments, and not in test code unless `includeTests`. It runs no tests.
2. **Run** finds the test command (your `suiteCommand`, then `.probatio.json`, then a marker such as `Cargo.toml`, `package.json`, `go.mod`, `pom.xml`, or `pytest.ini`) and runs it once on the unchanged code. A red baseline, a missing tool, or a suite that ran no tests stops here, with the last lines it printed. Then each mutant is applied in a worktree and the suite runs again.
   The mutant tree is a git worktree of the whole checkout (a workspace member builds with its workspace), made once per run in Probatio's cache folder (`PROBATIO_CACHE_DIR`, else `%LOCALAPPDATA%\probatio`, `~/Library/Caches/probatio`, or `~/.cache/probatio`). It holds your tracked and untracked files with their mtimes, not ignored ones. After each mutant, the files it or the suite changed are copied back and what the suite added is removed. Each tree has a Cargo target directory that is kept between runs, so mutants after the first, and later runs, build only what changed; a file a mutant edited is always rebuilt from its clean text. Your own target directory is never used, even when `CARGO_TARGET_DIR` or `--target-dir` names it. Before the first mutant the suite runs once in each tree with no mutant, which builds it, and a test that fails there stops the run (a test that needs a file git ignores can get it with `"copy": ["path"]` in `.probatio.json`). The tree is removed when the run ends, is cancelled, or, after a crash, on the next start, which also sweeps trees and target folders older versions left in the temp folder.
3. Each mutant is **killed** (a test failed), **survived** (a gap), **no coverage** (no test ran that line), **unviable** (did not build), **timeout**, or **error**. Kills are confirmed by rerunning the killing test twice unless you pass `confirm: false`.
4. **Tally** lists the tests that killed something and the gaps with `file:line`. It works while a run is still going, so a second agent can read progress. It never deletes a test.

More than 30 mutants with no line map stop before the first mutant: pass `serial: true`, or a line map that names tests per line. Python (`coverage.py`), Go, Node, Java (JaCoCo), and Rust (`cargo llvm-cov`) baselines write that map when the tool is installed. A run that is interrupted resumes with the same call. [docs/reference.md](docs/reference.md) has the detail, and [AGENTS.md](AGENTS.md) is the contract an agent follows.

## Tool groups

| Group | Tools | Overlaps with |
| --- | --- | --- |
| `core` (always on) | `mutate_diff`, `mutate_generate`, `mutate_run`, `mutate_tally`, `job_status`, `cancel_job` | none |
| `diagnostics` | `schema` | none |
| `cli` | `probatio` (any CLI command by argv: ledger, golden, matrix, queue, check-kill, verify-change, ...) | none |

No group overlaps another tool in the suite, so the shared names (`locks`, `notes`, `tasks`, `messages`, `history`) are not used. `tools/list` is 6538 bytes with every group on, 5919 with `cli` off, 6245 with `diagnostics` off, and 5626 with only `core` (0.7.1: 4633 with every group on; the two job tools and the when-to-use text are the difference).

An operator sets `PROBATIO_TOOLS` (an allowlist; `core` is always added) or `PROBATIO_DISABLE` in the client's `env` block. Names are groups or tool names, commas or spaces, any case. An unknown name, or `core`, stops the server with one line on stderr and exit code 2.

```json
{ "mcpServers": { "probatio": { "command": "npx", "args": ["-y", "probatio", "mcp"], "env": { "PROBATIO_DISABLE": "cli" } } } }
```

### Choosing groups

An agent chooses the groups for a project with an `agent-tools` block in the `AGENTS.md` at the directory the server starts in. The server rereads the file when it changes and sends `notifications/tools/list_changed`, so no restart is needed. An env var wins over the file for the same key. A bad line in the file is ignored and named in the `initialize` instructions.

- Keep `core`. It is the campaign.
- Turn off `cli` when the agent only runs campaigns: no ledger, golden, matrix, queue, or check-kill work.
- Turn off `diagnostics` once the agent knows the flags; the tools' own schemas are enough for the MCP calls.

The line to add, in a fenced block whose info string is `agent-tools`:

```text
probatio: disable cli, diagnostics
```

## Benchmarks

Five runs of each command on one machine, median milliseconds with the min and max in parentheses. TypeScript is commit `c5cd175` (`node /tmp/probatio-ts/dist/cli.js`, Node 22.23.4). Rust is `target/release/probatio` from this checkout. The machine was Linux 6.12, x86_64, 4 cores, Intel Xeon, 16 GB. Peak RSS and the raw envelopes are in [scripts/benchmark_results.md](scripts/benchmark_results.md). Re-run with `cargo run --release --example benchmark`.

Kill counts matched on every scored run. `--no-confirm` is on for the run rows so the clock is the suite, not the confirmation rerun. The JS run was measured again after that command gained `--test-concurrency` and `--test-timeout`; the count stayed 2 killed and 4 survived. itsdangerous is Pallets itsdangerous at the depth-1 checkout used here: 10 mutants in 8 files, so `--max-mutants 20` did not drop any.

| Command | TypeScript | Rust |
| --- | --- | --- |
| startup, `schema` | 287 (287–297) | 11 (11–11) |
| generate, JS sample, 6 mutants | 307 (307–308) | 11 (11–11) |
| generate, Python sample, 6 mutants | 307 (297–308) | 11 (11–11) |
| run, JS sample, 2 killed / 4 survived | 832 (823–853) | 743 (722–762) |
| run, Python sample, 2 killed / 4 survived | 1429 (1418–1450) | 1130 (1090–1171) |
| run, Python sample, `--workers 4`, same 2 killed | 805 (796–851) | 491 (471–495) |
| generate, itsdangerous `src`, 10 mutants | 369 (360–371) | 21 (21–21) |
| run, itsdangerous, 7 killed / 3 survived | 7568 (7528–7625) | 7003 (6995–7049) |

## More

- [AGENTS.md](AGENTS.md): the agent contract, including suite discovery, line maps, and every CLI command.
- [docs/reference.md](docs/reference.md): what each command prints, discovery, results, stops, and line maps.
- [MIGRATION.md](MIGRATION.md): differences from the TypeScript program (npm 0.3.0, branch `typescript-legacy`).
- [CHANGELOG.md](CHANGELOG.md) and [docs/releasing.md](docs/releasing.md).

MIT licensed.
