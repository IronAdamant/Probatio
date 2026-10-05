import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("verify-change lists affected tests, caught and missed mutants, and golden contract rows", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-verify-"))
  const out = path.join(dir, "out")
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "golden"))
    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0\n}\n")
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
    const golden = {
      rows: {
        gate: { ok: true, reason: "opened", next: "Go on." },
        copy: { ok: true, reason: "same", next: "Stay." },
      },
    }
    writeFileSync(path.join(dir, "golden", "rows.golden.json"), `${JSON.stringify(golden, null, 2)}\n`)
    commit(dir, "init")
    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0 || n < -1000\n}\n")
    golden.rows.gate.ok = false
    golden.rows.gate.reason = "shut"
    golden.rows.copy.next = "Remain."
    writeFileSync(path.join(dir, "golden", "rows.golden.json"), `${JSON.stringify(golden, null, 2)}\n`)
    commit(dir, "edit gate")
    const result = spawnSync(tsx, ["src/cli.ts", "verify-change", "--package", dir, "--out", out, "--base", "HEAD~1", "--commit", "HEAD", "--max-mutants", "4"], {
      cwd: root,
      encoding: "utf8",
    })
    assert.equal(result.status, 0, result.stderr)
    const report = JSON.parse(result.stdout) as Report
    assert.equal(report.ok, true)
    assert.ok(report.affected.includes("tests/gate.test.ts"))
    assert.deepEqual(report.ran, ["tests/gate.test.ts"])
    assert.equal(report.rest, 0)
    assert.deepEqual(report.notRun, [])
    assert.ok(report.caught.length > 0)
    assert.ok(report.missed.length > 0)
    assert.ok(report.caught.every((item) => item.file === "src/gate.ts" && item.line > 0))
    assert.ok(report.missed.every((item) => item.file === "src/gate.ts" && item.line > 0))
    assert.ok(report.goldenContract.some((item) => item.row === "gate" && item.fields.includes("ok")))
    assert.equal(report.goldenContract.some((item) => item.row === "copy"), false)
    const ranIds = new Set([...report.caught, ...report.missed].map((item) => item.id))
    const capped = spawnSync(tsx, ["src/cli.ts", "verify-change", "--package", dir, "--out", path.join(dir, "capped"), "--base", "HEAD~1", "--commit", "HEAD", "--max-mutants", "1"], {
      cwd: root,
      encoding: "utf8",
    })
    assert.equal(capped.status, 0, capped.stderr)
    const narrow = JSON.parse(capped.stdout) as Report
    assert.ok(narrow.rest > 0)
    assert.equal(narrow.notRun.length, narrow.rest)
    assert.match(narrow.summary, new RegExp(`${narrow.rest} not run`))
    for (const item of narrow.notRun) {
      assert.equal(ranIds.has(item.id), true)
      assert.equal(item.file, "src/gate.ts")
      assert.ok(item.line > 0)
      assert.equal(typeof item.op, "string")
      assert.ok(item.op.length > 0)
    }
    const shown = new Set([...narrow.caught, ...narrow.missed].map((item) => item.id))
    for (const item of narrow.notRun) assert.equal(shown.has(item.id), false)
    assert.equal(narrow.caught.length + narrow.missed.length + narrow.rest, report.caught.length + report.missed.length)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("verify-change uses the generate finder on a non-TypeScript diff and still emits TypeScript operators", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-verify-text-"))
  const pyBefore = ["def gate(n):", "    # stay and leave", "    label = \"keep and go\"", "    return n > 0 and n < 10", ""].join("\n")
  const pyAfter = ["def gate(n):", "    # stay and remain", "    label = \"keep and going\"", "    return n > 0 and n < 11", ""].join("\n")
  const tsBefore = ["export function open(n: number): boolean {", "  return n > 0 && n < 10", "}", ""].join("\n")
  const tsAfter = ["export function open(n: number): boolean {", "  return n > 0 && n < 11", "}", ""].join("\n")
  try {
    mkdirSync(path.join(dir, "src"))
    writeFileSync(path.join(dir, "src", "gate.py"), pyBefore)
    writeFileSync(path.join(dir, "src", "open.ts"), tsBefore)
    commit(dir, "init")
    writeFileSync(path.join(dir, "src", "gate.py"), pyAfter)
    writeFileSync(path.join(dir, "src", "open.ts"), tsAfter)
    commit(dir, "edit operators")
    const generated = spawnSync(tsx, ["src/cli.ts", "mutate", "generate", "--package", dir, "--src", "src", "--out", path.join(dir, "gen"), "--max-minutes", "1"], {
      cwd: root,
      encoding: "utf8",
    })
    assert.equal(generated.status, 0, generated.stderr + generated.stdout)
    const gen = JSON.parse(readFileSync(path.join(dir, "gen", "mutants", "mutants.json"), "utf8")) as {
      mutants: Array<{ id: string; file: string; line: number; op: string }>
    }
    const pyLine = (line: number) => pyAfter.split("\n")[line - 1] ?? ""
    const codeMutant = gen.mutants.find((item) => item.file === "src/gate.py" && item.op === "and-to-or" && pyLine(item.line).includes("return n > 0 and"))
    assert.ok(codeMutant, JSON.stringify(gen.mutants))
    assert.equal(gen.mutants.some((item) => item.file === "src/gate.py" && (pyLine(item.line).includes("stay and") || pyLine(item.line).includes("keep and"))), false)
    const verified = spawnSync(tsx, ["src/cli.ts", "verify-change", "--package", dir, "--out", path.join(dir, "verify"), "--base", "HEAD~1", "--commit", "HEAD", "--max-mutants", "20"], {
      cwd: root,
      encoding: "utf8",
    })
    assert.equal(verified.status, 0, verified.stderr + verified.stdout)
    const report = JSON.parse(verified.stdout) as Report
    const located = [...report.caught, ...report.missed, ...report.notRun]
    const ids = new Set(located.map((item) => item.id))
    assert.equal(ids.has(codeMutant.id), true, JSON.stringify(located))
    assert.equal(located.some((item) => item.file === "src/gate.py" && (pyLine(item.line).includes("stay and") || pyLine(item.line).includes("keep and"))), false)
    const tsMutant = located.find((item) => item.file === "src/open.ts" && (item.op === "and-to-or" || item.op === "gt-to-ge"))
    assert.ok(tsMutant, JSON.stringify(located))
    assert.equal(gen.mutants.some((item) => item.id === tsMutant.id), true)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

type Report = {
  ok: boolean
  summary: string
  affected: string[]
  ran: string[]
  rest: number
  notRun: Array<{ id: string; file: string; line: number; op: string }>
  caught: Array<{ id: string; file: string; line: number }>
  missed: Array<{ id: string; file: string; line: number }>
  goldenContract: Array<{ row: string; fields: string[] }>
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
