import assert from "node:assert/strict"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { probatioWorktreeDir } from "../src/mutate/run.ts"

test("only a probatio temp directory may be removed as a worktree", () => {
  const ok = path.join(tmpdir(), "probatio-abcd1234-0")
  assert.equal(probatioWorktreeDir(ok), true)
  assert.equal(probatioWorktreeDir(path.join(ok, "..", "other")), false)
  assert.equal(probatioWorktreeDir("/etc"), false)
  assert.equal(probatioWorktreeDir(path.join(tmpdir(), "probatio-not-hex-0")), false)
})
