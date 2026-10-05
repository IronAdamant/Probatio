import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("mutate run kills a Python mutant with python3", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-py-"))
  const real = spawnSync("python3", ["-c", "import sys; print(sys.executable)"], { encoding: "utf8" })
  assert.equal(real.status, 0, real.stderr)
  const python = real.stdout.trim()
  const log = path.join(dir, "runner.log")
  const bin = path.join(dir, "bin")
  mkdirSync(bin)
  writeFileSync(path.join(bin, "python3"), `#!/bin/sh\nprintf '%s\\n' "python3 $*" >> "$PROB_RUNNER_LOG"\nexec ${JSON.stringify(python)} "$@"\n`)
  chmodSync(path.join(bin, "python3"), 0o755)
  const before = "def gate(n):\n    return n > 0 and n < 10\n"
  const after = "def gate(n):\n    return n > 0 or n < 10\n"
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "src", "gate.py"), before)
  writeFileSync(path.join(dir, "src", "__init__.py"), "")
  writeFileSync(
    path.join(dir, "tests", "test_gate.py"),
    [
      "import unittest",
      "from src.gate import gate",
      "",
      "class GateTests(unittest.TestCase):",
      "    def test_inside(self):",
      "        self.assertTrue(gate(1))",
      "    def test_low(self):",
      "        self.assertFalse(gate(0))",
      "    def test_high(self):",
      "        self.assertFalse(gate(10))",
      "",
    ].join("\n"),
  )
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  const patches = path.join(dir, "patches")
  mkdirSync(patches)
  writeFileSync(path.join(patches, "m-and.patch"), forwardDiff("src/gate.py", before, after))
  const env = {
    ...process.env,
    PATH: `${bin}${path.delimiter}${process.env.PATH ?? ""}`,
    PROB_RUNNER_LOG: log,
  }
  try {
    const first = launch(dir, patches, path.join(dir, "out-1"), env)
    const second = launch(dir, patches, path.join(dir, "out-2"), env)
    assert.equal(first.status, 0, first.stderr + first.stdout)
    assert.equal(second.status, 0, second.stderr + second.stdout)
    const firstJson = JSON.parse(first.stdout) as { summary: string; killed: number }
    const secondJson = JSON.parse(second.stdout) as { summary: string; killed: number }
    assert.equal(firstJson.summary, secondJson.summary)
    assert.match(firstJson.summary, /1 killed/)
    assert.equal(firstJson.killed, 1)
    const firstResult = JSON.parse(readFileSync(path.join(dir, "out-1", "results", "m-and.json"), "utf8")) as { id: string; outcome: string }
    const secondResult = JSON.parse(readFileSync(path.join(dir, "out-2", "results", "m-and.json"), "utf8")) as { id: string; outcome: string }
    assert.equal(firstResult.id, secondResult.id)
    assert.equal(firstResult.outcome, "killed")
    assert.equal(secondResult.outcome, "killed")
    const trace = readFileSync(log, "utf8")
    const lines = trace.split("\n").filter((line) => line.length > 0)
    assert.ok(lines.length > 0, trace)
    assert.ok(lines.every((line) => line.startsWith("python3 ")), trace)
    assert.match(trace, /py-reporter\.py/)
    assert.equal(trace.includes("node "), false, trace)
    assert.equal(trace.includes("tsx"), false, trace)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("mutate run keeps a Python traceback out of the baseline summary", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-py-trace-"))
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "tests", "test_raises.py"), "raise RuntimeError('boom')\n")
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  const patches = path.join(dir, "patches")
  mkdirSync(patches)
  writeFileSync(path.join(patches, "m-unused.patch"), "")
  try {
    const result = launch(dir, patches, path.join(dir, "out"), process.env)
    const body = JSON.parse(result.stdout) as { ok: boolean; schemaVersion: number; summary: string; next: string; nextCall: unknown }
    assert.equal(body.schemaVersion, 1)
    assert.equal(body.ok, false)
    assert.equal(body.next.length > 0, true)
    assert.equal(body.nextCall, null)
    assert.equal(body.summary, "baseline suite did not return a test report")
    assert.equal(result.stdout.includes("Traceback"), false, result.stdout)
    assert.equal(result.stdout.includes("RuntimeError"), false, result.stdout)
    assert.equal(result.status, 1)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function launch(dir: string, patches: string, out: string, env: NodeJS.ProcessEnv) {
  return spawnSync(
    tsx,
    ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", patches, "--out", out, "--no-build", "--max-mutants", "1", "--max-minutes", "1", "--workers", "1", "--suite-timeout-ms", "20000"],
    { cwd: root, encoding: "utf8", env },
  )
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
