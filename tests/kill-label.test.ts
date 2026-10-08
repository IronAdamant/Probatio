import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

const COMPILER = "javac|compiler|build|cobc|nasm|clang|cargo|tsc|dotnet|swiftc"

test("a mutant that does not build is unviable, not a kill", { timeout: 60_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-label-"))
  const source = "export function open(n) { return n > 0 }\n"
  const flipped = "export function open(n) { return n >= 0 }\n"
  const broken = "export function open(n) { return n > ; }\n"
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "src", "gate.js"), source)
  writeFileSync(
    path.join(dir, "tests", "gate.test.js"),
    [
      'import test from "node:test"',
      'import assert from "node:assert/strict"',
      'import { open } from "../src/gate.js"',
      'test("closed at zero", () => { assert.equal(open(0), false) })',
      'test("open above zero", () => { assert.equal(open(1), true) })',
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-test.patch"), forwardDiff("src/gate.js", source, flipped))
  writeFileSync(path.join(dir, "patches", "m-build.patch"), forwardDiff("src/gate.js", source, broken))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = launch(dir, path.join(dir, "out"))
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as {
      ok: boolean
      summary: string
      killed: number
      unviable: number
      unviableIds: string[]
      survived: number
      kills?: Array<{ id: string; cause: string; killedBy: string[]; next: string }>
    }
    assert.equal(body.ok, true, body.summary)
    // A compiler rejecting the mutant is not a test catching it. The count says so.
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.unviable, 1, body.summary)
    assert.deepEqual(body.unviableIds, ["m-build"])
    assert.match(body.summary, /1 killed, 1 did not build,/)
    assert.equal(body.survived, 0, body.summary)
    const kills = body.kills ?? []
    const byTest = kills.find((item) => item.id === "m-test")
    assert.ok(byTest, JSON.stringify(kills))
    assert.equal(kills.some((item) => item.id === "m-build"), false, JSON.stringify(kills))
    assert.equal(byTest.cause, "test")
    assert.match(byTest.next, /closed at zero/)
    const testSaved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-test.json"), "utf8")) as { outcome: string; cause: string }
    assert.equal(testSaved.outcome, "killed")
    const buildSaved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-build.json"), "utf8")) as { outcome: string; cause: string; next: string }
    assert.equal(buildSaved.outcome, "unviable")
    assert.equal(buildSaved.cause, "build")
    assert.match(buildSaved.next, /did not build/)
    assert.doesNotMatch(buildSaved.next, new RegExp(`add a test named (${COMPILER})`, "i"))
    const tally = spawnSync(tsx, ["src/cli.ts", "mutate", "tally", "--out", path.join(dir, "out")], { cwd: root, encoding: "utf8" })
    const tallied = JSON.parse(tally.stdout) as { keep: string[]; gaps: unknown[] }
    assert.equal(tallied.gaps.length, 0, "an unviable mutant is not a gap")
    assert.equal(tallied.keep.some((name) => new RegExp(`^(${COMPILER})$`).test(name)), false, JSON.stringify(tallied.keep))
    const testKill = check(dir, "m-test")
    const buildKill = check(dir, "m-build")
    assert.equal(testKill.status, 0, testKill.stderr + testKill.stdout)
    assert.equal(buildKill.status, 0, buildKill.stderr + buildKill.stdout)
    const testBody = JSON.parse(testKill.stdout) as { ok: boolean; cause: string; next: string }
    const buildBody = JSON.parse(buildKill.stdout) as { ok: boolean; cause: string; next: string }
    assert.equal(testBody.ok, true)
    assert.equal(buildBody.ok, true)
    assert.equal(testBody.cause, "test")
    assert.equal(buildBody.cause, "build")
    assert.match(testBody.next, /closed at zero/)
    assert.match(buildBody.next, /did not build/)
    assert.equal(testBody.next === buildBody.next, false)
    assert.doesNotMatch(buildBody.next, new RegExp(`add a test named (${COMPILER})`, "i"))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function launch(dir: string, out: string) {
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
      out,
      "--build",
      "node --check src/gate.js",
      "--no-confirm",
      "--workers",
      "1",
      "--suite-timeout-ms",
      "30000",
    ],
    { cwd: root, encoding: "utf8" },
  )
}

function check(dir: string, id: string) {
  return spawnSync(
    tsx,
    [
      "src/cli.ts",
      "check-kill",
      id,
      "--package",
      dir,
      "--repo",
      dir,
      "--patches",
      path.join(dir, "patches"),
      "--out",
      path.join(dir, `check-${id}`),
      "--build",
      "node --check src/gate.js",
      "--no-confirm",
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
