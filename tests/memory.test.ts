import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("a fixed gap regresses, goes stale, and a guard change needs a trailer", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-memory-"))
  const state = path.join(dir, "state")
  const page = path.join(dir, "PROBATIO.md")
  const guardFile = path.join(dir, "tests", "gate.test.ts")
  const sealed = "sealed-bug-9f3a"
  try {
    mkdirSync(path.dirname(guardFile), { recursive: true })
    writeFileSync(guardFile, "export {}\nassert.equal(1, 1)\n")
    const fix = run(["gap", "fix", "--state", state, "--root", dir, "--id", "gap-1", "--file", "src/gate.ts", "--line", "1", "--guard", "tests/gate.test.ts:2", "--commit", "abc123"])
    assert.equal(fix.status, 0, fix.stderr)
    let status = readStatus(state, dir, page)
    assert.equal(status.recentFixes[0].id, "gap-1")
    assert.equal(status.recentFixes[0].guard, "tests/gate.test.ts:2")
    assert.equal(status.recentFixes[0].fixedIn, "abc123")
    assert.equal(status.decisions[0].review, "current")
    assert.match(pageText(page), /gap-1 fixed in abc123, guard tests\/gate\.test\.ts:2/)

    const reverted = run(["gap", "revert", "--state", state, "--id", "gap-1"])
    assert.equal(reverted.status, 0, reverted.stderr)
    assert.equal(JSON.parse(reverted.stdout).summary, "fixed in abc123, guarded by tests/gate.test.ts:2; the guard broke")
    status = readStatus(state, dir, page)
    assert.equal(status.regressions[0].id, "gap-1")
    assert.equal(status.regressions[0].text, "fixed in abc123, guarded by tests/gate.test.ts:2; the guard broke")

    writeFileSync(guardFile, "export {}\nassert.equal(1, 2)\n")
    status = readStatus(state, dir, page)
    assert.equal(status.decisions[0].review, "stale")
    const refused = run(["guard", "check", "--state", state, "--root", dir, "--guard", "tests/gate.test.ts:2"])
    assert.equal(refused.status, 1, refused.stderr)
    const allowed = run(["guard", "check", "--state", state, "--root", dir, "--guard", "tests/gate.test.ts:2", "--message", "Guard-Change: tests/gate.test.ts:2: the assertion moved"])
    assert.equal(allowed.status, 0, allowed.stderr)

    const missing = run(["findings", "add", "--state", state, "--id", "note-1", "--status", "open"])
    assert.notEqual(missing.status, 0)
    for (const item of [
      ["note-1", "open", "still looking"],
      ["note-1", "fixed", "covered"],
      ["note-2", "equivalent", "same branch"],
      ["note-3", "wont-fix", "intended"],
    ] as const) {
      const added = run(["findings", "add", "--state", state, "--id", item[0], "--status", item[1], "--reason", item[2]])
      assert.equal(added.status, 0, added.stderr)
    }
    const lines = readFileSync(path.join(state, "findings.jsonl"), "utf8").trim().split("\n")
    assert.equal(lines.length, 4)
    assert.equal(JSON.parse(lines[0]).reason, "still looking")
    assert.equal(JSON.parse(lines[1]).status, "fixed")
    assert.equal(JSON.parse(lines[2]).status, "equivalent")
    assert.equal(JSON.parse(lines[3]).status, "wont-fix")

    writeFileSync(guardFile, "export {}\nassert.equal(1, 1)\n")
    const sealedFix = run(["gap", "fix", "--state", state, "--root", dir, "--id", sealed, "--file", "src/secret.ts", "--line", "1", "--guard", "tests/gate.test.ts:2", "--commit", "fff0001"])
    assert.equal(sealedFix.status, 0, sealedFix.stderr)
    const seal = run(["seal", "--state", state, "--id", sealed])
    assert.equal(seal.status, 0, seal.stderr)
    const scores = path.join(dir, "scores.json")
    writeFileSync(scores, JSON.stringify({ visible: { before: 1, after: 4 }, sealed: { before: 2, after: 2 } }))
    status = readStatus(state, dir, page, scores)
    assert.equal(status.summary, "these tests fit the yardstick, not the code.")
    assert.equal(JSON.stringify(status).includes(sealed), false)
    assert.equal(pageText(page).includes(sealed), false)
    assert.match(pageText(page), /these tests fit the yardstick, not the code\./)
    const seeded = run(["queue", "seed", "--state", state, "--id", sealed, "--id", "visible-1"])
    assert.equal(seeded.status, 0, seeded.stderr)
    assert.equal(existsSync(path.join(state, "queue", `${sealed}.json`)), false)
    assert.equal(existsSync(path.join(state, "queue", "visible-1.json")), true)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

type Status = {
  summary: string
  recentFixes: Array<{ id: string; guard: string | null; fixedIn: string | null }>
  decisions: Array<{ review: string }>
  regressions: Array<{ id: string; text: string }>
}

function readStatus(state: string, rootDir: string, page: string, scores?: string) {
  const args = ["status", "--state", state, "--root", rootDir, "--page", page]
  if (scores) args.push("--scores", scores)
  const result = run(args)
  assert.equal(result.status, 0, result.stderr)
  return JSON.parse(result.stdout) as Status
}

function pageText(page: string) {
  return readFileSync(page, "utf8")
}

function run(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8" })
}
