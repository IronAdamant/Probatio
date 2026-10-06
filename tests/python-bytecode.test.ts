import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { runDiscoveredSuite } from "../src/mutate/suites.ts"
import { missingTool } from "./require-tool.ts"

const fresh = ["def open(n):", "    return n >  0", ""].join("\n")
const mutated = ["def open(n):", "    return n >= 0", ""].join("\n")

test("a python suite reads the patched source when a matching bytecode file is left behind", { skip: missingTool("python3") }, async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-pyc-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  const gate = path.join(dir, "src", "gate.py")
  writeFileSync(gate, fresh)
  writeFileSync(
    path.join(dir, "tests", "test_gate.py"),
    ["import unittest", "from gate import open as gate_open", "class GateTest(unittest.TestCase):", "    def test_closed(self):", "        self.assertFalse(gate_open(0))", ""].join("\n"),
  )
  const compiled = spawnSync("python3", ["-c", "import py_compile; py_compile.compile('src/gate.py', doraise=True)"], { cwd: dir, encoding: "utf8" })
  assert.equal(compiled.status, 0, compiled.stderr)
  const stamp = statSync(gate)
  writeFileSync(gate, mutated)
  utimesSync(gate, stamp.atime, stamp.mtime)
  try {
    const result = await runDiscoveredSuite(dir, [], "tests", null, 30_000, process.env, {
      reporterPath: "",
      testTimeoutMs: 5_000,
      concurrency: 1,
    })
    assert.equal(result.compileToken, null)
    assert.ok(result.report, result.detail)
    assert.ok(result.report.fail > 0, JSON.stringify(result.report))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
