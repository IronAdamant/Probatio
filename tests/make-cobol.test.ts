import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { firstSkip, missingTool } from "./require-tool.ts"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

const source = [
  ">>SOURCE FORMAT FREE",
  "IDENTIFICATION DIVISION.",
  "PROGRAM-ID. GATE.",
  "DATA DIVISION.",
  "WORKING-STORAGE SECTION.",
  "01 N PIC 9 VALUE 0.",
  "01 FAILED PIC 9 VALUE 0.",
  "PROCEDURE DIVISION.",
  "MAIN.",
  "    IF N > 0",
  "        MOVE 1 TO FAILED",
  "    END-IF",
  "    DISPLAY \"    Case: stays-closed - OK\"",
  "    DISPLAY \"Tests run: 1\"",
  "    DISPLAY \"Tests failed: \" FAILED",
  "    DISPLAY \"Tests skipped: 0\"",
  "    IF FAILED > 0",
  "        DISPLAY \"Failures:\"",
  "        DISPLAY \"Suite: gate\"",
  "        DISPLAY \"  Test: main\"",
  "        DISPLAY \"    Case: stays-closed\"",
  "        STOP RUN RETURNING 1",
  "    END-IF",
  "    STOP RUN RETURNING 0.",
  "",
].join("\n")

test("make test names a COBOL failure and a cobc failure differently", { timeout: 60_000, skip: firstSkip(missingTool("make"), missingTool("cobc")) }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-make-cob-"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "tests", "main.cob"), source)
  writeFileSync(
    path.join(dir, "Makefile"),
    ["test:", "\tcobc -x -free -o suite tests/main.cob", "\t./suite", ""].join("\n"),
  )
  const flipped = source.replace("IF N > 0", "IF N >= 0")
  const broken = source.replace("IF N > 0", "IF N > >")
  writeFileSync(path.join(dir, "patches", "m-test.patch"), forwardDiff("tests/main.cob", source, flipped))
  writeFileSync(path.join(dir, "patches", "m-build.patch"), forwardDiff("tests/main.cob", source, broken))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = launch(dir)
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as {
      ok: boolean
      summary: string
      killed: number
      survived: number
      kills?: Array<{ id: string; cause: string; next: string }>
    }
    assert.equal(body.ok, true, body.summary)
    const baseline = JSON.parse(readFileSync(path.join(dir, "out", "baseline.json"), "utf8")) as {
      ok: boolean
      failed: string[]
    }
    assert.equal(baseline.ok, true, JSON.stringify(baseline))
    assert.deepEqual(baseline.failed, [])
    assert.equal(body.killed, 2, body.summary)
    assert.equal(body.survived, 0, body.summary)
    const byTest = (body.kills ?? []).find((item) => item.id === "m-test")
    const byBuild = (body.kills ?? []).find((item) => item.id === "m-build")
    assert.ok(byTest, JSON.stringify(body.kills))
    assert.ok(byBuild, JSON.stringify(body.kills))
    assert.equal(byTest.cause, "test")
    assert.equal(byBuild.cause, "build")
    assert.match(byTest.next, /gate::main::stays-closed/)
    assert.doesNotMatch(byTest.next, /stays-closed - OK/)
    assert.match(byBuild.next, /build/i)
    assert.doesNotMatch(byBuild.next, /add a test named (javac|compiler|build|cobc|nasm|clang|cargo|tsc|dotnet|swiftc)/i)
    const saved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-build.json"), "utf8")) as { killedBy: string[] }
    assert.deepEqual(saved.killedBy, ["cobc"])
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function launch(dir: string) {
  return spawnSync(
    tsx,
    [
      "src/cli.ts",
      "mutate",
      "run",
      "--package",
      dir,
      "--repo",
      dir,
      "--patches",
      path.join(dir, "patches"),
      "--out",
      path.join(dir, "out"),
      "--no-build",
      "--no-confirm",
      "--workers",
      "1",
      "--suite-timeout-ms",
      "30000",
    ],
    { cwd: root, encoding: "utf8" },
  )
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
