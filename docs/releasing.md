# Releasing Probatio

One version, one commit, one tag, in this order. `npm publish` runs `scripts/release-check.mjs` first (`prepublishOnly`) and refuses when a step is missing. `npm run release:check` runs the same checks without publishing.

1. **Changes land on `main` and both workflows go green on that commit.** `test` (core, pack on Node 22 and 24, toolchain, macos) and `self-score` (Probatio's own ledger: every fixed bug keeps the outcome recorded in `ledger/<id>.golden.json`). A red self-score means a fixed bug is back in reach of the suite, or a ledger patch stopped applying. Fix that first.
2. **Version commit.** `npm version <x.y.z> --no-git-tag-version`, then add `## <x.y.z>` to `CHANGELOG.md` with what changed for an agent reading the JSON. Bump `SCHEMA_VERSION` in `src/contract.ts` and the `schemaVersion` const in every `schemas/*.schema.json` when a field is renamed, removed, or changes meaning. A new field does not bump it. Commit as `Set version <x.y.z> ...` and push.
3. **Wait for both workflows on the version commit.** `gh run list --commit $(git rev-parse HEAD)`.
4. **Tag that commit.** `git tag -a v<x.y.z> -m v<x.y.z> && git push origin v<x.y.z>`.
5. **Publish.** `npm publish` from a clean `main`. The checks print one line each, and every one must say `ok`.
6. **Record it.** A follow-up commit adds the publish date and gitHead to the `CHANGELOG.md` entry. The tag stays on the version commit.

`npm publish --ignore-scripts` skips the checks. Use it only when a check is wrong, and say which and why in the CHANGELOG entry.

Why this order: 0.0.1 to 0.1.3 went out in two days, and the review on 2026-10-08 found false kills, false survivors, and a headline proof that did not hold. Each one was green on the tests of the day. A self-score on the version commit is the cheapest check that the tool still tells the truth about its own suite.
