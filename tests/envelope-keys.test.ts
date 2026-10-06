import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { LABEL_LEAK_SUMMARY } from "../src/mutate/sealed.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")
const source = "export function gate(n: number): boolean {\n  return n > 0\n}\n"

test("mutate run and tally keep the envelope keys and scrub a home path", { timeout: 90_000 }, () => {
  const dir = mkdtempSync(path.join(homedir(), "probatio-env-"))
  try {
    const out = path.join(dir, "out")
    writeGate(dir, "tests/gate.test.ts", "zero stays shut", "assert.equal(gate(0), false)")
    const result = cli(["mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", out, "--no-build", "--no-confirm"])
    assert.equal(result.status, 0, result.stderr + result.stdout)
    assert.equal(result.stdout.trim().startsWith("{"), true)
    assert.equal(result.stdout.includes(homedir()), false, result.stdout)
    assert.match(result.stdout, /~/)
    const body = JSON.parse(result.stdout) as {
      schemaVersion: number
      ok: boolean
      command: string
      summary: string
      next: string
      nextCall: unknown
      killed: number
      survived: number
      noCoverage: number
      timeouts: number
      full?: string
      kills?: Array<{ killedBy?: string[]; files?: Array<{ file: string }> }>
    }
    for (const key of ["schemaVersion", "ok", "command", "summary", "next", "nextCall"]) {
      assert.equal(Object.prototype.hasOwnProperty.call(body, key), true, key)
    }
    assert.equal(body.schemaVersion, 1)
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.ok((body.kills?.[0]?.killedBy ?? []).length > 0)
    assert.ok((body.kills?.[0]?.files ?? []).some((file) => file.file.endsWith("gate.ts")))
    assert.equal(String(body.full ?? "").includes(homedir()), false)
    assert.match(String(body.full ?? ""), /^~/)
    const tally = cli(["mutate", "tally", "--out", out])
    assert.equal(tally.status, 0, tally.stderr + tally.stdout)
    const counted = JSON.parse(tally.stdout) as {
      summary: string
      next: string
      nextCall: { argv: string[] } | null
      full?: string
      keep: string[]
      drop: string[]
      gaps: unknown[]
      pruning: { deletedTests: number; mode: string }
    }
    // nextCall.argv stays absolute so a resume can be spawned. Everywhere else the home path is ~.
    const { nextCall, ...shown } = counted
    assert.equal(JSON.stringify(shown).includes(homedir()), false, tally.stdout)
    assert.match(String(counted.full ?? ""), /^~/)
    assert.ok(nextCall && nextCall.argv.includes(out), tally.stdout)
    assert.equal(nextCall.argv.some((arg) => arg.includes("~")), false)
    assert.ok(Array.isArray(counted.keep))
    assert.ok(Array.isArray(counted.drop))
    assert.ok(Array.isArray(counted.gaps))
    assert.equal(counted.pruning.deletedTests, 0)
    assert.equal(counted.pruning.mode, "advisory")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a sealed label that lands in the report is the fixed sentence and the token is absent", { timeout: 60_000 }, () => {
  const dir = mkdtempSync(path.join(homedir(), "probatio-env-seal-"))
  const token = "leak-gate.test.ts"
  try {
    writeGate(dir, `tests/${token}`, "one stays open", "assert.equal(gate(1), true)")
    writeFileSync(path.join(dir, "label.txt"), `bug_id=${token}\nissue_id=pbissue-env\nexpected_fail=pbexpect-env\n`)
    const result = cli([
      "mutate",
      "sealed",
      "--package",
      dir,
      "--repo",
      dir,
      "--patches",
      path.join(dir, "patches"),
      "--out",
      path.join(dir, "out"),
      "--label",
      path.join(dir, "label.txt"),
      "--hide",
      "tests/missing.test.ts",
      "--no-build",
      "--no-confirm",
    ])
    assert.equal(result.status, 1, result.stdout)
    assert.equal(result.stdout.includes(token), false, result.stdout)
    assert.equal(result.stdout.includes(homedir()), false, result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; command: string }
    assert.equal(body.ok, false)
    assert.equal(body.summary, LABEL_LEAK_SUMMARY)
    assert.equal(body.command, "mutate.sealed")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function writeGate(dir: string, testFile: string, title: string, check: string) {
  mkdirSync(path.join(dir, "src"), { recursive: true })
  mkdirSync(path.dirname(path.join(dir, testFile)), { recursive: true })
  mkdirSync(path.join(dir, "patches"), { recursive: true })
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "src", "gate.ts"), source)
  writeFileSync(
    path.join(dir, testFile),
    `import test from "node:test"\nimport assert from "node:assert/strict"\nimport { gate } from "../src/gate.ts"\ntest("${title}", () => { ${check} })\n`,
  )
  writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.ts", source, source.replace("n > 0", "n >= 0")))
  const git = (args: string[]) => {
    const result = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8" })
    if (result.status !== 0) throw new Error(result.stderr || result.stdout)
  }
  git(["init", "-q"])
  git(["add", "."])
  git(["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
}

function cli(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", timeout: 80_000 })
}
