import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("a red baseline stops before a mutant that would make the broken test pass", () => {
  const marker = path.join(mkdtempSync(path.join(tmpdir(), "probatio-red-mark-")), "runs")
  const pkg = gitPackage(marker)
  const out = path.join(pkg, "out")
  try {
    const first = run(pkg, out)
    assert.equal(first.status, 1, first.stdout + first.stderr)
    const body = JSON.parse(first.stdout) as { ok: boolean; summary: string; gaps: unknown[]; kills: Array<{ killedBy?: string[] }> }
    assert.equal(body.ok, false)
    assert.match(body.summary, /baseline already failing/)
    assert.match(body.summary, /zero stays open/)
    assert.deepEqual(body.gaps, [])
    assert.deepEqual(body.kills, [])
    assert.equal(resultFiles(out).some((file) => JSON.parse(readFileSync(file, "utf8")).killedBy?.includes("zero stays open")), false)
    assert.equal(resultFiles(out).length, 0)
    assert.equal(readFileSync(marker, "utf8").trim().split("\n").length, 1)

    rmSync(marker)
    const second = run(pkg, out)
    assert.equal(second.status, 1, second.stdout + second.stderr)
    const again = JSON.parse(second.stdout) as { ok: boolean; summary: string; gaps: unknown[]; kills: unknown[] }
    assert.equal(again.ok, false)
    assert.equal(again.summary, body.summary)
    assert.deepEqual(again.gaps, [])
    assert.deepEqual(again.kills, [])
    assert.equal(existsSync(marker), false)
    assert.equal(resultFiles(out).length, 0)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
    rmSync(path.dirname(marker), { recursive: true, force: true })
  }
})

test("verify-change on a red suite returns no caught mutants", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-verify-red-"))
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(path.join(dir, "tests", "gate.test.ts"), testFile(greenTests()))
    git(dir, ["init", "-q"])
    commit(dir, "green")
    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0 && true\n}\n")
    writeFileSync(path.join(dir, "tests", "gate.test.ts"), testFile(`${greenTests()}test("zero stays open", () => { assert.equal(gate(0), true) })\n`))
    commit(dir, "red")
    const out = path.join(dir, "verify")
    const result = spawnSync(tsx, ["src/cli.ts", "verify-change", "--package", dir, "--out", out, "--base", "HEAD~1", "--commit", "HEAD", "--max-mutants", "4"], {
      cwd: root,
      encoding: "utf8",
    })
    assert.equal(result.status, 1, result.stdout + result.stderr)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; caught: unknown[] }
    assert.equal(body.ok, false)
    assert.match(body.summary, /baseline already failing/)
    assert.match(body.summary, /zero stays open/)
    assert.deepEqual(body.caught, [])
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("check-kill on a red suite does not name the broken test", () => {
  const marker = path.join(mkdtempSync(path.join(tmpdir(), "probatio-kill-red-")), "runs")
  const pkg = gitPackage(marker)
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "check-kill", "m-ge", "--package", pkg, "--out", path.join(pkg, "kill"), "--patches", path.join(pkg, "patches"), "--no-build", "--no-confirm"],
      { cwd: root, encoding: "utf8" },
    )
    assert.equal(result.status, 1, result.stdout + result.stderr)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killedBy: string[]; outcome: string | null }
    assert.equal(body.ok, false)
    assert.match(body.summary, /zero stays open/)
    assert.equal(body.outcome, null)
    assert.deepEqual(body.killedBy, [])
    assert.equal(resultFiles(path.join(pkg, "kill")).length, 0)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
    rmSync(path.dirname(marker), { recursive: true, force: true })
  }
})

test("a stored red baseline stops before the suite starts", () => {
  const marker = path.join(mkdtempSync(path.join(tmpdir(), "probatio-stored-red-")), "runs")
  const pkg = gitPackage(marker)
  const out = path.join(pkg, "stored")
  try {
    mkdirSync(out)
    writeFileSync(path.join(out, "baseline.json"), `${JSON.stringify({ ok: false, failed: ["zero stays open"], durationMs: 10 })}\n`)
    const result = run(pkg, out)
    assert.equal(result.status, 1, result.stdout + result.stderr)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; gaps: unknown[]; kills: unknown[] }
    assert.equal(body.ok, false)
    assert.match(body.summary, /baseline already failing: zero stays open/)
    assert.deepEqual(body.gaps, [])
    assert.deepEqual(body.kills, [])
    assert.equal(existsSync(marker), false)
    assert.equal(resultFiles(out).length, 0)
  } finally {
    rmSync(pkg, { recursive: true, force: true })
    rmSync(path.dirname(marker), { recursive: true, force: true })
  }
})

test("a missing compiler still stops with no gaps", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-missing-go-"))
  const out = path.join(dir, "out")
  try {
    writeFileSync(path.join(dir, "go.mod"), "module example.com/gate\n\ngo 1.22\n")
    writeFileSync(path.join(dir, "gate.go"), "package gate\n\nfunc Gate(n int) bool {\n  return n > 0\n}\n")
    writeFileSync(path.join(dir, "gate_test.go"), "package gate\n\nimport \"testing\"\n\nfunc TestGate(t *testing.T) {\n  if !Gate(1) {\n    t.Fatal(\"closed\")\n  }\n}\n")
    git(dir, ["init", "-q"])
    commit(dir, "init")
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(
      path.join(dir, "patches", "m-ge.patch"),
      forwardDiff("gate.go", readFileSync(path.join(dir, "gate.go"), "utf8"), "package gate\n\nfunc Gate(n int) bool {\n  return n >= 0\n}\n"),
    )
    const bin = path.join(dir, "bin")
    mkdirSync(bin)
    symlinkSync(process.execPath, path.join(bin, "node"))
    const gitBin = spawnSync("which", ["git"], { encoding: "utf8" })
    assert.equal(gitBin.status, 0, gitBin.stderr)
    symlinkSync(gitBin.stdout.trim(), path.join(bin, "git"))
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--patches", path.join(dir, "patches"), "--out", out, "--no-build", "--no-confirm", "--max-mutants", "1"],
      { cwd: root, encoding: "utf8", env: { ...process.env, PATH: bin } },
    )
    assert.equal(result.status, 1, result.stdout + result.stderr)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; gaps: unknown[] }
    assert.equal(body.ok, false)
    assert.match(body.summary, /did not return a test report/)
    assert.match(body.summary, /go/)
    assert.deepEqual(body.gaps, [])
    assert.equal(resultFiles(out).length, 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function run(pkg: string, out: string) {
  return spawnSync(
    tsx,
    [
      "src/cli.ts",
      "mutate", "run",
      "--package", pkg,
      "--patches", path.join(pkg, "patches"),
      "--out", out,
      "--no-build",
      "--no-confirm",
      "--max-mutants", "1",
    ],
    { cwd: root, encoding: "utf8" },
  )
}

function resultFiles(out: string): string[] {
  const dir = path.join(out, "results")
  if (!existsSync(dir)) return []
  return readdirSync(dir).filter((name) => name.endsWith(".json")).map((name) => path.join(dir, name))
}

function gitPackage(marker: string): string {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-red-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  const source = "export function gate(n: number): boolean {\n  return n > 0\n}\n"
  writeFileSync(path.join(dir, "src", "gate.ts"), source)
  writeFileSync(
    path.join(dir, "tests", "gate.test.ts"),
    `import { appendFileSync } from "node:fs"\nimport test from "node:test"\nimport assert from "node:assert/strict"\nimport { gate } from "../src/gate.ts"\nappendFileSync(${JSON.stringify(marker)}, "run\\n")\n${greenTests()}\ntest("zero stays open", () => { assert.equal(gate(0), true) })\n`,
  )
  git(dir, ["init", "-q"])
  commit(dir, "init")
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(
    path.join(dir, "patches", "m-ge.patch"),
    forwardDiff("src/gate.ts", source, "export function gate(n: number): boolean {\n  return n >= 0\n}\n"),
  )
  return dir
}

function testFile(body: string): string {
  return `import test from "node:test"\nimport assert from "node:assert/strict"\nimport { gate } from "../src/gate.ts"\n${body}`
}

function greenTests(): string {
  return `test("one opens", () => { assert.equal(gate(1), true) })
test("closed stays shut", () => { assert.equal(gate(0), false) })
`
}

function commit(repo: string, message: string): void {
  git(repo, ["add", "."])
  const result = git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", message])
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}

function git(repo: string, args: string[]) {
  return spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
}
