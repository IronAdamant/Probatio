import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { labelLeaked, readSealedLabel } from "../src/mutate/sealed.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

const source = "export function gate(n: number): boolean {\n  return n > 0\n}\n"

test("mutate sealed keeps the label out of the report and hides the revealing test", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-sealed-"))
  const labelFile = path.join(dir, "label.txt")
  writeFileSync(labelFile, "bug_id=seal-gate-9f3\nissue_id=issue-gate-9f3\nexpected_fail=zero stays shut\n")
  const label = readSealedLabel(labelFile)
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "src", "gate.ts"), source)
  writeFileSync(
    path.join(dir, "tests", "open.test.ts"),
    "import test from \"node:test\"\nimport assert from \"node:assert/strict\"\nimport { gate } from \"../src/gate.ts\"\ntest(\"one stays open\", () => { assert.equal(gate(1), true) })\n",
  )
  writeFileSync(
    path.join(dir, "tests", "shut.test.ts"),
    "import test from \"node:test\"\nimport assert from \"node:assert/strict\"\nimport { gate } from \"../src/gate.ts\"\ntest(\"zero stays shut\", () => { assert.equal(gate(0), false) })\n",
  )
  writeFileSync(path.join(dir, "patches", "m1.patch"), forwardDiff("src/gate.ts", source, source.replace("n > 0", "n >= 0")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const first = sealed(dir, labelFile, path.join(dir, "out-1"))
    const second = sealed(dir, labelFile, path.join(dir, "out-2"))
    assert.equal(first.status, 0, first.stderr + first.stdout)
    assert.equal(second.status, 0, second.stderr + second.stdout)
    const a = JSON.parse(first.stdout) as { schemaVersion: number; ok: boolean; summary: string; killed: number; survived: number }
    const b = JSON.parse(second.stdout) as { schemaVersion: number; ok: boolean; summary: string; killed: number; survived: number }
    assert.equal(a.schemaVersion, 1)
    assert.equal(b.schemaVersion, 1)
    assert.equal(a.ok, true, a.summary)
    assert.equal(a.summary, b.summary)
    assert.equal(a.killed, 0, a.summary)
    assert.equal(a.survived, 1, a.summary)
    assert.equal(labelLeaked(first.stdout, label), false)
    assert.equal(labelLeaked(second.stdout, label), false)
    assert.equal(first.stdout.includes("shut.test.ts"), false, first.stdout)
    assert.equal(first.stdout.includes("zero stays shut"), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("--hide with a parent segment is refused and the outside file stays", () => {
  const parent = mkdtempSync(path.join(tmpdir(), "probatio-hide-"))
  const secret = path.join(parent, "secret.txt")
  const dir = path.join(parent, "pkg")
  writeFileSync(secret, "keep\n")
  mkdirSync(dir)
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--patches", dir, "--out", path.join(dir, "out"), "--hide", "../secret.txt", "--no-confirm"],
      { cwd: root, encoding: "utf8" },
    )
    assert.equal(existsSync(secret), true)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string }
    assert.equal(body.ok, false)
    assert.match(body.summary, /hide/)
    assert.match(body.summary, /secret\.txt/)
  } finally {
    rmSync(parent, { recursive: true, force: true })
  }
})

test("a sealed kill by a test the fix commit edited is not called a catch by older tests", { timeout: 120_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-sealed-edited-"))
  const labelFile = path.join(dir, "label.txt")
  writeFileSync(labelFile, "bug_id=seal-edit-71c\nissue_id=issue-edit-71c\nexpected_fail=zero stays shut\n")
  const buggy = source.replace("n > 0", "n >= 0")
  const openTest = (extra: string) =>
    `import test from "node:test"\nimport assert from "node:assert/strict"\nimport { gate } from "../src/gate.ts"\ntest("one stays open", () => {\n  assert.equal(gate(1), true)\n${extra}})\n`
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "src", "gate.ts"), buggy)
  writeFileSync(path.join(dir, "tests", "open.test.ts"), openTest(""))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "before the fix"])
  // The fix: the source, a new revealing test, and an edit to an older test that now also sees the bug.
  writeFileSync(path.join(dir, "src", "gate.ts"), source)
  writeFileSync(path.join(dir, "tests", "open.test.ts"), openTest("  assert.equal(gate(0), false)\n"))
  writeFileSync(
    path.join(dir, "tests", "shut.test.ts"),
    "import test from \"node:test\"\nimport assert from \"node:assert/strict\"\nimport { gate } from \"../src/gate.ts\"\ntest(\"zero stays shut\", () => { assert.equal(gate(0), false) })\n",
  )
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "fix"])
  writeFileSync(path.join(dir, "patches", "m1.patch"), forwardDiff("src/gate.ts", source, buggy))
  try {
    const run = sealed(dir, labelFile, path.join(dir, "out"))
    assert.equal(run.status, 0, run.stderr + run.stdout)
    const body = JSON.parse(run.stdout) as { ok: boolean; summary: string; next: string; killed: number; fixEdited?: string[]; olderTestCatch?: boolean }
    assert.equal(body.killed, 1, body.summary)
    assert.deepEqual(body.fixEdited, ["tests/open.test.ts"], run.stdout)
    assert.equal(body.olderTestCatch, false, run.stdout)
    assert.match(body.summary, /edited by the fix commit/)
    assert.equal(labelLeaked(run.stdout, readSealedLabel(labelFile)), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function sealed(dir: string, label: string, out: string) {
  return spawnSync(
    tsx,
    [
      "src/cli.ts",
      "mutate",
      "sealed",
      "--package",
      dir,
      "--repo",
      dir,
      "--patches",
      path.join(dir, "patches"),
      "--out",
      out,
      "--label",
      label,
      "--hide",
      "tests/shut.test.ts",
      "--no-build",
      "--no-confirm",
      "--workers",
      "1",
    ],
    { cwd: root, encoding: "utf8" },
  )
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
