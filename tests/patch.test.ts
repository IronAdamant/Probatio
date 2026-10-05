import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { applyPatch, changedLines, chooseDirection, filesInDiff, forwardDiff, readHeader, restoreTree } from "../src/mutate/patch.ts"

test("a forward diff names its direction and applies to introduce the bug", () => {
  const repo = gitRepo()
  try {
    writeFileSync(path.join(repo, "gate.ts"), "export const open = true\n")
    commit(repo)
    const diff = forwardDiff("gate.ts", "export const open = true\n", "export const open = false\n")
    assert.match(diff, /direction=forward/)
    assert.equal(readHeader(diff).direction, "forward")
    assert.deepEqual(filesInDiff(diff), [{ file: "gate.ts", line: 1 }])
    assert.equal(chooseDirection(repo, diff, "auto"), "forward")
    assert.equal(applyPatch(repo, diff, "forward").ok, true)
    assert.equal(readFileSync(path.join(repo, "gate.ts"), "utf8"), "export const open = false\n")
  } finally {
    rmSync(repo, { recursive: true, force: true })
  }
})

test("a headerless experiment-style patch is reverse when that is the direction that applies", () => {
  const repo = gitRepo()
  try {
    writeFileSync(path.join(repo, "gate.ts"), "export const open = true\n")
    commit(repo)
    const reverse = `--- a/gate.ts
+++ b/gate.ts
@@ -1 +1 @@
-export const open = false
+export const open = true
`
    assert.equal(chooseDirection(repo, reverse, "auto"), "reverse")
    assert.equal(applyPatch(repo, reverse, "reverse").ok, true)
    assert.equal(readFileSync(path.join(repo, "gate.ts"), "utf8"), "export const open = false\n")
  } finally {
    rmSync(repo, { recursive: true, force: true })
  }
})

test("a reversed diff label still names the real file, and restore clears it", () => {
  const repo = gitRepo()
  try {
    writeFileSync(path.join(repo, "gate.ts"), "export const open = true\n")
    commit(repo)
    mkdirSync(path.join(repo, "node_modules"))
    writeFileSync(path.join(repo, "node_modules", "keep.txt"), "stay\n")
    const swapped = `diff --git b/gate.ts a/gate.ts
--- b/gate.ts
+++ a/gate.ts
@@ -1 +1 @@
-export const open = false
+export const open = true
`
    assert.deepEqual(filesInDiff(swapped), [{ file: "gate.ts", line: 1 }])
    assert.equal(applyPatch(repo, swapped, "reverse").ok, true)
    assert.equal(readFileSync(path.join(repo, "gate.ts"), "utf8"), "export const open = false\n")
    writeFileSync(path.join(repo, "leak.ts"), "leftover\n")
    restoreTree(repo)
    assert.equal(readFileSync(path.join(repo, "gate.ts"), "utf8"), "export const open = true\n")
    assert.equal(existsSync(path.join(repo, "leak.ts")), false)
    assert.equal(readFileSync(path.join(repo, "node_modules", "keep.txt"), "utf8"), "stay\n")
  } finally {
    rmSync(repo, { recursive: true, force: true })
  }
})

test("hunk lines survive a multi-file git diff", () => {
  const raw = `diff --git a/pkg/src/job.ts b/pkg/src/job.ts
--- a/pkg/src/job.ts
+++ b/pkg/src/job.ts
@@ -10,3 +12,4 @@
 context
-old
+new
diff --git a/pkg/src/receipt.ts b/pkg/src/receipt.ts
--- a/pkg/src/receipt.ts
+++ b/pkg/src/receipt.ts
@@ -2,1 +2,1 @@
-a
+b
`
  assert.deepEqual(filesInDiff(raw), [
    { file: "pkg/src/job.ts", line: 12 },
    { file: "pkg/src/receipt.ts", line: 2 },
  ])
})

test("changed lines are the removed lines, not the hunk start", () => {
  const raw = `--- a/tests/main.c
+++ b/tests/main.c
@@ -1,6 +1,6 @@
 int gate(int n) { return n > 0 && n < 10; }
-int unused(int n) { return n > 2 && n < 3; }
+int unused(int n) { return n >= 2 && n < 3; }
 int main(void) {
`
  assert.deepEqual(changedLines(raw), [{ file: "tests/main.c", line: 2 }])
})

function gitRepo(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-patch-"))
  git(dir, ["init", "-q"])
  return dir
}

function commit(repo: string): void {
  git(repo, ["add", "."])
  const result = git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  if (result.status !== 0) throw new Error(result.stderr)
}

function git(repo: string, args: string[]) {
  return spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
}
