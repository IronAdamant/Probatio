---
name: probatio
description: >
  Set up and run a Probatio mutant campaign from this repo: generate, baseline,
  line map, and the serial run. Use when the user says set up Probatio, line map,
  serial campaign, mutate this repo, score this suite, or runs /probatio.
---

# Probatio

Follow `AGENTS.md` in this checkout. That file is the contract: what Probatio does, the MCP tools, the CLI commands, how a campaign runs, and the scoring rules. `docs/` and `CHANGELOG.md` describe the old TypeScript program.

Prefer the `probatio` MCP server when it is connected. The tools are `schema`, `mutate_generate`, `mutate_run`, and `mutate_tally`. The project entry is `.grok/config.toml`. It launches `probatio mcp` through `.grok/probatio-mcp.sh`. A session that is already open does not see a server that was just added.

If the server is not connected:

```bash
cargo build --offline
./target/debug/probatio <command>
```

The package is the directory the user named. Do not edit that checkout's source, tests, or environment to make a suite pass.
