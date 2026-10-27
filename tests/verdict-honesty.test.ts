import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { isBuildName, killLabel } from "../src/mutate/kill-label.ts"
import { findInSource } from "../src/mutate/find.ts"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")
const BUILD_NAMES = ["build", "javac", "compiler", "cobc", "nasm", "clang", "cargo", "tsc", "dotnet", "swiftc"]

test("a test titled with a compiler token stays a kill", () => {
  for (const name of BUILD_NAMES) {
    assert.equal(isBuildName(name), true, name)
    assert.equal(killLabel([name])?.cause, "build", name)
    const titled = `tests/gate.test.ts::${name}`
    assert.equal(isBuildName(titled), false, titled)
    const label = killLabel([titled])
    assert.equal(label?.cause, "test", titled)
    assert.match(label?.next ?? "", new RegExp(name))
  }
  assert.equal(isBuildName("src/build.js"), false)
  assert.equal(isBuildName("Gate#build"), false)
  assert.equal(killLabel(["compiler"])?.cause, "build")
  assert.equal(killLabel(["tests/gate.test.ts::build", "compiler"])?.cause, "test")
})

test("java and c# text blocks are not mutated and a js interpolation is", () => {
  const java = [
    "class Gate {",
    "  String label() {",
    "    return \"\"\"",
    "      ready && n > 0",
    "      \"\"\";",
    "  }",
    "  boolean open(int n) {",
    "    return n > 0 && n < 10;",
    "  }",
    "}",
    "",
  ].join("\n")
  const javaOps = findInSource("src/Gate.java", java)
  assert.equal(javaOps.points.some((point) => point.line === 4), false, javaOps.points.map((point) => `${point.line}:${point.op}`).join(" "))
  assert.ok(javaOps.points.some((point) => point.line === 8 && point.op === "and-to-or"))
  assert.ok(javaOps.points.some((point) => point.line === 8 && point.op === "gt-to-ge"))
  const csharp = [
    "class Gate {",
    "  string Label() {",
    "    return $\"\"\"",
    "      ready && n > 0",
    "      \"\"\";",
    "  }",
    "  bool Open(int n) {",
    "    return n > 0 && n < 10;",
    "  }",
    "}",
    "",
  ].join("\n")
  const csOps = findInSource("src/Gate.cs", csharp)
  assert.equal(csOps.points.some((point) => point.line === 4), false, csOps.points.map((point) => `${point.line}:${point.op}`).join(" "))
  assert.ok(csOps.points.some((point) => point.line === 8 && point.op === "gt-to-ge"))
  const interpolated = "export function greet(n, ready) {\n  return `hello ${n > 0 && ready}`\n}\n"
  const jsOps = findInSource("src/greet.js", interpolated).points.map((point) => point.op).sort()
  assert.deepEqual(jsOps, ["and-to-or", "gt-to-ge"])
  const nested = "export function greet(n, ready) {\n  return `a ${\`b ${n > 0 && ready}\`}`\n}\n"
  const nestedOps = findInSource("src/greet.js", nested).points.map((point) => point.op).sort()
  assert.deepEqual(nestedOps, ["and-to-or", "gt-to-ge"])
  const commented = "export function greet(n) {\n  return `a ${n > 0 /* && ready */}`\n}\n"
  const commentedOps = findInSource("src/greet.js", commented).points.map((point) => point.op)
  assert.deepEqual(commentedOps, ["gt-to-ge"])
  const quoted = "export function greet(name) {\n  return `hello ${name === \"n > 0 && ready\"}`\n}\n"
  const quotedOps = findInSource("src/greet.js", quoted).points.map((point) => point.op)
  assert.deepEqual(quotedOps, ["eq-to-neq"])
  const ordinary = "export function open(n, ready) {\n  const label = \"n > 0 && ready\"\n  return n > 0 && ready\n}\n"
  const ordinaryOps = findInSource("src/open.js", ordinary).points.map((point) => `${point.op}:${point.line}`)
  assert.deepEqual(ordinaryOps.sort(), ["and-to-or:3", "gt-to-ge:3"])
  const adjacent = "export function open(n, ready) {\n  const weird = \"\"\"hello\"\n  return n > 0 && ready\n}\n"
  const adjacentOps = findInSource("src/open.js", adjacent).points.map((point) => point.op).sort()
  assert.deepEqual(adjacentOps, ["and-to-or", "gt-to-ge"])
})

test("an uncovered line stays no coverage when a test name is forced", { timeout: 120_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-uncovered-"))
  const source = "export function gate(n) {\n  if (n > 0) {\n    return n > 1 && n < 10\n  }\n  return false\n}\n"
  const mutated = source.replace("return n > 1 && n < 10", "return n >= 1 && n < 10")
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
      'import { gate } from "../src/gate.js"',
      'test("stays shut", () => {',
      "  assert.equal(gate(0), false)",
      "  assert.equal(gate(-1), false)",
      "})",
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-inner.patch"), forwardDiff("src/gate.js", source, mutated))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = run(dir, path.join(dir, "out"), ["--only-test", "stays shut", "--no-confirm", "--no-build"])
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; survived: number; noCoverage: number }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.survived, 0, body.summary)
    assert.equal(body.noCoverage, 1, body.summary)
    const saved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-inner.json"), "utf8")) as { outcome: string; next: string }
    assert.equal(saved.outcome, "no coverage", JSON.stringify(saved))
    assert.match(saved.next, /No coverage/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a same-file failure the line map did not name is a kill", { timeout: 120_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-same-file-"))
  const source = "export function gate(n) {\n  return n > 0 && n < 10\n}\n"
  const mutated = "export function gate(n) {\n  return n > 0 || n < 10\n}\n"
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
      'import { gate } from "../src/gate.js"',
      'test("selected", () => { assert.equal(gate(1), true) })',
      'test("other", () => { assert.equal(gate(0), false) })',
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-or.patch"), forwardDiff("src/gate.js", source, mutated))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  const out = path.join(dir, "out")
  mkdirSync(out)
  writeFileSync(
    path.join(out, "coverage-map.json"),
    `${JSON.stringify({ files: { "src/gate.js": { "1": ["selected"], "2": ["selected"], "3": ["selected"] } } }, null, 2)}\n`,
  )
  try {
    const result = run(dir, out, ["--no-confirm", "--no-build", "--workers", "1"])
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; survived: number }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.survived, 0, body.summary)
    assert.equal(body.killed, 1, body.summary)
    const saved = JSON.parse(readFileSync(path.join(out, "results", "m-or.json"), "utf8")) as { outcome: string; killedBy: string[] }
    assert.equal(saved.outcome, "killed", JSON.stringify(saved))
    assert.ok(saved.killedBy.some((name) => name.includes("other")), JSON.stringify(saved.killedBy))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a test titled build is a kill on the real run", { timeout: 120_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-titled-build-"))
  const source = "export function open(n) {\n  return n > 0\n}\n"
  const mutated = "export function open(n) {\n  return n >= 0\n}\n"
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
      'test("build", () => { assert.equal(open(0), false) })',
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.js", source, mutated))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = run(dir, path.join(dir, "out"), ["--no-confirm", "--no-build"])
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; unviable: number }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.unviable, 0, body.summary)
    const saved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-ge.json"), "utf8")) as {
      outcome: string
      cause: string
      killedBy: string[]
    }
    assert.equal(saved.outcome, "killed", JSON.stringify(saved))
    assert.equal(saved.cause, "test")
    assert.ok(saved.killedBy.some((name) => name.endsWith("::build")), JSON.stringify(saved.killedBy))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a pytest syntax or import failure is unviable and a call failure stays a kill", { timeout: 120_000 }, (t) => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-pytest-load-"))
  const env = pytestEnv(dir)
  if (!env) {
    rmSync(dir, { recursive: true, force: true })
    t.skip("missing tool: pytest")
    return
  }
  const source = "def gate(n):\n    return n > 0\n"
  const syntax = "def gate(n):\n    return n >\n"
  const imported = "import not_a_real_mod_probatio\ndef gate(n):\n    return n > 0 and True\n"
  const flipped = "def gate(n):\n    return n >= 0\n"
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "pytest.ini"), "[pytest]\n")
  writeFileSync(path.join(dir, "src", "gate.py"), source)
  writeFileSync(
    path.join(dir, "tests", "test_gate.py"),
    [
      "import pytest",
      "from gate import gate",
      "",
      "def test_low():",
      "    assert gate(0) is False, \"SyntaxError ImportError ModuleNotFoundError IndentationError\"",
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-syntax.patch"), forwardDiff("src/gate.py", source, syntax))
  writeFileSync(path.join(dir, "patches", "m-import.patch"), forwardDiff("src/gate.py", source, imported))
  writeFileSync(path.join(dir, "patches", "m-kill.patch"), forwardDiff("src/gate.py", source, flipped))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = run(dir, path.join(dir, "out"), ["--no-confirm", "--no-build", "--suite-timeout-ms", "60000"], env)
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; unviable: number; unviableIds?: string[] }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.unviable, 2, body.summary)
    const syntaxSaved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-syntax.json"), "utf8")) as { outcome: string }
    const importSaved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-import.json"), "utf8")) as { outcome: string }
    const killSaved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-kill.json"), "utf8")) as { outcome: string; killedBy: string[] }
    assert.equal(syntaxSaved.outcome, "unviable", JSON.stringify(syntaxSaved))
    assert.equal(importSaved.outcome, "unviable", JSON.stringify(importSaved))
    assert.equal(killSaved.outcome, "killed", JSON.stringify(killSaved))
    assert.ok(killSaved.killedBy.some((name) => name.includes("test_low")), JSON.stringify(killSaved.killedBy))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function run(dir: string, out: string, extra: string[], env: NodeJS.ProcessEnv = process.env) {
  return spawnSync(
    tsx,
    ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", out, "--workers", "1", ...extra],
    { cwd: root, encoding: "utf8", timeout: 300_000, env },
  )
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}

function pytestEnv(dir: string): NodeJS.ProcessEnv | null {
  if (spawnSync("python3", ["-c", "import pytest"], { encoding: "utf8" }).status === 0) return process.env
  const which = spawnSync("which", ["pytest"], { encoding: "utf8" })
  const bin = which.stdout.trim()
  if (which.status !== 0 || !bin) return null
  const shebang = readFileSync(bin, "utf8").split("\n")[0] ?? ""
  if (!shebang.startsWith("#!")) return null
  const python = shebang.slice(2).trim()
  const shim = path.join(dir, "bin")
  mkdirSync(shim)
  writeFileSync(path.join(shim, "python3"), `#!/bin/sh\nexec ${JSON.stringify(python)} "$@"\n`)
  chmodSync(path.join(shim, "python3"), 0o755)
  return { ...process.env, PATH: `${shim}${path.delimiter}${process.env.PATH ?? ""}` }
}
