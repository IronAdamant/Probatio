import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { render } from "../src/contract.ts"
import { generateMutants } from "../src/mutate/generate.ts"
import { mutantId } from "../src/mutate/ids.ts"
import { runMutants } from "../src/mutate/run.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")

test("ids follow the file, the span, and the operator", () => {
  assert.equal(mutantId("src/gate.ts", 4, 8, "and-to-or"), mutantId("src/gate.ts", 4, 8, "and-to-or"))
  assert.notEqual(mutantId("src/gate.ts", 4, 8, "and-to-or"), mutantId("src/gate.ts", 4, 8, "or-to-and"))
})

test("generate writes forward patches and no string sentinel", () => {
  const pkg = gitPackage()
  const out = path.join(pkg, "out")
  try {
    const result = generateMutants({
      packageDir: pkg,
      srcDir: "src",
      outDir: out,
      perFile: null,
      skip: 0,
      seed: 20261003,
      maxMutants: null,
      skipFiles: [],
    })
    assert.equal(result.violations.length, 0)
    assert.ok(result.mutants.length >= 2)
    assert.ok(result.mutants.every((mutant) => mutant.file === "src/gate.ts"))
    const again = generateMutants({
      packageDir: pkg,
      srcDir: "src",
      outDir: out,
      perFile: null,
      skip: 0,
      seed: 20261003,
      maxMutants: null,
      skipFiles: [],
    })
    assert.deepEqual(again.mutants.map((mutant) => mutant.id), result.mutants.map((mutant) => mutant.id))
    const blob = result.mutants.map((mutant) => readFileSync(path.join(out, "mutants", mutant.patch), "utf8")).join("\n")
    const changed = blob.split("\n").filter((line) => /^[+-]/.test(line) && !line.startsWith("+++") && !line.startsWith("---"))
    assert.equal(changed.some((line) => line.includes("SENTINEL_STRING_SHOULD_STAY")), false)
    assert.match(blob, /direction=forward/)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("the cli prints one json object and hides the home directory", () => {
  const home = "/Users/example"
  const text = render(
    {
      schemaVersion: 1,
      ok: true,
      command: "mutate.generate",
      summary: "1 mutant.",
      next: "Run it.",
      nextCall: { argv: ["mutate", "run", "--package", `${home}/pkg`] },
    },
    false,
    home,
  )
  const parsed = JSON.parse(text) as { schemaVersion: number; nextCall: { argv: string[] } }
  assert.equal(parsed.schemaVersion, 1)
  assert.equal(parsed.nextCall.argv[3], "~/pkg")
  assert.equal(text.includes(home), false)
})

test("mutate run confirms kills and reports an untested line as no coverage", async () => {
  const pkg = gitPackage()
  const generated = path.join(pkg, "generated")
  const out = path.join(pkg, "runs")
  try {
    const batch = generateMutants({
      packageDir: pkg,
      srcDir: "src",
      outDir: generated,
      perFile: null,
      skip: 0,
      seed: 1,
      maxMutants: null,
      skipFiles: [],
    })
    const report = await runMutants({
      packageDir: pkg,
      repoDir: pkg,
      commit: "HEAD",
      patchDirs: [path.join(generated, "mutants")],
      outDir: out,
      direction: "auto",
      workers: 1,
      concurrency: 1,
      maxMinutes: null,
      maxMutants: null,
      build: null,
      testsDir: "tests",
      unset: [],
      suiteTimeoutMs: 60_000,
      testTimeoutMs: 10_000,
      confirm: true,
      affected: false,
      agent: "test",
    })
    assert.equal(report.ok, true, report.summary)
    assert.match(report.summary, new RegExp(`^${report.noCoverage} no coverage, ${report.survived} survived`))
    assert.ok(report.killed >= 1, report.summary)
    assert.ok(report.noCoverage >= 1, report.summary)
    assert.equal(report.errors, 0, report.summary)
    assert.equal(report.flaky, 0, report.summary)
    const untouched = batch.mutants.find((mutant) => mutant.op === "true-to-false" && mutant.line > 4)
    assert.ok(untouched)
    const saved = JSON.parse(readFileSync(path.join(out, "results", `${untouched.id}.json`), "utf8")) as { outcome: string; next: string; command: string }
    assert.equal(saved.outcome, "no coverage")
    assert.match(saved.next, /No coverage/)
    assert.equal(report.gaps.some((gap) => gap.id === untouched.id), false)
    assert.equal(report.kills.some((item) => item.id === untouched.id), false)
    const killed = batch.mutants.find((mutant) => mutant.op === "and-to-or")
    assert.ok(killed)
    const kill = JSON.parse(readFileSync(path.join(out, "results", `${killed.id}.json`), "utf8")) as { outcome: string; killedBy: string[] }
    assert.equal(kill.outcome, "killed")
    assert.ok(kill.killedBy.some((item) => item.includes("allows the open range")))
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("a file that does not exit is a confirmed kill", async () => {
  const pkg = gitPackage()
  const out = path.join(pkg, "runs")
  try {
    writeFileSync(path.join(pkg, "src", "hold.ts"), "export function hold(): void {}\n")
    writeFileSync(
      path.join(pkg, "tests", "hold.test.ts"),
      `import test from "node:test"
import assert from "node:assert/strict"
import { hold } from "../src/hold.ts"
test("releases the process", () => {
  hold()
  assert.equal(1, 1)
})
`,
    )
    commit(pkg, "hold")
    const report = await runMutants({
      packageDir: pkg,
      repoDir: pkg,
      commit: "HEAD",
      patchDirs: [hangPatch(pkg)],
      outDir: out,
      direction: "forward",
      workers: 1,
      concurrency: 1,
      maxMinutes: null,
      maxMutants: null,
      build: null,
      testsDir: "tests",
      unset: [],
      suiteTimeoutMs: 30_000,
      testTimeoutMs: 1_500,
      confirm: true,
      affected: false,
      agent: "test",
    })
    assert.equal(report.ok, true, report.summary)
    assert.equal(report.killed, 1, report.summary)
    assert.equal(report.flaky, 0, report.summary)
    assert.equal(report.survived, 0, report.summary)
    assert.equal(report.timeouts, 0, report.summary)
    const saved = JSON.parse(readFileSync(path.join(out, "results", "m-hang.json"), "utf8")) as { outcome: string; killedBy: string[] }
    assert.equal(saved.outcome, "killed")
    assert.ok(saved.killedBy.some((item) => item.includes("hold.test.ts")))
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("duplicate titles stop the run", async () => {
  const pkg = gitPackage()
  try {
    writeFileSync(
      path.join(pkg, "tests", "dup.test.ts"),
      'import test from "node:test"\ntest("same", () => {})\ntest("same", () => {})\n',
    )
    commit(pkg, "dup")
    const report = await runMutants({
      packageDir: pkg,
      repoDir: pkg,
      commit: "HEAD",
      patchDirs: [writeDummyPatch(pkg)],
      outDir: path.join(pkg, "runs"),
      direction: "forward",
      workers: 1,
      concurrency: 1,
      maxMinutes: null,
      maxMutants: null,
      build: null,
      testsDir: "tests",
      unset: [],
      suiteTimeoutMs: 30_000,
      testTimeoutMs: 10_000,
      confirm: true,
      affected: false,
      agent: null,
    })
    assert.equal(report.ok, false)
    assert.match(report.summary, /duplicate test title/)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("cli generate is one json object on stdout", () => {
  const pkg = gitPackage()
  const out = path.join(pkg, "out")
  try {
    const tsx = path.join(root, "node_modules", ".bin", "tsx")
    const result = spawnSync(tsx, ["src/cli.ts", "mutate", "generate", "--package", pkg, "--out", out, "--no-build"], {
      cwd: root,
      encoding: "utf8",
    })
    assert.equal(result.status, 0, result.stderr)
    const parsed = JSON.parse(result.stdout) as { ok: boolean; schemaVersion: number; stringLiteralMutants: number; command: string }
    assert.equal(parsed.schemaVersion, 1)
    assert.equal(parsed.ok, true)
    assert.equal(parsed.command, "mutate.generate")
    assert.equal(parsed.stringLiteralMutants, 0)
    assert.equal(result.stdout.includes("SENTINEL_STRING_SHOULD_STAY"), false)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
  }
})

test("mutate run uses the main checkout node_modules when the worktree has none", () => {
  const pkg = mkdtempSync(path.join(tmpdir(), "probatio-deps-"))
  const wt = mkdtempSync(path.join(tmpdir(), "probatio-deps-wt-"))
  const out = mkdtempSync(path.join(tmpdir(), "probatio-deps-out-"))
  rmSync(wt, { recursive: true, force: true })
  try {
    mkdirSync(path.join(pkg, "src"))
    mkdirSync(path.join(pkg, "tests"))
    writeFileSync(path.join(pkg, "package.json"), '{"type":"module"}\n')
    writeFileSync(path.join(pkg, ".gitignore"), "node_modules\n")
    writeFileSync(path.join(pkg, "src", "gate.js"), "export function gate(n) {\n  return n > 0\n}\n")
    writeFileSync(
      path.join(pkg, "tests", "gate.test.js"),
      'import test from "node:test"\nimport assert from "node:assert/strict"\nimport { value } from "fixture-dep"\nimport { gate } from "../src/gate.js"\ntest("dep loads", () => {\n  assert.equal(value, 1)\n  assert.equal(gate(1), true)\n})\n',
    )
    git(pkg, ["init", "-q"])
    commit(pkg, "init")
    mkdirSync(path.join(pkg, "node_modules", "fixture-dep"), { recursive: true })
    writeFileSync(path.join(pkg, "node_modules", "fixture-dep", "package.json"), '{"type":"module"}\n')
    writeFileSync(path.join(pkg, "node_modules", "fixture-dep", "index.js"), "export const value = 1\n")
    const added = git(pkg, ["worktree", "add", "--detach", "--quiet", wt, "HEAD"])
    assert.equal(added.status, 0, added.stderr)
    const patches = path.join(out, "patches")
    mkdirSync(patches)
    writeFileSync(
      path.join(patches, "m-dep.patch"),
      "--- a/src/gate.js\n+++ b/src/gate.js\n@@ -1,3 +1,3 @@\n export function gate(n) {\n-  return n > 0\n+  return n >= 0\n }\n",
    )
    const tsx = path.join(root, "node_modules", ".bin", "tsx")
    const result = spawnSync(
      tsx,
      [
        "src/cli.ts",
        "mutate",
        "run",
        "--package",
        wt,
        "--repo",
        wt,
        "--patches",
        patches,
        "--out",
        path.join(out, "run"),
        "--no-build",
        "--max-mutants",
        "1",
        "--max-minutes",
        "1",
        "--suite-timeout-ms",
        "20000",
      ],
      { cwd: root, encoding: "utf8" },
    )
    const parsed = JSON.parse(result.stdout) as { ok: boolean; summary: string }
    assert.equal(parsed.summary.includes("suite already failing"), false, parsed.summary)
    assert.equal(parsed.ok, true, parsed.summary)
    assert.match(parsed.summary, /1 survived/)
  } finally {
    git(pkg, ["worktree", "remove", "--force", wt])
    rmSync(wt, { recursive: true, force: true })
    rmSync(pkg, { recursive: true, force: true })
    rmSync(out, { recursive: true, force: true })
  }
})

test("cli mutate run does not check out a tree when the package has no tests", () => {
  const pkg = mkdtempSync(path.join(tmpdir(), "probatio-norun-"))
  const patches = path.join(pkg, "patches")
  const out = mkdtempSync(path.join(tmpdir(), "probatio-norun-out-"))
  const log = path.join(out, "git.log")
  const wrap = mkdtempSync(path.join(tmpdir(), "probatio-gitwrap-"))
  try {
    mkdirSync(path.join(pkg, "src"))
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export const n = 1\n")
    mkdirSync(patches)
    writeFileSync(path.join(patches, "m-dummy.patch"), "not a diff\n")
    git(pkg, ["init", "-q"])
    commit(pkg, "init")
    const real = spawnSync("which", ["git"], { encoding: "utf8" }).stdout.trim()
    writeFileSync(path.join(wrap, "git"), `#!/bin/sh\necho "$@" >> "$GIT_LOG"\nexec ${real} "$@"\n`)
    chmodSync(path.join(wrap, "git"), 0o755)
    const tsx = path.join(root, "node_modules", ".bin", "tsx")
    const result = spawnSync(
      tsx,
      [
        "src/cli.ts",
        "mutate",
        "run",
        "--package",
        pkg,
        "--patches",
        patches,
        "--out",
        out,
        "--no-build",
        "--max-mutants",
        "1",
        "--max-minutes",
        "1",
      ],
      {
        cwd: root,
        encoding: "utf8",
        env: { ...process.env, PATH: `${wrap}${path.delimiter}${process.env.PATH ?? ""}`, GIT_LOG: log },
      },
    )
    const parsed = JSON.parse(result.stdout) as { ok: boolean; summary: string }
    assert.equal(parsed.ok, false)
    assert.equal(parsed.summary, "no tests in tests")
    const trace = readFileSync(log, "utf8")
    assert.equal(trace.includes("worktree"), false, trace)
    assert.equal(git(pkg, ["status", "--porcelain"]).stdout, "")
  } finally {
    rmSync(pkg, { recursive: true, force: true })
    rmSync(out, { recursive: true, force: true })
    rmSync(wrap, { recursive: true, force: true })
  }
})

function gitPackage(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-pkg-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(
    path.join(dir, "src", "gate.ts"),
    `export function allow(n: number): boolean {
  if (n > 0 && n < 10) return true
  return false
}
export function leftover(): boolean {
  return true
}
const label = "SENTINEL_STRING_SHOULD_STAY true && false"
`,
  )
  writeFileSync(
    path.join(dir, "tests", "gate.test.ts"),
    `import test from "node:test"
import assert from "node:assert/strict"
import { allow } from "../src/gate.ts"
test("allows the open range", () => {
  assert.equal(allow(1), true)
  assert.equal(allow(0), false)
  assert.equal(allow(10), false)
})
`,
  )
  git(dir, ["init", "-q"])
  commit(dir, "init")
  return dir
}

function hangPatch(pkg: string): string {
  const dir = path.join(pkg, "patches")
  mkdirSync(dir)
  const original = path.join(pkg, "src", "hold.ts")
  const mutated = path.join(pkg, "hold.mutated.ts")
  writeFileSync(mutated, "export function hold(): void {\n  setInterval(() => {}, 1000)\n}\n")
  const diff = spawnSync("diff", ["-u", "-L", "a/src/hold.ts", "-L", "b/src/hold.ts", original, mutated], { encoding: "utf8" })
  if (!diff.stdout.includes("setInterval")) throw new Error(diff.stderr || "hang patch was empty")
  const body = `# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug\n${diff.stdout}`
  writeFileSync(path.join(dir, "m-hang.patch"), body)
  rmSync(mutated)
  return dir
}

function writeDummyPatch(pkg: string): string {
  const dir = path.join(pkg, "patches")
  mkdirSync(dir)
  const diff = `--- a/src/gate.ts
+++ b/src/gate.ts
@@ -1,1 +1,1 @@
-export function allow(n: number): boolean {
+export function allow(n: number): boolean {
`
  // A no-op hunk will not apply. The duplicate-title check happens before any patch is applied.
  writeFileSync(path.join(dir, "m-dummy.patch"), diff)
  return dir
}

function commit(repo: string, message: string): void {
  git(repo, ["add", "."])
  const result = git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", message])
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}

function git(repo: string, args: string[]) {
  return spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
}
