# Agent rules

Probatio scores a test suite. It does not replace the suite, and a kill is not proof a change is safe.

- Start from a green suite. If the baseline has any failing test, or it returns no test report, stop. Do not read a kill count from that run.
- Read no coverage first, then survivors. Neither is a pass. A timeout is the baseline clock. It is not a kill and not a gap. A mutant that did not build is `unviable`. That includes a Node test file that no longer links. It is not a kill and not a gap. For a ledger fix, write a hand-made mutant that keeps the API and restores the old behaviour.
- Do not delete a test. `mutate tally` sets `deletedTests` to 0. `noKillsYet` is not a deletion list: a test that saw no kill in one batch has not earned deletion. You may propose a deletion. You must not delete a file from a score.
- Do not treat a kill as proof the change is safe. A kill means an assertion failed. Golden and contract checks are what say the expected result is still the expected result.
- Refuse a slow unmapped suite. When the baseline is at least 5 seconds, there is no line map, and more than 30 mutants are pending, stop before the first mutant.
- A keep id is a name the suite can collect. Rescore it with `mutate run --only-test`. Do not pass `--suite-command` for that rescore. `::command` is not a test id.
- Do not restore a hidden test. If a sealed run misses, the suite did not see the bug. Leave the hidden test hidden and record the miss.
- Before calling a sealed kill a catch, check that the fix commit did not also edit the killing test. If it did, the kill came from the fix, and it is a miss.
- Do not patch a project's files or environment inside Probatio to make one repository pass. A project that needs its own env or test-line format says so in `.probatio.json`.
- Run a `nextCall` as printed. It writes to a fresh out dir. Reusing an old out dir repeats the old result.
- Read the contract from `schemas/` or `probatio schema <command>`, not from an example. A changed `schemaVersion` means a field you read may have moved.
- A red `ledger check` means a fixed bug is back in reach of the suite. Do not re-record it away. Find the test that stopped guarding it, or give the reason in a `Golden-Change:` line.
- A golden table that kills what a unit test kills is evidence, not permission. `golden compare` deletes nothing, and neither do you without a reason in the commit.
- Do not publish around the release checks. `npm publish --ignore-scripts` needs a reason in the CHANGELOG.

A trustworthy loop, after the green baseline:

- Mutate the changed lines, or a bounded batch, with a line map.
- Keep a test that killed a real mutant.
- For each survivor, add one test that fails on that mutant and passes on the unmutated code. Then rerun. A gap with no new test is still a gap.
- Leave no coverage lines alone. There is no evidence either way.
- Keep golden and contract checks.

Trust a green suite, a line map, and a small or medium diff: the caught, missed, and no coverage counts. Trust a refusal. Do not trust a kill count from a red suite, a kill count that includes compile failures, a child process started with a cleared environment, a slow Go or Java repo with no map, or a tally used as permission to delete tests.
