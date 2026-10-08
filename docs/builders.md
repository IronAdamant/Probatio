# Building Probatio

This page is for people changing Probatio. An agent scoring some other repository can stop at the README and `AGENTS.md`.

Read in this order:

1. `HANDOFF.md` — why, the evidence from Auspex, the design, and the lessons that cost time.
2. `FOR-GROK.md` — the builder brief: build order with acceptance tests, the agent contract, memory for agents, anti-gaming, swarm protocol, traps.
3. `seed/` — code copied from the Auspex experiment to start from (Auspex paths inside).

Roles: Grok builds most of it. Claude refines and finishes. Grok keeps `NOTES-FOR-CLAUDE.md` (decisions, guesses, shortcuts with file:line, acceptance output actually run).

`mutate generate` reads the commit `mutate run` checks out. The default commit is `HEAD`. `--working-tree` reads files on disk. Those patches do not match `mutate run` until the files are committed.

`verify-change` refuses a dirty working tree when `--base` and `--commit` are the same, and when that range has no diff. A clean checkout with an empty diff still says `No diff-scoped mutant.`

The MCP server is `probatio mcp`, or the `probatio-mcp` bin. It speaks one JSON object per line. `ping` returns an empty result. An unknown method returns JSON-RPC `-32601` and the same id.

A run checks the suite out in a throwaway directory under the OS temp dir (`probatio-` plus the run id). That directory symlinks the checkout's real `node_modules`. The checkout files stay where they are. Cleanup deletes only those temp directories. A path written into `worktrees.json` that is not one of them is left alone.

A project-specific need goes in that project's `.probatio.json` (`env`, `testLine`, `passLine`), not in Probatio's source. Do not add a branch that recognises one repository's files, output, or toolchain quirk. When a sample repository does not build or does not report on this machine, the result is a refusal, and that is a fine result.

Git tags and GitHub Releases are not created by a checklist run. They need a maintainer.
