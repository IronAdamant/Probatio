import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { missingPytest } from "./require-tool.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")

test("the packed binary, not src/cli.ts, prints one JSON object", { timeout: 300_000, skip: missingPytest() }, () => {
  const packDir = mkdtempSync(path.join(tmpdir(), "probatio-pack-"))
  const prefix = mkdtempSync(path.join(tmpdir(), "probatio-prefix-"))
  try {
    const built = spawnSync("npm", ["run", "build"], { cwd: root, encoding: "utf8", timeout: 120_000 })
    assert.equal(built.status, 0, built.stderr + built.stdout)
    const packed = spawnSync("npm", ["pack", "--pack-destination", packDir], { cwd: root, encoding: "utf8", timeout: 60_000 })
    assert.equal(packed.status, 0, packed.stderr + packed.stdout)
    const tarball = readdirSync(packDir).find((name) => name.endsWith(".tgz"))
    assert.ok(tarball, readdirSync(packDir).join(","))
    const installed = spawnSync("npm", ["install", "--prefix", prefix, path.join(packDir, tarball as string)], {
      cwd: prefix,
      encoding: "utf8",
      timeout: 120_000,
    })
    assert.equal(installed.status, 0, installed.stderr + installed.stdout)
    const bin = path.join(prefix, "node_modules", ".bin", "probatio")
    assert.equal(existsSync(bin), true, bin)
    const linked = realpathSync(bin)
    assert.match(linked, /[/\\]dist[/\\]cli\.js$/)
    assert.equal(linked.includes(`${path.sep}src${path.sep}cli.ts`), false, linked)
    assert.equal(linked.startsWith(root + path.sep), false, linked)
    const red = fixture("assert 1 == 2", "def gate(n):\n    return n > 0\n", "def gate(n):\n    return n >= 0\n")
    try {
      const stopped = runPacked(bin, red.dir, path.join(red.dir, "out"))
      assert.equal(stopped.status, 1, stopped.stderr + stopped.stdout)
      const body = oneObject(stopped.stdout)
      assert.equal(body.ok, false, body.summary)
      assert.equal(body.killed ?? 0, 0)
      assert.match(body.summary, /baseline already failing/)
    } finally {
      rmSync(red.dir, { recursive: true, force: true })
    }
    const green = fixture("assert gate(0) is False", "def gate(n):\n    return n > 0\n", "def gate(n):\n    return n >= 0\n")
    try {
      const killed = runPacked(bin, green.dir, path.join(green.dir, "out"))
      assert.equal(killed.status, 0, killed.stderr + killed.stdout)
      const body = oneObject(killed.stdout)
      assert.equal(body.ok, true, body.summary)
      assert.equal(body.killed, 1, body.summary)
      const names = body.kills?.[0]?.killedBy ?? []
      assert.deepEqual(names, ["tests/test_gate.py::test_low"])
      assert.equal(names.filter((name) => name === "tests/test_gate.py::test_low").length, 1)
    } finally {
      rmSync(green.dir, { recursive: true, force: true })
    }
  } finally {
    rmSync(packDir, { recursive: true, force: true })
    rmSync(prefix, { recursive: true, force: true })
  }
})

function oneObject(stdout: string) {
  const trimmed = stdout.trim()
  assert.equal(trimmed.startsWith("{"), true, stdout)
  assert.equal(trimmed.endsWith("}"), true, stdout)
  const body = JSON.parse(trimmed) as {
    schemaVersion: number
    ok: boolean
    command: string
    summary: string
    next: string
    nextCall: unknown
    killed?: number
    kills?: Array<{ killedBy?: string[] }>
  }
  for (const key of ["schemaVersion", "ok", "command", "summary", "next", "nextCall"]) {
    assert.equal(Object.prototype.hasOwnProperty.call(body, key), true, key)
  }
  assert.equal(body.schemaVersion, 2)
  return body
}

function runPacked(bin: string, dir: string, out: string) {
  return spawnSync(
    bin,
    ["mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", out, "--no-build", "--no-confirm", "--workers", "1"],
    { cwd: dir, encoding: "utf8", timeout: 90_000 },
  )
}

function fixture(check: string, before: string, after: string) {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-packed-fx-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "src", "gate.py"), before)
  writeFileSync(path.join(dir, "tests", "test_gate.py"), `import pytest\nfrom gate import gate\n\ndef test_low():\n    ${check}\n`)
  writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.py", before, after))
  const git = (args: string[]) => {
    const result = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8" })
    if (result.status !== 0) throw new Error(result.stderr || result.stdout)
  }
  git(["init", "-q"])
  git(["add", "."])
  git(["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  return { dir }
}
