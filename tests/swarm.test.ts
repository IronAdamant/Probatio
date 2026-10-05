import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("a queue claim is an atomic rename and an expired lease returns", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-queue-"))
  try {
    const seeded = run(["queue", "seed", "--state", dir, "--id", "m-one"])
    assert.equal(seeded.status, 0, seeded.stderr)
    const queueFile = path.join(dir, "queue", "m-one.json")
    assert.equal(existsSync(queueFile), true)
    const claimed = run(["queue", "claim", "--state", dir, "--id", "m-one", "--agent", "agent-1", "--now", "1000", "--lease-ms", "500"])
    assert.equal(claimed.status, 0, claimed.stderr)
    const claimFile = path.join(dir, "claimed", "agent-1-m-one.json")
    assert.equal(existsSync(queueFile), false)
    assert.equal(existsSync(claimFile), true)
    const body = JSON.parse(readFileSync(claimFile, "utf8")) as { leasedUntil: number }
    assert.equal(body.leasedUntil, 1500)
    const again = run(["queue", "claim", "--state", dir, "--id", "m-one", "--agent", "agent-2", "--now", "1000", "--lease-ms", "500"])
    assert.equal(again.status, 1)
    const early = run(["queue", "reap", "--state", dir, "--now", "1499"])
    assert.equal(early.status, 0, early.stderr)
    assert.deepEqual(JSON.parse(early.stdout).returned, [])
    assert.equal(existsSync(claimFile), true)
    const late = run(["queue", "reap", "--state", dir, "--now", "1500"])
    assert.equal(late.status, 0, late.stderr)
    assert.deepEqual(JSON.parse(late.stdout).returned, ["m-one"])
    assert.equal(existsSync(queueFile), true)
    assert.equal(existsSync(claimFile), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("check-kill is done only when the mutant dies and the suite is green", () => {
  const killed = packageWith("export const n = 1\n", "export const n = 2\n", "assert.equal(n, 1)\n")
  const survived = packageWith("export const n = 1\nexport const label = \"a\"\n", "export const n = 1\nexport const label = \"b\"\n", "assert.equal(n, 1)\n")
  const red = packageWith("export const n = 1\n", "export const n = 2\n", "assert.equal(n, 2)\n")
  try {
    const dead = check(killed)
    assert.equal(dead.status, 0, dead.stderr)
    const deadJson = JSON.parse(dead.stdout) as { done: boolean; suiteGreen: boolean; outcome: string }
    assert.equal(deadJson.done, true)
    assert.equal(deadJson.suiteGreen, true)
    assert.equal(deadJson.outcome, "killed")

    const live = check(survived)
    assert.equal(live.status, 1, live.stdout)
    const liveJson = JSON.parse(live.stdout) as { done: boolean; outcome: string }
    assert.equal(liveJson.done, false)
    assert.equal(liveJson.outcome, "survived")

    const failing = check(red)
    assert.equal(failing.status, 1, failing.stdout)
    const failingJson = JSON.parse(failing.stdout) as { done: boolean; suiteGreen: boolean }
    assert.equal(failingJson.done, false)
    assert.equal(failingJson.suiteGreen, false)
  } finally {
    for (const item of [killed, survived, red]) rmSync(item.dir, { recursive: true, force: true })
  }
})

test("check-kill runs only the direct importer and confirms the kill in isolation", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-kill-scope-"))
  const ran = path.join(dir, "ran")
  mkdirSync(ran)
  const otherMark = path.join(ran, "other")
  const hits = path.join(ran, "hits")
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  const before = "export const mode = \"stable\"\n"
  writeFileSync(path.join(dir, "src", "gate.ts"), before)
  writeFileSync(
    path.join(dir, "tests", "gate.test.ts"),
    [
      "import test from \"node:test\"",
      "import assert from \"node:assert/strict\"",
      "import { existsSync, mkdirSync, readFileSync, writeFileSync } from \"node:fs\"",
      "import { mode } from \"../src/gate.ts\"",
      `const hits = ${JSON.stringify(hits)}`,
      "test(\"gate holds\", () => {",
      "  if (mode === \"stable\") return",
      "  mkdirSync(hits, { recursive: true })",
      "  const file = hits + \"/\" + mode",
      "  const seen = existsSync(file) ? Number(readFileSync(file, \"utf8\")) : 0",
      "  writeFileSync(file, String(seen + 1))",
      "  if (mode === \"die\") assert.fail(\"dead\")",
      "  if (mode === \"once\" && seen === 0) assert.fail(\"first sight\")",
      "})",
      "",
    ].join("\n"),
  )
  writeFileSync(
    path.join(dir, "tests", "other.test.ts"),
    [
      "import test from \"node:test\"",
      "import { writeFileSync } from \"node:fs\"",
      "test(\"other stays quiet\", () => {",
      `  writeFileSync(${JSON.stringify(otherMark)}, "ran\\n")`,
      "})",
      "",
    ].join("\n"),
  )
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  const stableDir = path.join(dir, "stable-patches")
  const onceDir = path.join(dir, "once-patches")
  mkdirSync(stableDir)
  mkdirSync(onceDir)
  writeFileSync(path.join(stableDir, "stable.patch"), forwardDiff("src/gate.ts", before, "export const mode = \"die\"\n"))
  writeFileSync(path.join(onceDir, "once.patch"), forwardDiff("src/gate.ts", before, "export const mode = \"once\"\n"))
  try {
    const stable = run([
      "check-kill", "stable",
      "--package", dir,
      "--out", path.join(dir, "stable-out"),
      "--patches", stableDir,
      "--suite-timeout-ms", "20000",
    ])
    assert.equal(stable.status, 0, stable.stderr + stable.stdout)
    const stableJson = JSON.parse(stable.stdout) as { ok: boolean; done: boolean; outcome: string; summary: string; suiteGreen: boolean }
    assert.equal(stableJson.ok, true)
    assert.equal(stableJson.done, true)
    assert.equal(stableJson.suiteGreen, true)
    assert.equal(stableJson.outcome, "killed")
    assert.match(stableJson.summary, /stable is dead and the suite is green/)
    assert.equal(existsSync(otherMark), false)

    const once = run([
      "check-kill", "once",
      "--package", dir,
      "--out", path.join(dir, "once-out"),
      "--patches", onceDir,
      "--suite-timeout-ms", "20000",
    ])
    assert.equal(once.status, 1, once.stdout)
    const onceJson = JSON.parse(once.stdout) as { ok: boolean; done: boolean; outcome: string }
    assert.equal(onceJson.ok, false)
    assert.equal(onceJson.done, false)
    assert.equal(onceJson.outcome, "flaky")
    assert.equal(existsSync(otherMark), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function check(fixture: { dir: string; patches: string }) {
  return run(["check-kill", "bug", "--package", fixture.dir, "--out", path.join(fixture.dir, "out"), "--patches", fixture.patches, "--no-confirm"])
}

function packageWith(before: string, after: string, assertion: string) {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-kill-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  const patches = path.join(dir, "patches")
  mkdirSync(patches)
  writeFileSync(path.join(dir, "src", "n.ts"), before)
  writeFileSync(
    path.join(dir, "tests", "n.test.ts"),
    `import test from "node:test"\nimport assert from "node:assert/strict"\nimport { n } from "../src/n.ts"\ntest("n stays", () => { ${assertion}})`,
  )
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  writeFileSync(path.join(patches, "bug.patch"), forwardDiff("src/n.ts", before, after))
  return { dir, patches }
}

function run(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8" })
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
  return result
}
