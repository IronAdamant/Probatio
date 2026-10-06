import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { sealedTreeLeaked, type SealedLabel } from "../src/mutate/sealed.ts"

const label: SealedLabel = { bugId: "bug-sealed-1", issueId: "issue-sealed-1", expectedFail: "zero stays shut" }

test("a sealed label in a result file on disk is a leak", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-sealed-tree-"))
  try {
    assert.equal(sealedTreeLeaked(dir, label), false)
    mkdirSync(path.join(dir, "results"))
    writeFileSync(path.join(dir, "results", "m1.json"), `{"next":"${label.bugId}"}\n`)
    assert.equal(sealedTreeLeaked(dir, label), true)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
