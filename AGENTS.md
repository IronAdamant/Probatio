# Agent rules

Probatio scores a test suite. It does not replace the suite, and a kill is not proof a change is safe.

- Start from a green suite. If the baseline has any failing test, or it returns no test report, stop. Do not read a kill count from that run.
- Read no coverage first, then survivors. Neither is a pass. A timeout is the baseline clock. It is not a kill and not a gap.
- Do not delete a test. `mutate tally` sets `deletedTests` to 0. A test that never saw a mutant has not earned deletion. You may propose a deletion. You must not delete a file from a score.
- Do not treat a kill as proof the change is safe. A kill means an assertion failed. Golden and contract checks are what say the expected result is still the expected result.
- Refuse a slow unmapped suite. When the baseline is at least 5 seconds, there is no line map, and more than 30 mutants are pending, stop before the first mutant.
- A keep id is a name the suite can collect. Rescore it with `mutate run --only-test`. Do not pass `--suite-command` for that rescore. `::command` is not a test id.
- Do not restore a hidden test. If a sealed run misses, the suite did not see the bug. Leave the hidden test hidden and record the miss.

A trustworthy loop, after the green baseline:

- Mutate the changed lines, or a bounded batch, with a line map.
- Keep a test that killed a real mutant.
- For each survivor, add one test that fails on that mutant and passes on the unmutated code. Then rerun. A gap with no new test is still a gap.
- Leave no coverage lines alone. There is no evidence either way.
- Keep golden and contract checks.

Trust a green suite, a line map, and a small or medium diff: the caught, missed, and no coverage counts. Trust a refusal. Do not trust a kill count from a red suite, a child process marked covered, a slow Go or Java repo with no map, or a tally used as permission to delete tests.
