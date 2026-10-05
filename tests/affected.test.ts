import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { affectedTests, directImporters } from "../src/mutate/affected.ts"

test("affected tests follow imports, and a process test reaches every module", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-affected-"))
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "src", "a.ts"), "export const a = 1\n")
    writeFileSync(path.join(dir, "src", "b.ts"), "export const b = 2\n")
    writeFileSync(path.join(dir, "src", "assert_receipt.py"), "def audit():\n    return []\n")
    writeFileSync(path.join(dir, "src", "sandbox.ts"), 'export const py = "assert_receipt.py"\n')
    writeFileSync(path.join(dir, "tests", "a.test.ts"), 'import { a } from "../src/a.ts"\n')
    writeFileSync(path.join(dir, "tests", "sandbox.test.ts"), 'import { py } from "../src/sandbox.ts"\n')
    writeFileSync(path.join(dir, "tests", "spawn.test.ts"), 'import { spawn } from "node:child_process"\nspawn("true")\n')
    const map = affectedTests(dir)
    assert.ok(map["src/a.ts"].includes("tests/a.test.ts"))
    assert.equal(map["src/b.ts"].includes("tests/a.test.ts"), false)
    assert.ok(map["src/a.ts"].includes("tests/spawn.test.ts"))
    assert.ok(map["src/b.ts"].includes("tests/spawn.test.ts"))
    assert.ok(map["src/assert_receipt.py"].includes("tests/sandbox.test.ts"))
    assert.equal(map["src/assert_receipt.py"].includes("tests/a.test.ts"), false)
    const importers = directImporters(dir, ["src/a.ts"])
    assert.deepEqual(importers, ["tests/a.test.ts"])
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("direct importers see a Python test that names the module", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-py-import-"))
  try {
    mkdirSync(path.join(dir, "src", "pkg"), { recursive: true })
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "src", "pkg", "gate.py"), "def open(n):\n    return n > 0\n")
    writeFileSync(path.join(dir, "src", "pkg", "other.py"), "def other():\n    return True\n")
    writeFileSync(path.join(dir, "tests", "test_gate.py"), "import pkg.gate\n\ndef test_low():\n    assert pkg.gate.open(0) is False\n")
    writeFileSync(path.join(dir, "tests", "test_other.py"), "import pkg.other\n\ndef test_other():\n    assert pkg.other.other() is True\n")
    assert.deepEqual(directImporters(dir, ["src/pkg/gate.py"]), ["tests/test_gate.py"])
    assert.deepEqual(affectedTests(dir)["src/pkg/gate.py"], ["tests/test_gate.py"])
    assert.deepEqual(affectedTests(dir)["src/pkg/other.py"], ["tests/test_other.py"])
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
