# Agent run plan

This is the work to make a campaign followable from the envelope. `plan.md` stays the record of the Rust rewrite.

`AGENTS.md` keeps the scoring rules. This plan does not restate them and does not weaken them.

Thirty is `LARGE_BATCH` in `src/decide.rs`. It is the edge of an unmapped batch. It is not a memory budget. The run stays one mutant at a time, so peak memory stays one worktree plus one suite process. This plan does not add workers.

`schemaVersion` stays 2. A new field may appear. A field that already exists must not change meaning.

## 1. Name the suite command

Every `mutate run` envelope names the command that was used.

- A discovered command, a `--suite-command` flag, and a command from `.probatio.json` all follow this.
- Put the string on the envelope as `suiteCommand`. The run schema already allows that field.
- Also put it in `next`, including when the run stops. A red baseline, a missing report, a timeout, a missing tool, and the unmapped stop must all carry it. An agent that only reads `summary` and `next` can pass a different command on the next call.
- `--suite-command` wins when it is present.
- When the flag is absent, read `<package>/.probatio.json`. If it is valid JSON and has a non-empty string `suiteCommand`, use that. If the file is missing, or it has no `suiteCommand`, discover from the root markers as today.
- If `.probatio.json` exists and is not valid JSON, refuse. Do not fall through to discovery.
- Do not interpret `env`, `testLine`, or `passLine` in this work. Those keys stay in the file for a later reader.

## 2. Chunk a mapped batch

The unmapped stop stays. More than 30 mutants and no usable line map still writes `not-started.txt` and starts nothing.

A usable line map allows the batch. It does not choose tests. The first mapped envelope says that in `next`.

After the baseline is green:

- Take mutants that do not yet have `results/<id>.json`, in `mutants.json` order.
- Score at most `LARGE_BATCH` of them.
- `summary` counts everything in the out dir so far, which is what the run already does when it reuses result files.
- `next` says how many started this call and how many are still waiting.
- `nextCall.argv` repeats the same package, patches, out, suite command, line map, timeout, and tests file. It does not include `--confirm-count`.
- Rewrite `not-started.txt` to the ids that still have no result. Remove that file when none remain.
- The same command is the resume.

`--confirm-count N` is the one-call escape.

- `N` is compared to the number of mutants that have no result at the start of the call, after the green baseline.
- If `N` matches, score all of them in this call. `nextCall` is null when none remain.
- If `N` does not match, refuse. `next` contains the real count. Write no new result files.
- Omit the flag on ordinary agent calls. `serial: true` on the MCP tool stays "write `<out>/lines.json` and pass `--line-map`". The chunking then happens inside `mutate run`, so the CLI and the tool behave the same way.

## 3. Say when the suite was not narrowed

`--tests` and the line map do not change the suite command today. The command in `apply_and_run` runs as given.

- If the suite command contains the literal `{tests}`, replace it with the selected names, each single-quoted, joined by one space.
- If it contains the literal `{tests,}`, replace it with the selected names joined by a comma and not quoted, for a Maven `-Dtest=` list.
- A selected name that contains a quote, a comma, or other shell metacharacters makes that replacement refuse. The envelope names the name and starts no mutant with that command.
- If neither token is present, run the command unchanged. `next` says the suite ran whole.
- The line map is not a coverage file. Do not parse it for test names.

## Tests

Drive the shipped binary in `tests/cli.rs`. A pure decision can stay a unit test next to `scale_stop`.

- A red baseline's `next` contains the suite command, and the envelope has `suiteCommand`.
- A `.probatio.json` `suiteCommand` is used when the flag is absent. The flag wins when both are present. Invalid JSON refuses.
- Thirty-one mutants, a non-empty line map, and `echo ok` score 30 and return a `nextCall`. The second launch scores the last mutant and clears `not-started.txt`.
- Without a line map, those thirty-one still start nothing.
- `--confirm-count 31` scores all thirty-one in one launch. A wrong count refuses and adds no result file.
- `echo {tests}` with two names runs a command that contains both names. A command with no token says the suite ran whole.
- `schemaVersion` on these envelopes is 2.

## After the binary

Update the `probatio` skill, the project copy and `~/.grok/skills/probatio/SKILL.md`, so `serial` means one chunk and the same call continues. Update the `mutate_run` tool text the same way. Add `confirmCount` to that tool. Do not add a worker flag that runs more than one mutant at a time.

## Not in this plan

- A real coverage map from file and line to test names.
- Reading `env`, `testLine`, or `passLine`.
- Raising `LARGE_BATCH`, or treating 30 as a RAM cap.
- More than one worker.
- Editing a sample project so its suite goes green.
