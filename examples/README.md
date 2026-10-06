# Examples

Small suites Probatio can score. Each one is copied to a temp git repo by `tests/examples.test.ts`. A missing compiler skips that example with `missing tool: <name>`.

```bash
npx probatio mutate generate --package examples/node-ts --src src --out .probatio/generate
npx probatio mutate run --package examples/node-ts --patches .probatio/generate/mutants --out .probatio/runs
```

`mutate run` checks out the commit. Commit the example before you score it. `generate` reads that same commit unless you pass `--working-tree`.

`examples/pytest` has a `pytest.ini` so the suite is pytest. Without that file, the same tests are collected as unittest and the run stops with an empty report.
