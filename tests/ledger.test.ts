import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { buildLedger, type LedgerEntry } from "../src/ledger/build.ts"
import { applyPatch, forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")

test("a clean revert is kept and a conflict is not fuzzy-applied", () => {
  const pkg = repo()
  const out = path.join(pkg, "ledger")
  try {
    write("src/gate.ts", "export function allow(): boolean {\n  return false\n}\n")
    write("tests/gate.test.ts", "export {}\n")
    save("init")
    write("src/gate.ts", "export function allow(): boolean {\n  return true\n}\n")
    write("tests/gate.test.ts", "export const opened = true\n")
    save("Fix: allow opens")
    const gate = head()

    write("src/other.ts", "export const n = 1\n")
    write("tests/other.test.ts", "export {}\n")
    save("add other")
    write("src/other.ts", "export const n = 2\n")
    write("tests/other.test.ts", "export const two = true\n")
    save("Fix: n is two")
    const other = head()
    write("src/other.ts", "export const value = 2\n")
    save("rename the export")

    write("src/note.ts", "export const note = 1\n")
    save("src only, not a fix")

    write("src/solo.ts", "export const solo = 0\n")
    save("solo start")
    write("src/solo.ts", "export const solo = 1\n")
    save("count the solo\n\nFixes-bug: solo started at zero")
    const solo = head()

    write("src/big.ts", "export const big = 1\n")
    write("tests/big.test.ts", "export {}\n")
    save("add big")
    write("src/big.ts", `export const big = 1\n${Array.from({ length: 20 }, (_, i) => `// line ${i}\n`).join("")}`)
    write("tests/big.test.ts", "export const grew = true\n")
    save("Fix: big grew")
    const big = head()

    const report = buildLedger({
      packageDir: pkg,
      repoDir: pkg,
      commit: "HEAD",
      outDir: out,
      srcDir: "src",
      testsDir: "tests",
      maxLines: 10,
    })
    assert.equal(report.ok, true, report.summary)
    const entries = readEntries(out)
    const by = (commit: string) => entries.find((item) => item.commit === commit)
    const gateEntry = by(gate)
    const otherEntry = by(other)
    const soloEntry = by(solo)
    const bigEntry = by(big)
    assert.equal(gateEntry?.status, "clean")
    assert.equal(otherEntry?.status, "hand")
    assert.equal(otherEntry?.reason, "patch does not apply")
    assert.equal(soloEntry?.status, "clean")
    assert.equal(soloEntry?.fixer, "human")
    assert.equal(bigEntry?.status, "skipped")
    assert.equal(entries.some((item) => item.subject === "src only, not a fix"), false)
    assert.equal(readFileSync(path.join(pkg, "src", "other.ts"), "utf8"), "export const value = 2\n")

    const patch = readFileSync(path.join(out, `${gateEntry?.id}.patch`), "utf8")
    assert.match(patch, /direction=forward/)
    assert.match(patch, /source=history/)
    assert.match(patch, new RegExp(`fix=${gate}`))
    const tree = worktree(pkg)
    try {
      const applied = applyPatch(tree, patch, "forward")
      assert.equal(applied.ok, true)
      assert.match(readFileSync(path.join(tree, "src", "gate.ts"), "utf8"), /return false/)
    } finally {
      removeWorktree(pkg, tree)
    }
    assert.equal(git(pkg, ["worktree", "list"]).stdout.trim().split("\n").length, 1)

    const again = buildLedger({
      packageDir: pkg,
      repoDir: pkg,
      commit: "HEAD",
      outDir: out,
      srcDir: "src",
      testsDir: "tests",
      maxLines: 10,
    })
    assert.equal(again.clean, report.clean)
    assert.equal(again.hand, report.hand)
    assert.deepEqual(readEntries(out).map((item) => [item.id, item.status]), entries.map((item) => [item.id, item.status]))

    const handPatch = forwardDiff("src/other.ts", "export const value = 2\n", "export const value = 0\n").replace(
      "# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug\n",
      `# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug\n# probatio-ledger source=hand fix=${other}\n`,
    )
    writeFileSync(path.join(out, `${otherEntry?.id}.patch`), handPatch)
    const kept = buildLedger({
      packageDir: pkg,
      repoDir: pkg,
      commit: "HEAD",
      outDir: out,
      srcDir: "src",
      testsDir: "tests",
      maxLines: 10,
    })
    assert.equal(kept.handmade, 1, kept.summary)
    assert.equal(readEntries(out).find((item) => item.commit === other)?.status, "handmade")
    assert.match(readFileSync(path.join(out, `${otherEntry?.id}.patch`), "utf8"), /source=hand/)
    assert.equal(git(pkg, ["status", "--short", "--", "src", "tests"]).stdout, "")
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }

  function write(rel: string, text: string) {
    const dest = path.join(pkg, rel)
    mkdirSync(path.dirname(dest), { recursive: true })
    writeFileSync(dest, text)
  }
  function save(message: string) {
    commitIn(pkg, message)
  }
  function head() {
    return shaOf(pkg)
  }
})

test("cli ledger build prints one json object", () => {
  const pkg = repo()
  const out = path.join(pkg, "ledger")
  try {
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 1\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), "export {}\n")
    commitIn(pkg, "init")
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 2\n")
    commitIn(pkg, "Fix: n\n\nFixes-bug: n was one")
    const tsx = path.join(root, "node_modules", ".bin", "tsx")
    const result = spawnSync(tsx, ["src/cli.ts", "ledger", "build", "--package", pkg, "--out", out, "--max-lines", "300"], {
      cwd: root,
      encoding: "utf8",
    })
    assert.equal(result.status, 0, result.stderr)
    const parsed = JSON.parse(result.stdout) as { ok: boolean; command: string; schemaVersion: number; clean: number }
    assert.equal(parsed.schemaVersion, 1)
    assert.equal(parsed.command, "ledger.build")
    assert.equal(parsed.ok, true)
    assert.equal(parsed.clean, 1)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("cli ledger build stops after --max-commits and says the rest was not scanned", () => {
  const pkg = repo()
  const out = path.join(pkg, "ledger")
  try {
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 1\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), "export {}\n")
    commitIn(pkg, "init")
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 2\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), "export const a = true\n")
    commitIn(pkg, "Fix: a\n\nFixes-bug: a")
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 3\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), "export const b = true\n")
    commitIn(pkg, "Fix: b\n\nFixes-bug: b")
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 4\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), "export const c = true\n")
    commitIn(pkg, "Fix: c\n\nFixes-bug: c")
    const newest = shaOf(pkg)
    const tsx = path.join(root, "node_modules", ".bin", "tsx")
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "ledger", "build", "--package", pkg, "--out", out, "--max-lines", "300", "--max-commits", "1"],
      { cwd: root, encoding: "utf8" },
    )
    const parsed = JSON.parse(result.stdout) as {
      ok: boolean
      summary: string
      historyBudgetHit: boolean
      commitsScanned: number
    }
    assert.equal(parsed.ok, true, result.stdout + result.stderr)
    assert.equal(parsed.historyBudgetHit, true)
    assert.equal(parsed.commitsScanned, 1)
    assert.match(parsed.summary, /Stopped after 1 commit/)
    assert.match(parsed.summary, /History was not fully scanned/)
    assert.deepEqual(readEntries(out).map((item) => item.commit), [newest])
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("cli ledger build does not check out a tree when history has no fix to apply", () => {
  const pkg = repo()
  const out = path.join(pkg, "ledger")
  const log = path.join(pkg, "git.log")
  const wrap = gitWrapper()
  try {
    writeFileSync(path.join(pkg, "README.md"), "one\n")
    commitIn(pkg, "readme")
    writeFileSync(path.join(pkg, "README.md"), "two\n")
    commitIn(pkg, "readme again")
    const tsx = path.join(root, "node_modules", ".bin", "tsx")
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "ledger", "build", "--package", pkg, "--out", out, "--max-lines", "300", "--max-commits", "1"],
      {
        cwd: root,
        encoding: "utf8",
        env: { ...process.env, PATH: `${wrap}${path.delimiter}${process.env.PATH ?? ""}`, GIT_LOG: log },
      },
    )
    const parsed = JSON.parse(result.stdout) as { ok: boolean; summary: string }
    assert.equal(parsed.ok, true, result.stdout + result.stderr)
    assert.match(parsed.summary, /History was not fully scanned/)
    const trace = readFileSync(log, "utf8")
    assert.equal(trace.includes("worktree"), false, trace)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
    rmSync(wrap, { recursive: true, force: true })
  }
})

function gitWrapper(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-gitwrap-"))
  const real = spawnSync("which", ["git"], { encoding: "utf8" }).stdout.trim()
  writeFileSync(path.join(dir, "git"), `#!/bin/sh\necho "$@" >> "$GIT_LOG"\nexec ${real} "$@"\n`)
  chmodSync(path.join(dir, "git"), 0o755)
  return dir
}

function readEntries(out: string): LedgerEntry[] {
  return (JSON.parse(readFileSync(path.join(out, "ledger.json"), "utf8")) as { entries: LedgerEntry[] }).entries
}

function repo(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-ledger-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  git(dir, ["init", "-q"])
  return dir
}

function commitIn(repoDir: string, message: string): void {
  git(repoDir, ["add", "."])
  const result = git(repoDir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", message])
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}

function shaOf(repoDir: string): string {
  return git(repoDir, ["rev-parse", "HEAD"]).stdout.trim()
}

function worktree(repoDir: string): string {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-ledger-wt-"))
  rmSync(dir, { recursive: true, force: true })
  const added = git(repoDir, ["worktree", "add", "--detach", "--quiet", dir, "HEAD"])
  if (added.status !== 0) throw new Error(added.stderr)
  return dir
}

function removeWorktree(repoDir: string, dir: string): void {
  git(repoDir, ["worktree", "remove", "--force", dir])
  rmSync(dir, { recursive: true, force: true })
  git(repoDir, ["worktree", "prune"])
}

function git(repoDir: string, args: string[]) {
  return spawnSync("git", ["-C", repoDir, ...args], { encoding: "utf8" })
}
