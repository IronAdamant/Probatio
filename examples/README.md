# Examples

Small suites Probatio can score. `expected.json` is the least each one should kill. `examples/node-ts` runs TypeScript tests with `node --test`, which needs Node 22.18 or later (type stripping on by default).

```bash
npx probatio mutate generate --package examples/node-ts --src src --out .probatio/generate
npx probatio mutate run --package examples/node-ts --patches .probatio/generate/mutants --out .probatio/runs
```

`mutate run` checks out the commit. Commit the example before you score it. `generate` reads that same commit unless you pass `--working-tree`.

`examples/pytest` has a `pytest.ini` so the suite is pytest. Without that file, the same tests are collected as unittest and the run stops with an empty report.
