import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("mutate generate stops when the time budget is already spent", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-budget-"))
  const out = path.join(dir, "out")
  try {
    mkdirSync(path.join(dir, "src"))
    for (let index = 0; index < 12; index++) {
      writeFileSync(
        path.join(dir, "src", `gate-${index}.ts`),
        "export function gate(n: number): boolean {\n  return n > 0\n}\n",
      )
    }
    const spent = launch(dir, out, ["--max-minutes", "0", "--max-mutants", "4"])
    assert.equal(spent.status, 0, spent.stderr)
    const stopped = JSON.parse(spent.stdout) as {
      schemaVersion: number
      ok: boolean
      next: string
      nextCall: unknown
      budgetHit: boolean
      filesVisited: number
      mutantCount: number
      summary: string
    }
    assert.equal(stopped.schemaVersion, 2)
    assert.equal(stopped.ok, true)
    assert.equal(typeof stopped.next, "string")
    assert.equal(stopped.budgetHit, true)
    assert.equal(stopped.filesVisited, 0)
    assert.equal(stopped.mutantCount, 0)
    assert.match(stopped.summary, /time budget/)
    assert.equal(stopped.nextCall, null)

    const open = launch(dir, path.join(dir, "open"), ["--max-mutants", "4"])
    assert.equal(open.status, 0, open.stderr)
    const full = JSON.parse(open.stdout) as { budgetHit: boolean; mutantCount: number; filesVisited: number }
    assert.equal(full.budgetHit, false)
    assert.equal(full.filesVisited, 12)
    assert.ok(full.mutantCount > 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function launch(dir: string, out: string, extra: string[]) {
  return spawnSync(tsx, ["src/cli.ts", "mutate", "generate", "--package", dir, "--src", "src", "--out", out, ...extra], {
    cwd: root,
    encoding: "utf8",
  })
}
