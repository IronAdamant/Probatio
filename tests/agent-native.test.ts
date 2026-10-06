import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { firstSkip, missingPytest, missingTool } from "./require-tool.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")
const pass = process.env.PROBATIO_PASS ?? "1"
const scratch = process.env.PROBATIO_SCRATCH ?? ""
const compilers = ["nasm", "clang", "cobc", "clang++", "javac", "swiftc"]

type Body = {
  schemaVersion: number
  ok: boolean
  command: string
  summary: string
  next: string
  nextCall: unknown
  suiteCommand?: string
  killed?: number
  survived?: number
  noCoverage?: number
  timeouts?: number
  notStarted?: string[]
  gaps?: Array<{ id: string }>
  kills?: Array<{ id: string }>
  baselineMs?: number | null
  commands?: Array<{ id: string; outcome: string; command: string; cause: string | null; next: string; reused?: boolean; wholeSuite?: boolean }>
}

test("unknown layout asks for a suite command and an explicit command runs", { timeout: 60_000, skip: missingTool("clang") }, () => {
  const dir = fixture()
  writeFileSync(path.join(dir, "tests", "main.c"), "int main(void) { int n = 0; if (n > 0) return 1; return 0; }\n")
  const original = readFileSync(path.join(dir, "tests", "main.c"), "utf8")
  writeFileSync(path.join(dir, "patches", "m-one.patch"), "not a diff\n")
  commit(dir)
  const unknown = run(dir, [])
  const unknownBody = json(unknown)
  assert.equal(unknownBody.schemaVersion, 1)
  assert.equal(unknownBody.ok, false)
  assert.equal(unknownBody.command, "mutate.run")
  assert.match(unknownBody.next, /suite command is unknown/i)
  assert.match(unknownBody.next, /--suite-command/)
  assert.equal(unknownBody.suiteCommand ?? "", "")
  for (const tool of compilers) assert.notEqual(unknownBody.suiteCommand, tool)
  assert.equal(unknown.stderr.includes("suite clang"), false, unknown.stderr)
  assert.equal(existsSync(path.join(dir, "tests", "gate")), false)
  save("unknown-suite", unknown.stdout)
  save("step-unknown", `${unknown.stdout}\nnext=${unknownBody.next}\n`)

  rmSync(path.join(dir, "patches", "m-one.patch"))
  const flipped = original.replace("n > 0", "n >= 0")
  writeFileSync(path.join(dir, "patches", "m-one.patch"), forwardDiff("tests/main.c", original, flipped))
  const command = "clang -std=c11 -o tests/gate tests/main.c && tests/gate"
  const explicit = run(dir, ["--suite-command", command, "--no-confirm", "--suite-timeout-ms", "30000"])
  const explicitBody = json(explicit)
  assert.equal(explicitBody.ok, true, explicit.stdout)
  assert.equal(explicitBody.suiteCommand, command)
  assert.equal(/suite command is unknown/i.test(explicitBody.next), false, explicitBody.next)
  assert.match(explicit.stderr, /suite clang -std=c11/)
  save("step-explicit", `${explicit.stdout}\nnext=${explicitBody.next}\n`)
})

test("make test that would download a missing jar does not fetch", { timeout: 30_000 }, () => {
  const dir = fixture()
  const bin = path.join(dir, "bin")
  mkdirSync(bin)
  const marker = path.join(dir, "fetched")
  writeFileSync(path.join(bin, "curl"), `#!/bin/sh\necho curl >> "${marker}"\nexit 0\n`)
  writeFileSync(path.join(bin, "wget"), `#!/bin/sh\necho wget >> "${marker}"\nexit 0\n`)
  chmodSync(path.join(bin, "curl"), 0o755)
  chmodSync(path.join(bin, "wget"), 0o755)
  writeFileSync(path.join(dir, "tests", "main.cob"), ">>SOURCE FORMAT FREE\nIDENTIFICATION DIVISION.\nPROGRAM-ID. GATE.\nPROCEDURE DIVISION.\nSTOP RUN.\n")
  writeFileSync(
    path.join(dir, "Makefile"),
    ["test: server.jar", "\tcobc -x -free -o suite tests/main.cob", "\t./suite", "", "server.jar:", "\tcurl -o server.jar https://example.invalid/server.jar", ""].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-one.patch"), "not a diff\n")
  commit(dir)
  const result = run(dir, [], { PATH: `${bin}${path.delimiter}${process.env.PATH ?? ""}` })
  const body = json(result)
  assert.equal(body.ok, false, result.stdout)
  assert.match(body.next, /suite cannot run/i)
  assert.match(body.next, /server\.jar/)
  assert.match(body.next, /Nothing was fetched/)
  assert.equal(body.suiteCommand, "make test")
  assert.equal(body.suiteCommand.includes("cobc"), false)
  assert.equal(existsSync(marker), false, "a download tool ran")
  assert.equal(existsSync(path.join(dir, "server.jar")), false)
  save("cannot-run", result.stdout)
  save("step-cannot-run", `${result.stdout}\nnext=${body.next}\n`)
})

test("a mutant past the baseline multiple times out and another mutant finishes", { timeout: 180_000 }, () => {
  const dir = nodePackage(pauseSource())
  const generated = generate(dir)
  assert.equal(generated.ok, true, generated.stdout)
  const result = run(dir, [
    "--patches",
    path.join(dir, "generated", "mutants"),
    "--no-confirm",
    "--timeout-floor-ms",
    "4000",
    "--timeout-multiple",
    "5",
    "--suite-timeout-ms",
    "60000",
  ])
  const body = json(result)
  assert.equal(body.ok, true, result.stdout)
  const baseline = JSON.parse(readFileSync(path.join(dir, "out", "baseline.json"), "utf8")) as { durationMs?: number }
  assert.equal(typeof baseline.durationMs, "number")
  assert.ok((baseline.durationMs ?? 0) > 0)
  const commands = body.commands ?? []
  const slow = commands.find((item) => item.outcome === "timeout")
  const fast = commands.find((item) => item.outcome === "killed" || item.outcome === "survived")
  assert.ok(slow, result.stdout)
  assert.equal(slow?.cause ?? null, null)
  assert.equal(slow?.outcome, "timeout")
  assert.match(body.summary, /^\d+ no coverage, \d+ survived/)
  assert.match(body.next, /Timed out/)
  assert.match(body.next, /baseline/)
  assert.equal((body.kills ?? []).some((item) => item.id === slow?.id), false)
  assert.equal((body.gaps ?? []).some((item) => item.id === slow?.id), false)
  assert.ok((body.timeouts ?? 0) >= 1)
  for (const token of ["clang", "cobc", "javac", "nasm", "swiftc", "cargo", "tsc", "dotnet"]) {
    assert.notEqual(slow?.cause, token)
  }
  assert.ok(fast, result.stdout)
  assert.notEqual(fast?.outcome, "timeout")
  save("timeout", result.stdout)
  save("step-timeout", `${result.stdout}\nnext=${body.next}\nkilled=${body.killed} survived=${body.survived} timeouts=${body.timeouts}\n`)
})

test("a stopped campaign keeps finished mutants and lists the ones not started", { timeout: 60_000 }, () => {
  const dir = nodePackage(fastSource())
  const generated = generate(dir)
  assert.equal(generated.ok, true, generated.stdout)
  const result = run(dir, ["--patches", path.join(dir, "generated", "mutants"), "--no-confirm", "--budget-ms", "1", "--suite-timeout-ms", "30000"])
  const body = json(result)
  assert.equal(body.ok, true, result.stdout)
  const notStarted = body.notStarted ?? []
  assert.ok(notStarted.length > 0, result.stdout)
  assert.match(body.next, /Not started:/)
  assert.match(body.next, /resumes/)
  for (const id of notStarted) assert.match(body.next, new RegExp(id))
  const resultsDir = path.join(dir, "out", "results")
  const finished = readdirSync(resultsDir).filter((name) => name.endsWith(".json"))
  assert.ok(finished.length > 0, "no finished mutant on disk")
  for (const id of notStarted) assert.equal(existsSync(path.join(resultsDir, `${id}.json`)), false)
  save("partial", result.stdout)
  save("step-partial", `${result.stdout}\nnext=${body.next}\nnotStarted=${notStarted.join(",")}\nfinished=${finished.length}\n`)
})

test("python coverage marks an uncovered line and runs only the tests that hit a covered line", { timeout: 120_000, skip: missingPytest() }, () => {
  const dir = fixture()
  mkdirSync(path.join(dir, "src"))
  writeFileSync(
    path.join(dir, "src", "gate.py"),
    ["def gate(n):", "    return n > 0 and n < 10", "", "def wide(n):", "    return n > 0 or n < 100", "", "def unused(n):", "    return n > 2 and n < 3", ""].join("\n"),
  )
  writeFileSync(
    path.join(dir, "tests", "test_gate.py"),
    ["import pytest", "from gate import gate, wide", "", "def test_low():", "    assert gate(0) is False", "", "def test_wide():", "    assert wide(50) is True", ""].join("\n"),
  )
  commit(dir)
  const generated = generate(dir, ["--src", "src"])
  assert.equal(generated.ok, true, generated.stdout)
  const result = run(dir, ["--patches", path.join(dir, "generated", "mutants"), "--no-confirm", "--suite-timeout-ms", "60000"])
  const body = json(result)
  assert.equal(body.ok, true, result.stdout)
  const uncovered = (body.commands ?? []).filter((item) => item.outcome === "no coverage")
  assert.ok(uncovered.length > 0, result.stdout)
  for (const item of uncovered) {
    assert.notEqual(item.outcome, "survived")
    assert.match(item.next, /No coverage/)
    assert.match(item.next, /No test executed/)
  }
  const map = JSON.parse(readFileSync(path.join(dir, "out", "coverage-map.json"), "utf8")) as { files: Record<string, Record<string, string[]>> }
  const covered = (body.commands ?? []).find((item) => item.outcome === "killed" || item.outcome === "survived")
  assert.ok(covered, result.stdout)
  assert.match(covered.command, /-k /)
  const pattern = covered.command.split("-k ")[1] ?? ""
  assert.equal(pattern.includes("test_low") && pattern.includes("test_wide"), false, covered.command)
  const names = new Set(Object.values(map.files).flatMap((lines) => Object.values(lines).flat()))
  assert.ok(names.size > 0, "coverage map was empty")
  save("step-py-fixture", `${result.stdout}\nnext=${body.next}\nnoCoverage=${body.noCoverage} survived=${body.survived} killed=${body.killed}\n`)
})

test("pytest runs a parametrized node id that contains <", { timeout: 120_000, skip: missingPytest() }, () => {
  const dir = fixture()
  mkdirSync(path.join(dir, "src"))
  writeFileSync(path.join(dir, "src", "gate.py"), ["def gate(n):", "    return n > 0", ""].join("\n"))
  writeFileSync(
    path.join(dir, "tests", "test_gate.py"),
    [
      "import pytest",
      "from gate import gate",
      "",
      '@pytest.mark.parametrize("n", [pytest.param(0, id="<lambda>1")])',
      "def test_simple_reject(n):",
      "    assert gate(n) is False",
      "",
    ].join("\n"),
  )
  commit(dir)
  const generated = generate(dir, ["--src", "src"])
  assert.equal(generated.ok, true, generated.stdout)
  const result = run(dir, ["--patches", path.join(dir, "generated", "mutants"), "--no-confirm", "--suite-timeout-ms", "60000"])
  const body = json(result)
  assert.equal(body.ok, true, result.stdout)
  const killed = (body.commands ?? []).find((item) => item.outcome === "killed")
  assert.ok(killed, result.stdout)
  assert.match(killed.command, /test_simple_reject\[<lambda>1\]/)
  assert.equal(/-k /.test(killed.command), false, killed.command)
  const saved = JSON.parse(readFileSync(path.join(dir, "out", "results", `${killed.id}.json`), "utf8")) as { outcome: string; killedBy: string[] }
  assert.equal(saved.outcome, "killed")
  const nodeId = "tests/test_gate.py::test_simple_reject[<lambda>1]"
  assert.ok(saved.killedBy.includes(nodeId), JSON.stringify(saved.killedBy))
  assert.equal(saved.killedBy.some((item) => item.includes(`test_gate.py::${nodeId}`)), false, JSON.stringify(saved.killedBy))
  assert.equal(saved.killedBy.includes("::pytest"), false, JSON.stringify(saved.killedBy))
  save("step-pytest-nodeid", `${result.stdout}\ncommand=${killed.command}\nkilledBy=${saved.killedBy.join(",")}\n`)
})

test("pytest killedBy is the node id once", { timeout: 60_000, skip: missingPytest() }, () => {
  const dir = fixture()
  mkdirSync(path.join(dir, "src"))
  const before = "def gate(n):\n    return n > 0\n"
  const after = "def gate(n):\n    return n >= 0\n"
  writeFileSync(path.join(dir, "src", "gate.py"), before)
  writeFileSync(
    path.join(dir, "tests", "test_gate.py"),
    ["import pytest", "from gate import gate", "", "def test_low():", "    assert gate(0) is False", ""].join("\n"),
  )
  commit(dir)
  writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.py", before, after))
  const result = run(dir, ["--no-confirm", "--suite-timeout-ms", "60000"])
  const body = json(result)
  assert.equal(body.ok, true, result.stdout)
  const killed = (body.commands ?? []).find((item) => item.outcome === "killed")
  assert.ok(killed, result.stdout)
  const saved = JSON.parse(readFileSync(path.join(dir, "out", "results", `${killed.id}.json`), "utf8")) as { killedBy: string[] }
  const nodeId = "tests/test_gate.py::test_low"
  assert.deepEqual(saved.killedBy, [nodeId], JSON.stringify(saved.killedBy))
  assert.equal(saved.killedBy.some((name) => name.includes(`test_gate.py::${nodeId}`)), false, JSON.stringify(saved.killedBy))
  const tallied = launch(["mutate", "tally", "--out", path.join(dir, "out")])
  const tally = JSON.parse(tallied.stdout) as { ok: boolean; keep: string[]; pruning: { deletedTests: number } }
  assert.equal(tally.ok, true, tallied.stdout)
  assert.deepEqual(tally.keep, [nodeId], JSON.stringify(tally.keep))
  assert.equal(tally.pruning.deletedTests, 0)
})

test("node coverage marks an uncovered line and runs only the tests that hit a covered line", { timeout: 120_000 }, () => {
  const dir = nodeCoveragePackage(true)
  commit(dir)
  const generated = generate(dir, ["--src", "src"])
  assert.equal(generated.ok, true, generated.stdout)
  const result = run(dir, ["--patches", path.join(dir, "generated", "mutants"), "--no-confirm", "--suite-timeout-ms", "60000"])
  const body = json(result)
  assert.equal(body.ok, true, result.stdout)
  const uncovered = (body.commands ?? []).filter((item) => item.outcome === "no coverage")
  assert.ok(uncovered.length > 0, result.stdout)
  for (const item of uncovered) {
    assert.notEqual(item.outcome, "survived")
    assert.match(item.next, /No coverage/)
    assert.match(item.next, /No test executed/)
  }
  const map = JSON.parse(readFileSync(path.join(dir, "out", "coverage-map.json"), "utf8")) as { files: Record<string, Record<string, string[]>> }
  const gate = map.files["src/gate.ts"]
  assert.ok(gate, result.stdout)
  assert.deepEqual(gate["2"], ["test_low"])
  assert.equal(gate["8"], undefined)
  const covered = (body.commands ?? []).find((item) => item.command.includes("test_low") && !item.command.includes("test_wide"))
  assert.ok(covered, result.stdout)
  assert.match(covered.command, /--test-name-pattern=/)
  assert.equal(covered.command.includes("low.test.ts") && covered.command.includes("wide.test.ts"), false, covered.command)
  assert.ok(covered.command.includes("low.test.ts"), covered.command)
  const full = body.commands ?? []
  assert.ok(full.length > 1, result.stdout)
  save("node-cov", result.stdout)
  save("step-node-cov", `${result.stdout}\nnext=${body.next}\nnoCoverage=${body.noCoverage} killed=${body.killed} survived=${body.survived}\n`)
})

test("C LLVM coverage skips an uncovered line and runs a covered mutant from the map", { timeout: 120_000, skip: firstSkip(missingTool("clang"), missingTool("llvm-cov"), missingTool("llvm-profdata")) }, () => {
  const dir = fixture()
  writeFileSync(
    path.join(dir, "tests", "main.c"),
    ["int gate(int n) { return n > 0 && n < 10; }", "int unused(int n) { return n > 2 && n < 3; }", "int main(void) {", "  if (gate(0)) return 1;", "  if (!gate(1)) return 1;", "  return 0;", "}", ""].join("\n"),
  )
  commit(dir)
  const generated = generate(dir, ["--src", "tests"])
  assert.equal(generated.ok, true, generated.stdout)
  const command = "clang -fprofile-instr-generate -fcoverage-mapping -o tests/gate tests/main.c && tests/gate"
  const result = run(dir, ["--patches", path.join(dir, "generated", "mutants"), "--suite-command", command, "--no-confirm", "--suite-timeout-ms", "60000"])
  const body = json(result)
  const errorFile = path.join(dir, "out", "coverage-error.txt")
  if (!existsSync(path.join(dir, "out", "coverage-map.json"))) {
    const detail = existsSync(errorFile) ? readFileSync(errorFile, "utf8") : ""
    // A tool that cannot start is an environment limit. A tool that started and wrote no map is a product failure.
    if (/llvm tool missing/.test(detail)) {
      save("c-cov-launcher", detail)
      return
    }
    assert.fail(detail || result.stderr || result.stdout)
  }
  assert.equal(body.ok, true, result.stdout)
  const map = JSON.parse(readFileSync(path.join(dir, "out", "coverage-map.json"), "utf8")) as { files: Record<string, Record<string, string[]>> }
  assert.ok(Object.keys(map.files).length > 0, "llvm-cov wrote an empty map")
  const uncovered = (body.commands ?? []).find((item) => item.outcome === "no coverage")
  const covered = (body.commands ?? []).find((item) => item.outcome !== "no coverage" && item.command.includes("tests/gate"))
  assert.ok(uncovered, result.stdout)
  assert.notEqual(uncovered?.outcome, "survived")
  assert.ok(covered, result.stdout)
  assert.match(JSON.stringify(body), /cannot take a test name/)
  save("c-cov", result.stdout)
  save("step-c-cov", `${result.stdout}\nnext=${body.next}\nnoCoverage=${body.noCoverage}\n`)
})

test("a name filter is passed and a command that cannot take one runs the whole suite", { timeout: 120_000, skip: firstSkip(missingTool("go"), missingTool("clang")) }, () => {
  const goDir = fixture()
  writeFileSync(path.join(goDir, "go.mod"), "module example.com/gate\n\ngo 1.22\n")
  writeFileSync(path.join(goDir, "gate.go"), "package gate\n\nfunc Gate(n int) bool { return n > 0 && n < 10 }\n")
  writeFileSync(
    path.join(goDir, "gate_test.go"),
    ["package gate", "", 'import "testing"', "", "func TestLow(t *testing.T) {", '  if Gate(0) { t.Fatal("low") }', "}", "func TestInside(t *testing.T) {", '  if !Gate(1) { t.Fatal("inside") }', "}", ""].join("\n"),
  )
  commit(goDir)
  const generated = generate(goDir, ["--src", "."])
  assert.equal(generated.ok, true, generated.stdout)
  const filtered = run(goDir, ["--patches", path.join(goDir, "generated", "mutants"), "--max-mutants", "1", "--suite-timeout-ms", "60000"])
  const filteredBody = json(filtered)
  assert.equal(filteredBody.ok, true, filtered.stdout)
  const joined = (filteredBody.commands ?? []).map((item) => item.command).join("\n")
  assert.match(joined, /-run /)

  const plain = fixture()
  const source = "int main(void) { int n = 0; if (n > 0) return 1; return 0; }\n"
  writeFileSync(path.join(plain, "tests", "main.c"), source)
  writeFileSync(path.join(plain, "patches", "m-one.patch"), forwardDiff("tests/main.c", source, source.replace("n > 0", "n >= 0")))
  commit(plain)
  const command = "clang -std=c11 -o tests/gate tests/main.c && tests/gate"
  const whole = run(plain, ["--suite-command", command, "--suite-timeout-ms", "30000"])
  const wholeBody = json(whole)
  assert.equal(wholeBody.ok, true, whole.stdout)
  assert.match(wholeBody.next, /cannot take a test name/)
  assert.match(wholeBody.next, /whole suite/)
  const evidence = {
    schemaVersion: filteredBody.schemaVersion,
    ok: filteredBody.ok && wholeBody.ok,
    command: filteredBody.command,
    summary: filteredBody.summary,
    next: wholeBody.next,
    nextCall: filteredBody.nextCall,
    commands: filteredBody.commands,
    filtered: filteredBody,
    whole: wholeBody,
  }
  save("name-filter", `${JSON.stringify(evidence, null, 2)}\n`)
  save("step-name-filter", `${JSON.stringify(evidence)}\nnext=${evidence.next}\n`)
})

test("a second run reuses an unchanged mutant and its killing test", { timeout: 120_000 }, () => {
  const dir = nodePackage(fastSource())
  const generated = generate(dir)
  assert.equal(generated.ok, true, generated.stdout)
  const first = run(dir, ["--patches", path.join(dir, "generated", "mutants"), "--no-confirm", "--max-mutants", "1", "--suite-timeout-ms", "30000"])
  const firstBody = json(first)
  assert.equal(firstBody.ok, true, first.stdout)
  const stored = (firstBody.commands ?? []).find((item) => item.outcome === "killed" || item.outcome === "survived")
  assert.ok(stored, first.stdout)
  assert.equal(first.stderr.includes(`${stored.id} start`), true, first.stderr)
  const secondOut = path.join(dir, "out-2")
  const second = run(dir, ["--patches", path.join(dir, "generated", "mutants"), "--no-confirm", "--max-mutants", "1", "--history", path.join(dir, "out"), "--out", secondOut, "--suite-timeout-ms", "30000"])
  const secondBody = json(second)
  assert.equal(secondBody.ok, true, second.stdout)
  const again = (secondBody.commands ?? []).find((item) => item.id === stored.id)
  assert.ok(again, second.stdout)
  assert.equal(again?.reused, true)
  assert.equal(again?.outcome, stored.outcome)
  assert.equal(again?.command ?? "", "")
  assert.equal(second.stderr.includes(`reused ${stored.id}`), true, second.stderr)
  assert.equal(second.stderr.includes(`${stored.id} start`), false, second.stderr)
  save("history-1", first.stdout)
  save("history-2", second.stdout)
  save("step-history", `first=${stored.outcome}\nsecond=${again?.outcome}\nreused=${again?.reused}\n`)
})

function fixture(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-agent-"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  return dir
}

function nodeCoveragePackage(useTsx: boolean): string {
  const dir = fixture()
  mkdirSync(path.join(dir, "src"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(
    path.join(dir, "src", "gate.ts"),
    [
      "export function low(n: number): boolean {",
      "  return n < 10",
      "}",
      "export function wide(n: number): boolean {",
      "  return n > 0",
      "}",
      "export function unused(n: number): boolean {",
      "  return n > 2",
      "}",
      "",
    ].join("\n"),
  )
  writeFileSync(
    path.join(dir, "tests", "low.test.ts"),
    [
      'import test from "node:test"',
      'import assert from "node:assert/strict"',
      'import { low } from "../src/gate.ts"',
      'test("test_low", () => { assert.equal(low(10), false) })',
      "",
    ].join("\n"),
  )
  writeFileSync(
    path.join(dir, "tests", "wide.test.ts"),
    [
      'import test from "node:test"',
      'import assert from "node:assert/strict"',
      'import { wide } from "../src/gate.ts"',
      'test("test_wide", () => { assert.equal(wide(0), false) })',
      "",
    ].join("\n"),
  )
  if (useTsx) {
    const dest = path.join(dir, "node_modules", ".bin")
    mkdirSync(dest, { recursive: true })
    symlinkSync(tsx, path.join(dest, "tsx"))
  }
  return dir
}

function nodePackage(source: string): string {
  const dir = fixture()
  mkdirSync(path.join(dir, "src"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "src", "gate.js"), source)
  writeFileSync(
    path.join(dir, "tests", "gate.test.js"),
    [
      'import test from "node:test"',
      'import assert from "node:assert/strict"',
      'import { gate, pause } from "../src/gate.js"',
      'test("fast", () => { assert.equal(gate(0), false) })',
      'test("pause", () => { assert.equal(pause(1), true) })',
      "",
    ].join("\n"),
  )
  commit(dir)
  return dir
}

function fastSource(): string {
  return ["export function gate(n) {", "  return n > 0 && n < 10", "}", "export function pause(flag) {", "  return flag > 0 && flag < 10", "}", ""].join("\n")
}

function pauseSource(): string {
  return [
    "export function gate(n) {",
    "  return n > 0 && n < 10",
    "}",
    "export function pause(flag) {",
    "  if (flag < 1) {",
    "    const end = Date.now() + 120000",
    "    while (Date.now() < end) {}",
    "  }",
    "  return true",
    "}",
    "",
  ].join("\n")
}

function commit(dir: string): void {
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
}

function generate(dir: string, extra: string[] = ["--src", "src"]): Body & { stdout: string } {
  const result = launch(["mutate", "generate", "--package", dir, "--out", path.join(dir, "generated"), ...extra, "--max-minutes", "1"])
  const body = json(result)
  return { ...body, stdout: result.stdout }
}

function run(dir: string, extra: string[], env?: NodeJS.ProcessEnv): { stdout: string; stderr: string; status: number | null } {
  return launch(directRun(dir, extra), env)
}

function directRun(dir: string, extra: string[]): string[] {
  const suppliedPatches = extra.includes("--patches")
  const suppliedOut = extra.includes("--out")
  return [
    "mutate",
    "run",
    "--package",
    dir,
    "--repo",
    dir,
    ...(suppliedPatches ? [] : ["--patches", path.join(dir, "patches")]),
    ...(suppliedOut ? [] : ["--out", path.join(dir, "out")]),
    "--no-build",
    "--workers",
    "1",
    ...extra,
  ]
}

function launch(args: string[], env?: NodeJS.ProcessEnv): { stdout: string; stderr: string; status: number | null } {
  const result = spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", env: { ...process.env, ...env } })
  assert.equal(result.status === 0 || (result.stdout ?? "").trim().startsWith("{"), true, `${result.stderr ?? ""}${result.stdout ?? ""}`)
  return { stdout: result.stdout ?? "", stderr: result.stderr ?? "", status: result.status }
}

function json(result: { stdout: string }): Body {
  return JSON.parse(result.stdout) as Body
}

function git(repo: string, args: string[]): void {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}

function save(name: string, body: string): void {
  if (!scratch) return
  mkdirSync(scratch, { recursive: true })
  let target: string
  if (name === "c-cov-launcher") target = "c-cov-launcher.txt"
  else if (name === "name-filter" || name === "history-1" || name === "history-2") target = `${name}.json`
  else if (name.startsWith("step-")) target = `${name}-${pass}.log`
  else target = `${name}-${pass}.json`
  writeFileSync(path.join(scratch, target), body.endsWith("\n") ? body : `${body}\n`)
}
