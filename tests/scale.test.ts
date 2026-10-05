import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { scaleStop } from "../src/mutate/suite-decision.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("line map kinds are not stopped, and the slow unmapped boundary is exact", () => {
  for (const kind of ["node", "pytest", "c", "go", "maven", "cargo", "dotnet"]) {
    assert.equal(scaleStop({ kind, baselineMs: 70_000, pending: 400 }).action, "run")
  }
  assert.equal(scaleStop({ kind: "go", baselineMs: 1_052, pending: 251 }).action, "run")
  assert.equal(scaleStop({ kind: "maven", baselineMs: 4_999, pending: 100 }).action, "run")
  assert.equal(scaleStop({ kind: "command", baselineMs: 5_000, pending: 30 }).action, "run")
  for (const kind of ["swift", "cobol"]) {
    const refused = scaleStop({ kind, baselineMs: 5_000, pending: 31 })
    assert.equal(refused.action, "stop")
  }
  const stop = scaleStop({ kind: "swift", baselineMs: 5_000, pending: 31 })
  assert.equal(stop.action, "stop")
  if (stop.action === "stop") {
    assert.match(stop.next, /No line map/)
    assert.match(stop.next, /5\.0s/)
    assert.match(stop.next, /Nothing was started/)
  }
})

test("verify-change runs the tests that hit the edit, and still runs when nothing imports the file", { timeout: 90_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-scale-verify-"))
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(path.join(dir, "src", "alone.ts"), "export function alone(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(
      path.join(dir, "tests", "gate.test.ts"),
      [
        "import test from \"node:test\"",
        "import assert from \"node:assert/strict\"",
        "import { gate } from \"../src/gate.ts\"",
        "test(\"positive opens\", () => { assert.equal(gate(1), true) })",
        "test(\"zero stays shut\", () => { assert.equal(gate(0), false) })",
        "",
      ].join("\n"),
    )
    writeFileSync(
      path.join(dir, "tests", "other.test.ts"),
      ["import test from \"node:test\"", "import assert from \"node:assert/strict\"", "test(\"unrelated stays\", () => { assert.equal(1, 1) })", ""].join("\n"),
    )
    commit(dir, "init")
    const generated = cli(["mutate", "generate", "--package", dir, "--src", "src", "--out", path.join(dir, "gen"), "--max-minutes", "1"])
    assert.equal(generated.status, 0, generated.stderr)
    const gen = JSON.parse(generated.stdout) as { next: string; mutantCount: number }
    assert.ok(gen.mutantCount > 0)
    assert.match(gen.next, new RegExp(`${gen.mutantCount} mutants`))
    assert.match(gen.next, /node --test/)
    assert.match(gen.next, /line map/)

    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0 || n < -1000\n}\n")
    commit(dir, "edit gate")
    const verified = cli(["verify-change", "--package", dir, "--out", path.join(dir, "verify"), "--base", "HEAD~1", "--commit", "HEAD", "--max-mutants", "4"])
    assert.equal(verified.status, 0, verified.stderr + verified.stdout)
    const report = JSON.parse(verified.stdout) as VerifyReport
    assert.equal(report.ok, true)
    assert.match(report.summary, /^\d+ no coverage, \d+ missed/)
    assert.deepEqual(report.ran, ["tests/gate.test.ts"])
    assert.ok(report.caught.length > 0)
    assert.ok(report.missed.length > 0)
    const commands = readdirSync(path.join(dir, "verify", "run", "results")).filter((name) => name.endsWith(".json"))
    assert.ok(commands.length > 0)
    const bodies = commands.map((name) => JSON.parse(readFileSync(path.join(dir, "verify", "run", "results", name), "utf8")) as { command?: string; outcome?: string })
    const ranCommand = bodies.find((item) => item.outcome === "killed" || item.outcome === "survived")
    assert.ok(ranCommand?.command, JSON.stringify(bodies))
    assert.match(ranCommand.command, /gate\.test\.ts/)
    assert.equal(ranCommand.command.includes("other.test.ts"), false, ranCommand.command)
    assert.match(ranCommand.command, /positive opens|zero stays shut/)

    const tallied = cli(["mutate", "tally", "--out", path.join(dir, "verify", "run")])
    assert.equal(tallied.status, 0, tallied.stderr + tallied.stdout)
    const tally = JSON.parse(tallied.stdout) as TallyReport
    assert.equal(tally.ok, true)
    assert.ok(tally.keep.length > 0, tally.summary)
    assert.ok(tally.gaps.length > 0, tally.summary)
    assert.equal(tally.pruning.deletedTests, 0)
    assert.equal(tally.pruning.mode, "advisory")
    assert.equal(existsSync(path.join(dir, "tests", "gate.test.ts")), true)
    assert.equal(existsSync(path.join(dir, "tests", "other.test.ts")), true)

    writeFileSync(path.join(dir, "src", "alone.ts"), "export function alone(n: number): boolean {\n  return n > 0 || n < -1000\n}\n")
    commit(dir, "edit alone")
    const uncovered = cli(["verify-change", "--package", dir, "--out", path.join(dir, "alone"), "--base", "HEAD~1", "--commit", "HEAD", "--max-mutants", "8"])
    assert.equal(uncovered.status, 0, uncovered.stderr + uncovered.stdout)
    const alone = JSON.parse(uncovered.stdout) as VerifyReport
    assert.deepEqual(alone.ran, [])
    assert.match(uncovered.stderr, /baseline/)
    const attempted = alone.caught.length + alone.missed.length + alone.other.length
    assert.ok(attempted > 0, alone.summary)
    assert.ok(alone.other.some((item) => item.outcome === "no coverage"), alone.summary)
    assert.match(alone.summary, /no coverage/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("tally names a test that killed nothing and does not delete it", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-scale-tally-"))
  const results = path.join(dir, "results")
  try {
    mkdirSync(results)
    mkdirSync(path.join(dir, "tests"))
    const planted = path.join(dir, "tests", "never.test.ts")
    writeFileSync(planted, "test(\"never killed\", () => {})\n")
    writeFileSync(
      path.join(results, "m-killed.json"),
      `${JSON.stringify({ id: "m-killed", outcome: "killed", killedBy: ["tests/gate.test.ts::positive opens"], files: [{ file: "src/gate.ts", line: 2 }], selectedTests: ["positive opens"] })}\n`,
    )
    writeFileSync(
      path.join(results, "m-lived.json"),
      `${JSON.stringify({ id: "m-lived", outcome: "survived", killedBy: [], files: [{ file: "src/gate.ts", line: 4 }], selectedTests: ["never killed"] })}\n`,
    )
    writeFileSync(
      path.join(results, "m-dark.json"),
      `${JSON.stringify({ id: "m-dark", outcome: "no coverage", killedBy: [], files: [{ file: "src/alone.ts", line: 2 }], selectedTests: [] })}\n`,
    )
    writeFileSync(
      path.join(results, "m-slow.json"),
      `${JSON.stringify({ id: "m-slow", outcome: "timeout", killedBy: [], files: [{ file: "src/gate.ts", line: 3 }], selectedTests: ["positive opens"] })}\n`,
    )
    writeFileSync(
      path.join(results, "m-build.json"),
      `${JSON.stringify({ id: "m-build", outcome: "killed", killedBy: ["tsc"], files: [{ file: "src/gate.ts", line: 1 }], selectedTests: [] })}\n`,
    )
    writeFileSync(
      path.join(dir, "coverage-map.json"),
      `${JSON.stringify({ files: { "src/gate.ts": { "2": ["positive opens", "never killed"] } } })}\n`,
    )
    const result = cli(["mutate", "tally", "--out", dir])
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as TallyReport
    assert.equal(body.command, "mutate.tally")
    assert.ok(body.keep.some((name) => name.includes("positive opens")), body.summary)
    assert.ok(body.drop.includes("never killed"), JSON.stringify(body.drop))
    assert.equal(body.drop.includes("tsc"), false)
    assert.equal(body.keep.includes("tsc"), false)
    assert.equal(body.pruning.deletedTests, 0)
    assert.equal(body.pruning.advice.includes("never killed"), true)
    assert.equal(body.gaps.length, 1)
    assert.ok(body.gaps.some((gap) => gap.id === "m-lived" && gap.outcome === "survived"))
    assert.equal(body.gaps.some((gap) => gap.id === "m-dark" || gap.id === "m-slow"), false)
    assert.match(body.next ?? "", /Timed out/)
    assert.match(body.summary, /^1 no coverage, 1 survived/)
    assert.match(body.next ?? "", /not a pass/)
    assert.match(body.next ?? "", /not a gap/)
    assert.match(body.summary, /No test file was deleted/)
    assert.equal(readFileSync(planted, "utf8").includes("never killed"), true)
    assert.equal(existsSync(path.join(results, "m-lived.json")), true)
    const stored = JSON.parse(readFileSync(path.join(dir, "tally.json"), "utf8")) as { pruning: { deletedTests: number } }
    assert.equal(stored.pruning.deletedTests, 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a slow suite with no line map does not start a large batch", { timeout: 60_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-scale-stop-"))
  try {
    mkdirSync(path.join(dir, "src"))
    writeFileSync(path.join(dir, "src", "gates.ts"), manyGates(12))
    commit(dir, "init")
    const generated = cli(["mutate", "generate", "--package", dir, "--src", "src", "--out", path.join(dir, "gen"), "--max-minutes", "1"])
    assert.equal(generated.status, 0, generated.stderr)
    const gen = JSON.parse(generated.stdout) as { ok: boolean; next: string; mutantCount: number }
    assert.equal(gen.ok, true)
    assert.ok(gen.mutantCount > 30, String(gen.mutantCount))
    assert.match(gen.next, /No suite was discovered/)
    const result = cli([
      "mutate",
      "run",
      "--package",
      dir,
      "--repo",
      dir,
      "--patches",
      path.join(dir, "gen", "mutants"),
      "--out",
      path.join(dir, "out"),
      "--no-build",
      "--workers",
      "1",
      "--no-confirm",
      "--suite-command",
      "sleep 6",
      "--suite-timeout-ms",
      "60000",
    ])
    assert.equal(result.status, 1, result.stdout)
    const body = JSON.parse(result.stdout) as RunReport
    assert.equal(body.ok, false)
    assert.match(body.next, /No line map/)
    assert.match(body.next, /Nothing was started/)
    assert.match(body.summary, /No line map/)
    assert.ok((body.baselineMs ?? 0) >= 5_000, String(body.baselineMs))
    assert.ok((body.notStarted ?? []).length > 30)
    assert.equal(body.killed, 0)
    assert.equal(body.survived, 0)
    assert.equal(/m[0-9a-f]{12} start/.test(result.stderr), false, result.stderr)
    assert.equal(existsSync(path.join(dir, "out", "baseline.json")), true)
    assert.equal(existsSync(path.join(dir, "out", "results")), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a fast suite with no line map still runs", { timeout: 60_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-scale-fast-"))
  try {
    mkdirSync(path.join(dir, "src"))
    writeFileSync(path.join(dir, "src", "gates.ts"), manyGates(12))
    commit(dir, "init")
    const generated = cli(["mutate", "generate", "--package", dir, "--src", "src", "--out", path.join(dir, "gen"), "--max-minutes", "1"])
    assert.equal(generated.status, 0, generated.stderr)
    const gen = JSON.parse(generated.stdout) as { mutantCount: number }
    assert.ok(gen.mutantCount > 30)
    const result = cli([
      "mutate",
      "run",
      "--package",
      dir,
      "--repo",
      dir,
      "--patches",
      path.join(dir, "gen", "mutants"),
      "--out",
      path.join(dir, "out"),
      "--no-build",
      "--workers",
      "1",
      "--no-confirm",
      "--budget-ms",
      "1",
      "--suite-command",
      "true",
      "--suite-timeout-ms",
      "30000",
    ])
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as RunReport
    assert.equal(body.ok, true, body.summary)
    assert.equal(/No line map/.test(body.next), false, body.next)
    assert.match(result.stderr, /m[0-9a-f]{12} start/)
    assert.ok((body.killed ?? 0) + (body.survived ?? 0) >= 1, body.summary)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

type VerifyReport = {
  ok: boolean
  summary: string
  ran: string[]
  caught: unknown[]
  missed: unknown[]
  other: Array<{ outcome: string }>
}

type TallyReport = {
  ok: boolean
  command: string
  summary: string
  keep: string[]
  drop: string[]
  next?: string
  gaps: Array<{ id: string; outcome: string }>
  pruning: { mode: string; deletedTests: number; advice: string[] }
}

type RunReport = {
  ok: boolean
  summary: string
  next: string
  killed?: number
  survived?: number
  baselineMs?: number | null
  notStarted?: string[]
}

function manyGates(count: number): string {
  const parts: string[] = []
  for (let index = 0; index < count; index++) {
    parts.push(`export function gate${index}(n: number): boolean {\n  return n > 0 && n < 10\n}\n`)
  }
  return parts.join("\n")
}

function cli(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8" })
}

function commit(repo: string, message: string) {
  const git = (args: string[]) => {
    const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
    if (result.status !== 0) throw new Error(result.stderr || result.stdout)
  }
  git(["init", "-q"])
  git(["add", "."])
  git(["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", message])
}
