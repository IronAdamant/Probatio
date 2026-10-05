# Golden tables

Each file in `cases/` is a table: row name → one call into `src/`. `out/<area>.json` holds what
each call returned (or threw) when it was last recorded. `golden.test.ts` runs every row and fails
an area with the list of rows whose result changed, each with the recorded and the new value.

- **A row changed and you meant it** (new wording, a new field): `UPDATE_GOLDEN=1 npm test`, then
  read the JSON diff in `out/` before committing. The diff is the review.
- **A row changed and you did not mean it**: that is the regression. Fix the code, not the table.
- **New edge case**: add a row to the nearest table (real data first: a URL, receipt or error you
  met live), record, commit the row and its output together.

Rows run at a fixed clock (`at(...)`, or 2026-09-30 by default) and never touch the network.
`cases/*` from `record-goldens.ts` were recorded from the unit tests they replaced; `solari-errors`,
`schemas` and `real-receipts` are written by hand, the last from the live receipts in `demo/`.
