import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { buildMochaArgs, discoverSuite } from "../src/mutate/suites.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("pytest discovery accepts tests_*.py", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-tests-star-"))
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "pytest.ini"), "[pytest]\n")
  writeFileSync(path.join(dir, "tests", "tests_gate.py"), "def test_closed():\n    assert True\n")
  try {
    const spec = discoverSuite(dir, "tests")
    assert.ok(spec, "tests_*.py was not discovered")
    assert.equal(spec.kind, "pytest")
    assert.ok(spec.files.includes("tests/tests_gate.py"), spec.files.join(","))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("pytest discovery stays inside the project's testpaths", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-pytest-paths-"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "examples", "javascript", "tests"), { recursive: true })
  writeFileSync(
    path.join(dir, "pyproject.toml"),
    ["[tool.pytest.ini_options]", 'testpaths = ["tests"]', ""].join("\n"),
  )
  writeFileSync(path.join(dir, "tests", "test_app.py"), "def test_ok():\n    assert True\n")
  writeFileSync(
    path.join(dir, "examples", "javascript", "tests", "conftest.py"),
    "import missing_example\n",
  )
  writeFileSync(
    path.join(dir, "examples", "javascript", "tests", "test_js_example.py"),
    "def test_example():\n    assert True\n",
  )
  try {
    const spec = discoverSuite(dir, "tests")
    assert.ok(spec, "the tests directory was not discovered")
    assert.equal(spec.kind, "pytest")
    assert.ok(spec.files.includes("tests/test_app.py"), spec.files.join(","))
    assert.equal(spec.files.some((file) => file.includes("examples/")), false, spec.files.join(","))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("mocha discovery reads test/*.js and the npm test --require", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-mocha-plain-"))
  mkdirSync(path.join(dir, "test", "support"), { recursive: true })
  writeFileSync(
    path.join(dir, "package.json"),
    `${JSON.stringify({
      scripts: { test: "mocha --require test/support/env --reporter spec test/" },
      devDependencies: { mocha: "^10.0.0" },
    })}\n`,
  )
  writeFileSync(path.join(dir, "test", "support", "env.js"), "process.env.NODE_ENV = 'test'\n")
  writeFileSync(
    path.join(dir, "test", "app.js"),
    "const assert = require('assert')\ndescribe('app', function () {\n  it('answers', function () {\n    assert.equal(1, 1)\n  })\n})\n",
  )
  try {
    const spec = discoverSuite(dir, "test")
    assert.ok(spec, "test/*.js was not discovered")
    assert.equal(spec.kind, "mocha")
    assert.ok(spec.files.includes("test/app.js"), spec.files.join(","))
    assert.equal(spec.files.includes("test/support/env.js"), false)
    assert.match(spec.command, /--require test\/support\/env/)
    const args = buildMochaArgs(dir, spec.files)
    assert.ok(args.includes("--require"), args.join(" "))
    assert.ok(args.includes("test/support/env"), args.join(" "))
    assert.ok(args.includes("test/app.js"), args.join(" "))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a red baseline stops before mutants and does not say caught or killed", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-red-cli-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  const source = "export function gate(n: number): boolean {\n  return n > 0\n}\n"
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "src", "gate.ts"), source)
  writeFileSync(
    path.join(dir, "tests", "gate.test.ts"),
    [
      "import test from \"node:test\"",
      "import assert from \"node:assert/strict\"",
      "import { gate } from \"../src/gate.ts\"",
      "test(\"one stays open\", () => { assert.equal(gate(1), true) })",
      "test(\"zero stays open\", () => { assert.equal(gate(0), true) })",
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.ts", source, source.replace("n > 0", "n >= 0")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  const out = path.join(dir, "out")
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", out, "--no-build", "--no-confirm", "--workers", "1"],
      { cwd: root, encoding: "utf8" },
    )
    assert.equal(result.status, 1, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; survived: number; kills?: unknown[] }
    assert.equal(body.ok, false)
    assert.equal(body.killed, 0)
    assert.equal(/caught|killed/i.test(body.summary), false, body.summary)
    assert.deepEqual(body.kills ?? [], [])
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
