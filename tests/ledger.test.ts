import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
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
    assert.equal(parsed.schemaVersion, 2)
    assert.equal(parsed.command, "ledger.build")
    assert.equal(parsed.ok, true)
    assert.equal(parsed.clean, 1)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("a release commit that bumps the version is not a fix unless it says Fixes-bug", () => {
  const pkg = repo()
  const out = path.join(pkg, "ledger")
  try {
    writeFileSync(path.join(pkg, "package.json"), '{ "name": "gate", "version": "0.1.0" }\n')
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 1\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), "export {}\n")
    commitIn(pkg, "init")
    // A feature bundle shipped with a version bump. It touches src and tests, and it is not one bug.
    writeFileSync(path.join(pkg, "package.json"), '{ "name": "gate", "version": "0.2.0" }\n')
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 2\nexport const m = 3\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), "export const t = 1\n")
    commitIn(pkg, "Set version 0.2.0 for two features")
    const release = shaOf(pkg).slice(0, 7)
    // The same shape with a trailer is a fix the author named.
    writeFileSync(path.join(pkg, "package.json"), '{ "name": "gate", "version": "0.2.1" }\n')
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 2\nexport const m = 4\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), "export const t = 2\n")
    commitIn(pkg, "Set version 0.2.1\n\nFixes-bug: m was three")
    const named = shaOf(pkg).slice(0, 7)
    const tsx = path.join(root, "node_modules", ".bin", "tsx")
    const result = spawnSync(tsx, ["src/cli.ts", "ledger", "build", "--package", pkg, "--out", out, "--max-lines", "300"], { cwd: root, encoding: "utf8" })
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const ids = readEntries(out).map((item) => item.id)
    assert.equal(ids.includes(release), false, JSON.stringify(ids))
    assert.equal(ids.includes(named), true, JSON.stringify(ids))
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("a hand-made mutant that names its fix replaces the history revert, so an unviable revert can still guard the bug", () => {
  const pkg = repo()
  const out = path.join(pkg, "ledger")
  try {
    writeFileSync(path.join(pkg, "src", "join.ts"), "export function join(a: string[]): string {\n  return a.join(\",\")\n}\n")
    writeFileSync(path.join(pkg, "tests", "join.test.ts"), "export {}\n")
    commitIn(pkg, "init")
    // The fix adds a helper and its test imports it. Reverting the whole fix removes the export, so it does not build.
    writeFileSync(path.join(pkg, "src", "join.ts"), "export function join(a: string[]): string {\n  return a.join(\"+\")\n}\nexport const SEP = \"+\"\n")
    writeFileSync(path.join(pkg, "tests", "join.test.ts"), "import { SEP } from \"../src/join.ts\"\nexport const used = SEP\n")
    commitIn(pkg, "Join with a plus")
    const fix = shaOf(pkg)
    const id = fix.slice(0, 7)
    mkdirSync(out)
    // The hand mutant keeps the export and puts the old behaviour back.
    writeFileSync(
      path.join(out, `${id}-hand.patch`),
      [
        "# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug",
        `# probatio-ledger source=hand fix=${fix}`,
        "--- a/src/join.ts",
        "+++ b/src/join.ts",
        "@@ -1,3 +1,3 @@",
        " export function join(a: string[]): string {",
        '-  return a.join("+")',
        '+  return a.join(",")',
        " }",
        "",
      ].join("\n"),
    )
    const tsx = path.join(root, "node_modules", ".bin", "tsx")
    const result = spawnSync(tsx, ["src/cli.ts", "ledger", "build", "--package", pkg, "--out", out, "--max-lines", "300"], { cwd: root, encoding: "utf8" })
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const entryFor = readEntries(out).find((item) => item.id === id)
    assert.equal(entryFor?.status, "handmade", JSON.stringify(readEntries(out)))
    assert.equal(entryFor?.patch, `${id}-hand.patch`)
    assert.equal(existsSync(path.join(out, `${id}.patch`)), false, "the history revert is not written beside the hand mutant")
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("ledger check fails when a caught ledger bug stops being caught, and re-recording needs a reason", { timeout: 300_000 }, () => {
  const pkg = repo()
  const out = path.join(pkg, "ledger")
  const tsx = path.join(root, "node_modules", ".bin", "tsx")
  const cli = (args: string[]) => {
    const result = spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", timeout: 280_000 })
    return JSON.parse(result.stdout) as { ok: boolean; summary: string; next: string; regressed?: string[]; unrecorded?: string[]; caught?: number }
  }
  try {
    writeFileSync(path.join(pkg, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n >= 0\n}\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), 'import test from "node:test"\ntest("placeholder", () => {})\n')
    commitIn(pkg, "init")
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0\n}\n")
    const guard = 'import assert from "node:assert/strict"\nimport test from "node:test"\nimport { gate } from "../src/gate.ts"\ntest("zero stays shut", () => {\n  assert.equal(gate(0), false)\n})\n'
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), guard)
    commitIn(pkg, "Keep zero shut\n\nFixes-bug: zero opened the gate")
    const id = shaOf(pkg).slice(0, 7)
    const built = spawnSync(tsx, ["src/cli.ts", "ledger", "build", "--package", pkg, "--out", out], { cwd: root, encoding: "utf8" })
    assert.equal(built.status, 0, built.stdout)
    commitIn(pkg, "ledger")
    const check = (extra: string[] = []) => cli(["ledger", "check", "--package", pkg, "--out", path.join(pkg, ".check"), ...extra])

    const first = check()
    assert.equal(first.ok, false, "an outcome with no golden is not a pass")
    assert.deepEqual(first.unrecorded, [id])
    const recorded = check(["--update"])
    assert.equal(recorded.ok, true, recorded.summary)
    assert.ok(existsSync(path.join(out, `${id}.golden.json`)))
    commitIn(pkg, "record ledger goldens")
    const held = check()
    assert.equal(held.ok, true, held.summary)
    assert.equal(held.caught, 1)

    // Someone weakens the guard. The ledger bug is back in reach of the suite.
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), 'import test from "node:test"\nimport { gate } from "../src/gate.ts"\ntest("zero stays shut", () => {\n  gate(0)\n})\n')
    commitIn(pkg, "Simplify a test")
    const broke = check()
    assert.equal(broke.ok, false, broke.summary)
    assert.deepEqual(broke.regressed, [id])
    assert.match(broke.next, /Find the test that stopped guarding it/)
    const silent = check(["--update"])
    assert.equal(silent.ok, false, "a regression is not re-recorded without a reason")
    assert.match(readFileSync(path.join(out, `${id}.golden.json`), "utf8"), /"outcome": "killed"/)
    const reasoned = check(["--update", "--message", `Golden-Change: ${id}: the guard moved to an integration test`])
    assert.equal(reasoned.ok, true, reasoned.summary)
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
