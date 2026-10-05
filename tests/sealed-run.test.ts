import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
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
