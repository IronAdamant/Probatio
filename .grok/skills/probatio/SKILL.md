---
name: probatio
description: >
  Set up and run a Probatio mutant campaign from this repo: generate, baseline,
  line map, and the serial run. Use when the user says set up Probatio, line map,
  serial campaign, mutate this repo, score this suite, or runs /probatio.
---

# Probatio campaign

The scoring rules live in `AGENTS.md`. Follow them. This skill is only the procedure an agent uses to start a run, and the MCP tools that perform it.

Prefer the `probatio` MCP server when it is connected. The tools are `schema`, `mutate_generate`, `mutate_run`, and `mutate_tally`. If the server is not connected, run this repo's binary:

```bash
cargo build --offline
./target/debug/probatio <command>
```

Build before the first call. The project MCP entry is `.grok/config.toml`. It launches `probatio mcp` from this checkout. A new session picks up a server that was just added.

## Ask, then start

The package is the directory the user named. It can be this repo or another checkout. Do not edit that checkout's source, tests, or environment to make a suite pass.

1. Call `mutate_generate` with `package` and `out`. Use `<package>/.probatio/generate` for `out` unless the user named a directory. Pass `src` when they named one directory. Pass `maxMutants` only when they asked for a slice. The summary states the count, and a bound when the walk stopped early.
2. Tell the user the mutant count and the suite command you will use. Discovery is the marker at the package root: `go.mod` to `go test ./...`, `Cargo.toml` to `cargo test`, `pom.xml` to `mvn -B test`, `package.json` to `node --test`, `pytest.ini` or `pyproject.toml` to `python3 -m pytest`. If none of those is present, stop and ask for the suite command. Do not invent one.
3. If the count is above 100 and the user did not already ask for the whole repo or a serial campaign, stop and wait.
4. Call `mutate_run` with `package`, `patches` set to the generate directory, and `out` set to `<package>/.probatio/run`. Set `timeoutMs` to one full suite run with room to spare. The default is 120000. A Maven suite needs 600000 or more.
5. For 30 mutants or fewer, omit `lineMap` and `serial`. For more than 30, set `serial` to true. That writes `<out>/lines.json` and passes it as `--line-map`. The file is a non-empty permission slip. The binary does not read coverage from it and does not choose tests. The suite command runs once per mutant, one mutant at a time. Workers stay at 1.
6. Read the envelope. `ok` false is a stop. Show `summary` and `next`. A red baseline, a missing report, a timeout, a missing tool, or "No line map" means nothing was mutated. Do not treat any of those as a kill count.
7. When `ok` is true, call `mutate_tally` on the same `out`. `deletedTests` stays 0. Do not delete a test.

The same `mutate_run` call is the resume. A green `baseline.json` is kept. A mutant that already has `results/<id>.json` is not run again.

A long `mutate_run` holds the MCP server until that call returns. If the tool call is cut off, run the same call again.
