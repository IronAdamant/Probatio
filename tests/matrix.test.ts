import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("matrix report reads the score files, headlines the gap, and does not delete a test", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-matrix-"))
  const scores = path.join(dir, "scores.tsv")
  const direct = path.join(dir, "direct.txt")
  const patches = path.join(dir, "patches")
  const out = path.join(dir, "out")
  const sleepy = path.join(dir, "tests", "sleepy.test.ts")
  const kills = path.join(dir, "kills.json")
  const sleepyBody = "export const sleepy = true\n"
  try {
    mkdirSync(patches, { recursive: true })
    mkdirSync(path.dirname(sleepy), { recursive: true })
    writeFileSync(sleepy, sleepyBody)
    writeFileSync(
      scores,
      [
        "set\tlines\tholdout (4)\treal (4)\tsynthetic-1 (4)\tsynthetic-2 (4)",
        "full\t10\t4 (100.0%)\t4 (100.0%)\t3 (75.0%)\t4 (100.0%)",
        "cover\t3\t1 (25.0%)\t4 (100.0%)\t4 (100.0%)\t1 (25.0%)",
        "",
      ].join("\n"),
    )
    writeFileSync(
      direct,
      [
        "thin (run directly)",
        "holdout: 1/4 (25.0%)  missed ho-abc",
        "real (train): 4/4 (100.0%)",
        "synthetic-2: 1/4 (25.0%)",
        "",
      ].join("\n"),
    )
    writeFileSync(
      path.join(patches, "ho-abc.patch"),
      [
        "diff --git a/src/gate.ts b/src/gate.ts",
        "--- a/src/gate.ts",
        "+++ b/src/gate.ts",
        "@@ -4,1 +4,1 @@",
        "-return true",
        "+return false",
        "",
      ].join("\n"),
    )
    writeFileSync(kills, JSON.stringify({ "tests/sleepy.test.ts": 0, "tests/awake.test.ts": 2 }))

    const first = run(dir, scores, direct, patches, kills, out)
    const second = run(dir, scores, direct, patches, kills, out)
    assert.equal(first.status, 0, first.stderr)
    assert.equal(second.status, 0, second.stderr)
    assert.equal(first.stdout, second.stdout)
    const report = JSON.parse(first.stdout) as MatrixReport
    assert.equal(report.schemaVersion, 1)
    assert.equal(report.ok, true)
    assert.equal(report.command, "matrix.report")
    const thin = report.rows.find((row) => row.set === "thin")
    const cover = report.rows.find((row) => row.set === "cover")
    assert.ok(thin?.holdout)
    assert.equal(thin.holdout.caught, 1)
    assert.equal(thin.holdout.total, 4)
    assert.equal(thin.real?.caught, 4)
    assert.equal(thin.real?.total, 4)
    assert.ok(report.collapse.batch2.some((item) => item.set === "cover"))
    assert.equal(report.collapse.batch2.some((item) => item.set === "full"), false)
    assert.equal(cover?.synthetic1?.caught, 4)
    assert.equal(cover?.synthetic2?.caught, 1)
    assert.equal(report.gaps.length, 1)
    assert.equal(report.rest, 0)
    assert.equal(report.gaps[0].id, "ho-abc")
    assert.equal(report.gaps[0].file, "src/gate.ts")
    assert.equal(report.gaps[0].line, 4)
    assert.match(report.next, /ho-abc at src\/gate\.ts:4/)
    assert.equal(report.pruning.mode, "advisory")
    assert.equal(report.pruning.deletedTests, 0)
    assert.deepEqual(report.pruning.advice, ["tests/sleepy.test.ts"])
    assert.equal(readFileSync(sleepy, "utf8"), sleepyBody)
    assert.equal(existsSync(sleepy), true)
    const full = JSON.parse(readFileSync(report.full, "utf8")) as MatrixReport
    assert.equal(full.gaps[0].id, "ho-abc")
    assert.equal(full.pruning.deletedTests, 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

type Count = { caught: number; total: number }
type MatrixReport = {
  schemaVersion: number
  ok: boolean
  command: string
  next: string
  rest: number
  full: string
  rows: Array<{ set: string; holdout: Count | null; real: Count | null; synthetic1: Count | null; synthetic2: Count | null }>
  collapse: { batch2: Array<{ set: string }> }
  gaps: Array<{ id: string; file: string | null; line: number | null }>
  pruning: { mode: string; deletedTests: number; advice: string[] }
}

function run(cwd: string, scores: string, direct: string, patches: string, kills: string, out: string) {
  return spawnSync(
    tsx,
    [
      "src/cli.ts",
      "matrix",
      "report",
      "--scores",
      scores,
      "--direct",
      direct,
      "--patches",
      patches,
      "--kills",
      kills,
      "--out",
      out,
    ],
    { cwd: root, encoding: "utf8" },
  )
}
