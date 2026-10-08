import assert from "node:assert/strict"
import test from "node:test"

const HEAD = "a".repeat(40)
const OLD = "b".repeat(40)

type Reply = { status: number; stdout: string }

/** A fake git, npm, and gh. Every answer is what a release-ready repo would say, unless overridden. */
function runner(overrides: Record<string, Reply> = {}) {
  const answers: Record<string, Reply> = {
    "git status --porcelain": { status: 0, stdout: "" },
    "git rev-parse --abbrev-ref HEAD": { status: 0, stdout: "main\n" },
    "git rev-parse HEAD": { status: 0, stdout: `${HEAD}\n` },
    "git ls-remote origin refs/heads/main": { status: 0, stdout: `${HEAD}\trefs/heads/main\n` },
    "npm view probatio@0.3.0 version": { status: 0, stdout: "" },
    "git rev-parse v0.3.0^{commit}": { status: 0, stdout: `${HEAD}\n` },
    "git ls-remote --tags origin refs/tags/v0.3.0^{} refs/tags/v0.3.0": { status: 0, stdout: `${OLD}\trefs/tags/v0.3.0\n${HEAD}\trefs/tags/v0.3.0^{}\n` },
    [`gh run list --commit ${HEAD} --workflow test.yml --branch main --event push --json status,conclusion --limit 1`]: { status: 0, stdout: '[{"status":"completed","conclusion":"success"}]' },
    [`gh run list --commit ${HEAD} --workflow self-score.yml --branch main --event push --json status,conclusion --limit 1`]: { status: 0, stdout: '[{"status":"completed","conclusion":"success"}]' },
    ...overrides,
  }
  return (bin: string, args: string[]) => answers[[bin, ...args].join(" ")] ?? { status: 1, stdout: "" }
}

async function checks(overrides: Record<string, Reply> = {}, changelog = "# Changelog\n\n## 0.3.0\n\n- Published soon.\n") {
  const { releaseChecks } = (await import("../scripts/release-check.mjs")) as {
    releaseChecks: (input: { run: (bin: string, args: string[]) => Reply; version: string; changelog: string; name: string }) => {
      ok: boolean
      checks: Array<{ ok: boolean; label: string; fix: string }>
    }
  }
  return releaseChecks({ run: runner(overrides), version: "0.3.0", changelog, name: "probatio" })
}

test("a release-ready commit passes every check", async () => {
  const result = await checks()
  assert.equal(result.ok, true, JSON.stringify(result.checks.filter((check) => !check.ok)))
  assert.equal(result.checks.length, 8)
})

test("a red self-score, a missing tag, or a published version stops npm publish", async () => {
  const red = await checks({
    [`gh run list --commit ${HEAD} --workflow self-score.yml --branch main --event push --json status,conclusion --limit 1`]: { status: 0, stdout: '[{"status":"completed","conclusion":"failure"}]' },
  })
  assert.equal(red.ok, false)
  assert.ok(red.checks.some((check) => !check.ok && check.label.startsWith("self-score.yml")))
  const running = await checks({
    [`gh run list --commit ${HEAD} --workflow test.yml --branch main --event push --json status,conclusion --limit 1`]: { status: 0, stdout: '[{"status":"in_progress","conclusion":""}]' },
  })
  assert.equal(running.ok, false, "a run that has not finished is not green")
  const untagged = await checks({ "git rev-parse v0.3.0^{commit}": { status: 128, stdout: "" } })
  assert.equal(untagged.ok, false)
  assert.match(untagged.checks.find((check) => !check.ok)?.fix ?? "", /git tag -a v0\.3\.0/)
  const published = await checks({ "npm view probatio@0.3.0 version": { status: 0, stdout: "0.3.0\n" } })
  assert.equal(published.ok, false)
  const unlisted = await checks({}, "# Changelog\n\n## 0.2.0\n")
  assert.equal(unlisted.ok, false)
  const unpushed = await checks({ "git ls-remote origin refs/heads/main": { status: 0, stdout: `${OLD}\trefs/heads/main\n` } })
  assert.equal(unpushed.ok, false)
  const dirty = await checks({ "git status --porcelain": { status: 0, stdout: " M src/cli.ts\n" } })
  assert.equal(dirty.ok, false)
})

test("the check reads the main push run, so a run started by the tag push cannot stand in for it", async () => {
  // Without --branch main, gh would return the newest run on this commit, which is the tag's.
  const ignoresTag = await checks({
    [`gh run list --commit ${HEAD} --workflow test.yml --json status,conclusion --limit 1`]: { status: 0, stdout: '[{"status":"in_progress","conclusion":""}]' },
  })
  assert.equal(ignoresTag.ok, true, JSON.stringify(ignoresTag.checks.filter((check) => !check.ok)))
})
