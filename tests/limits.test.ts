import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("verify-change scores a lib edit the tests import and ignores tests, docs, dist, and node_modules", { timeout: 90_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-lib-"))
  try {
    mkdirSync(path.join(dir, "lib"))
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "lib", "gate.ts"), "export function open(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(
      path.join(dir, "tests", "gate.test.ts"),
      [
        "import test from \"node:test\"",
        "import assert from \"node:assert/strict\"",
        "import { open } from \"../lib/gate.ts\"",
        "test(\"zero stays shut\", () => { assert.equal(open(0), false) })",
        "test(\"wide stays open\", () => { assert.equal(1, 1) })",
        "",
      ].join("\n"),
    )
    commit(dir, "init")
    writeFileSync(path.join(dir, "lib", "gate.ts"), "export function open(n: number): boolean {\n  return n > 0 || n < -1000\n}\n")
    commit(dir, "edit lib")
    const scored = verify(dir, path.join(dir, "out"))
    assert.equal(scored.ok, true, scored.summary)
    assert.deepEqual(scored.executed, ["zero stays shut"], JSON.stringify(scored.executed))
    const files = [...scored.caught, ...scored.missed, ...scored.notRun, ...scored.other]
    assert.ok(files.some((item) => item.file === "lib/gate.ts"), JSON.stringify(files))

    writeFileSync(path.join(dir, "tests", "gate.test.ts"), readFileSync(path.join(dir, "tests", "gate.test.ts"), "utf8").replace("wide stays open", "wide stays shut"))
    commit(dir, "edit test only")
    const testsOnly = verify(dir, path.join(dir, "tests-only"))
    assert.equal(testsOnly.executed.length, 0, JSON.stringify(testsOnly))
    assert.equal([...testsOnly.caught, ...testsOnly.missed, ...testsOnly.notRun].length, 0)

    mkdirSync(path.join(dir, "docs"))
    writeFileSync(path.join(dir, "docs", "note.md"), "a note\n")
    commit(dir, "docs")
    const docs = verify(dir, path.join(dir, "docs-out"))
    assert.equal([...docs.caught, ...docs.missed, ...docs.notRun].length, 0, JSON.stringify(docs))

    mkdirSync(path.join(dir, "dist"))
    writeFileSync(path.join(dir, "dist", "gate.js"), "export const built = true\n")
    commit(dir, "dist")
    const dist = verify(dir, path.join(dir, "dist-out"))
    assert.equal([...dist.caught, ...dist.missed, ...dist.notRun].length, 0, JSON.stringify(dist))

    mkdirSync(path.join(dir, "node_modules", "leftpad"), { recursive: true })
    writeFileSync(path.join(dir, "node_modules", "leftpad", "index.js"), "module.exports = function (n) { return n > 0 }\n")
    git(dir, ["add", "-f", "node_modules/leftpad/index.js"])
    git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "vendor"])
    const vendor = verify(dir, path.join(dir, "vendor-out"))
    assert.equal([...vendor.caught, ...vendor.missed, ...vendor.notRun].length, 0, JSON.stringify(vendor))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("verify-change scores a go file beside go.mod", { timeout: 120_000 }, () => {
  const go = spawnSync("go", ["version"], { encoding: "utf8" })
  assert.equal(go.status, 0, go.stderr || go.stdout)
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-go-"))
  try {
    writeFileSync(path.join(dir, "go.mod"), "module example.com/path\n\ngo 1.22\n")
    writeFileSync(path.join(dir, "path.go"), "package path\n\nfunc Open(n int) bool {\n\treturn n > 0\n}\n")
    writeFileSync(
      path.join(dir, "path_test.go"),
      "package path\n\nimport \"testing\"\n\nfunc TestOpenStaysShut(t *testing.T) {\n\tif Open(0) {\n\t\tt.Fatal(\"open\")\n\t}\n}\n",
    )
    commit(dir, "init")
    writeFileSync(path.join(dir, "path.go"), "package path\n\nfunc Open(n int) bool {\n\treturn n > 0 || n < -1000\n}\n")
    commit(dir, "edit path")
    const scored = verify(dir, path.join(dir, "out"))
    assert.equal(scored.ok, true, scored.summary)
    assert.ok(scored.executed.includes("TestOpenStaysShut"), JSON.stringify(scored.executed))
    assert.ok([...scored.caught, ...scored.missed].some((item) => item.file === "path.go"), JSON.stringify(scored))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a mocha hook selects by title and node-coverage does not name mocha tests", { timeout: 180_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-mocha-map-"))
  const open = ["export function open(n) {", "  return n > 0", "}", "export function dark(n) {", "  return n > 0", "}", ""].join("\n")
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "test"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "src", "gate.js"), open)
    writeFileSync(
      path.join(dir, "package.json"),
      `${JSON.stringify({ type: "module", scripts: { test: "mocha" }, mocha: { spec: ["test/gate.test.js"] } }, null, 2)}\n`,
    )
    writeFileSync(
      path.join(dir, "test", "gate.test.js"),
      [
        "import assert from \"node:assert/strict\"",
        "import { open } from \"../src/gate.js\"",
        "describe(\"gate\", function () {",
        "  it(\"zero stays shut\", function () {",
        "    assert.equal(open(0), false)",
        "  })",
        "  it(\"leaves dark alone\", function () {",
        "    assert.equal(1, 1)",
        "  })",
        "})",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(dir, "patches", "covered.patch"), forwardDiff("src/gate.js", open, open.replace("export function open(n) {\n  return n > 0", "export function open(n) {\n  return n >= 0")))
    writeFileSync(path.join(dir, "patches", "dark.patch"), forwardDiff("src/gate.js", open, open.replace("export function dark(n) {\n  return n > 0", "export function dark(n) {\n  return n >= 0")))
    writeFileSync(path.join(dir, ".gitignore"), "node_modules\n")
    const installed = spawnSync("npm", ["install", "--silent", "--no-fund", "--no-audit", "mocha@10"], { cwd: dir, encoding: "utf8", timeout: 120_000 })
    assert.equal(installed.status, 0, installed.stderr || installed.stdout)
    commit(dir, "init")
    const bare = spawnSync(
      process.execPath,
      [path.join(dir, "node_modules", "mocha", "bin", "mocha.js"), "--reporter", "json", "--require", path.join(root, "src", "mutate", "node-coverage.mjs"), "--grep", "zero stays shut", "test/gate.test.js"],
      { cwd: dir, encoding: "utf8", env: { ...process.env, PROBATIO_COVERAGE_MAP: path.join(dir, "bare-map.json") } },
    )
    assert.equal(bare.status, 0, bare.stderr + bare.stdout)
    const bareText = existsSync(path.join(dir, "bare-map.json")) ? readFileSync(path.join(dir, "bare-map.json"), "utf8") : ""
    const parts = path.join(dir, "bare-map.json.parts")
    const partText = existsSync(parts) ? readdirSync(parts).map((name) => readFileSync(path.join(parts, name), "utf8")).join("\n") : ""
    assert.equal(`${bareText}\n${partText}`.includes("zero stays shut"), false)
    const ran = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm", "--workers", "1"],
      { cwd: root, encoding: "utf8", timeout: 120_000 },
    )
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const covered = JSON.parse(readFileSync(path.join(dir, "out", "results", "covered.json"), "utf8")) as { outcome: string; command: string; selectedTests?: string[] }
    const dark = JSON.parse(readFileSync(path.join(dir, "out", "results", "dark.json"), "utf8")) as { outcome: string; command: string; selectedTests?: string[] }
    assert.equal(covered.outcome, "killed", JSON.stringify(covered))
    assert.ok(covered.selectedTests?.includes("gate zero stays shut"), JSON.stringify(covered.selectedTests))
    assert.match(covered.command, /zero stays shut/)
    assert.equal(covered.command.includes("leaves dark alone"), false, covered.command)
    assert.equal(covered.command.includes("node-coverage.mjs"), false, covered.command)
    assert.equal(dark.outcome, "no coverage", JSON.stringify(dark))
    assert.equal(dark.command, "")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a node file keeps the timeout its test declared", { timeout: 60_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-file-timeout-"))
  const source = "export function open(n: number): boolean {\n  return n > 0\n}\n"
  const mutant = "export function open(n: number): boolean {\n  return n < 0\n}\n"
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), source)
    writeFileSync(
      path.join(dir, "tests", "slow.test.ts"),
      [
        "import test from \"node:test\"",
        "import assert from \"node:assert/strict\"",
        "import { open } from \"../src/gate.ts\"",
        "test(\"stays open\", { timeout: 8_000 }, async () => {",
        "  await new Promise((resolve) => setTimeout(resolve, 2_500))",
        "  assert.equal(open(1), true)",
        "})",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(dir, "patches", "flip.patch"), forwardDiff("src/gate.ts", source, mutant))
    commit(dir, "init")
    const ran = spawnSync(
      tsx,
      [
        "src/cli.ts",
        "mutate",
        "run",
        "--package",
        dir,
        "--patches",
        path.join(dir, "patches"),
        "--out",
        path.join(dir, "out"),
        "--no-build",
        "--no-confirm",
        "--workers",
        "1",
        "--test-timeout-ms",
        "1000",
        "--suite-timeout-ms",
        "30000",
      ],
      { cwd: root, encoding: "utf8", timeout: 45_000 },
    )
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const report = JSON.parse(ran.stdout) as { ok: boolean; summary: string; killed: number; timeouts: number }
    assert.equal(report.ok, true, report.summary)
    assert.equal(report.summary.includes("baseline already failing"), false, report.summary)
    assert.equal(report.timeouts, 0, report.summary)
    assert.equal(report.killed, 1, report.summary)
    const result = JSON.parse(readFileSync(path.join(dir, "out", "results", "flip.json"), "utf8")) as { outcome: string; killedBy?: string[] }
    assert.equal(result.outcome, "killed", JSON.stringify(result))
    assert.ok(result.killedBy?.some((name) => name.includes("stays open")), JSON.stringify(result))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("covered mutants of one file share a process until one crashes", { timeout: 120_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-batch-"))
  const source = [
    "export function live(n: number): boolean {",
    "  return n > 0",
    "}",
    "export function crash(flag: boolean): number {",
    "  if (flag === false) process.exit(7)",
    "  return 1",
    "}",
    "export function after(n: number): boolean {",
    "  return n > 0",
    "}",
    "",
  ].join("\n")
  const live = source.replace("export function live(n: number): boolean {\n  return n > 0", "export function live(n: number): boolean {\n  return n >= 0")
  const crash = source.replace("flag === false", "flag !== false")
  const after = source.replace("export function after(n: number): boolean {\n  return n > 0", "export function after(n: number): boolean {\n  return n >= 0")
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), source)
    writeFileSync(
      path.join(dir, "tests", "gate.test.ts"),
      [
        "import test from \"node:test\"",
        "import assert from \"node:assert/strict\"",
        "import { live, crash, after } from \"../src/gate.ts\"",
        "test(\"live stays shut\", () => { assert.equal(live(0), false) })",
        "test(\"crash stays up\", () => { assert.equal(crash(true), 1) })",
        "test(\"after stays shut\", () => { assert.equal(after(0), false) })",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(dir, "patches", "a-live.patch"), forwardDiff("src/gate.ts", source, live))
    writeFileSync(path.join(dir, "patches", "b-crash.patch"), forwardDiff("src/gate.ts", source, crash))
    writeFileSync(path.join(dir, "patches", "c-after.patch"), forwardDiff("src/gate.ts", source, after))
    commit(dir, "init")
    const ran = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm"],
      { cwd: root, encoding: "utf8", timeout: 90_000 },
    )
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const read = (id: string) => JSON.parse(readFileSync(path.join(dir, "out", "results", `${id}.json`), "utf8")) as { outcome: string; error?: string; selectedTests?: string[]; killedBy?: string[] }
    const a = read("a-live")
    const b = read("b-crash")
    const c = read("c-after")
    assert.equal(a.outcome, "killed", JSON.stringify(a))
    assert.ok(a.killedBy?.some((name) => name.includes("live stays shut")), JSON.stringify(a))
    assert.equal(b.outcome, "error", JSON.stringify(b))
    assert.equal(b.error, "suite process crashed")
    assert.equal(c.outcome, "killed", JSON.stringify(c))
    assert.equal(c.error, undefined)
    assert.ok(c.killedBy?.some((name) => name.includes("after stays shut")), JSON.stringify(c))
    assert.equal(c.killedBy?.some((name) => name.includes("live stays shut")), false)
    const log = readFileSync(path.join(dir, "out", "batch-processes.jsonl"), "utf8").trim().split("\n").map((line) => JSON.parse(line) as { id: string; pid: number })
    const pid = (id: string) => log.find((row) => row.id === id)?.pid
    assert.ok(pid("a-live") && pid("a-live") === pid("b-crash"), JSON.stringify(log))
    assert.notEqual(pid("c-after"), pid("a-live"), JSON.stringify(log))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

type Located = { id: string; file: string; line: number; outcome?: string }

type VerifyReport = {
  ok: boolean
  summary: string
  executed: string[]
  caught: Located[]
  missed: Located[]
  other: Located[]
  notRun: Located[]
}

function verify(dir: string, out: string): VerifyReport {
  const result = spawnSync(
    tsx,
    ["src/cli.ts", "verify-change", "--package", dir, "--out", out, "--base", "HEAD~1", "--commit", "HEAD", "--max-mutants", "4"],
    { cwd: root, encoding: "utf8", timeout: 90_000 },
  )
  assert.equal(result.status, 0, result.stderr + result.stdout)
  return JSON.parse(result.stdout) as VerifyReport
}

function commit(repo: string, message: string): void {
  git(repo, ["init", "-q"])
  git(repo, ["add", "."])
  git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", message])
}

function git(repo: string, args: string[]): void {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0 && !(args[0] === "init" && result.status !== 0)) throw new Error(result.stderr || result.stdout)
}
