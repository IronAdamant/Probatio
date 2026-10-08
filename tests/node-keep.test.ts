import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

type RunBody = {
  ok: boolean
  summary: string
  killed?: number
  survived?: number
  errors?: number
  kills?: Array<{ killedBy?: string[] }>
  commands?: Array<{ outcome: string; command: string }>
}
type TallyBody = {
  ok: boolean
  summary: string
  keep: string[]
  noKillsYet: string[]
  drop?: unknown
  nextCall: { argv: string[] } | null
  pruning: { deletedTests: number }
}

test("node keep ids are one spelling, honour --only-test, and the tally nextCall really reruns", { timeout: 240_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-node-keep-"))
  try {
    const before = "export function gate(n: number): boolean {\n  return n > 0\n}\n"
    const after = "export function gate(n: number): boolean {\n  return n >= 0\n}\n"
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), before)
    writeFileSync(
      path.join(dir, "tests", "gate.test.ts"),
      [
        'import assert from "node:assert/strict"',
        'import test from "node:test"',
        'import { gate } from "../src/gate.ts"',
        'test("zero stays shut", () => {',
        "  assert.equal(gate(0), false)",
        "})",
        'test("five opens", () => {',
        "  assert.equal(gate(5), true)",
        "})",
        "",
      ].join("\n"),
    )
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.ts", before, after))

    const out = path.join(dir, "out")
    const first = run(dir, out)
    assert.equal(first.ok, true, first.summary)
    assert.equal(first.killed, 1, first.summary)
    const killedBy = first.kills?.[0]?.killedBy ?? []
    assert.equal(killedBy.some((name) => name.endsWith("five opens")), false, `five opens passes on the mutant: ${JSON.stringify(killedBy)}`)

    const tally = tallyOf(out)
    assert.equal(tally.pruning.deletedTests, 0)
    assert.equal(tally.keep.length, 1, `one test killed, one keep id: ${JSON.stringify(tally.keep)}`)
    assert.match(tally.keep[0], /zero stays shut$/)
    assert.equal("drop" in tally, false, "a test that killed nothing is not a deletion list")
    assert.deepEqual(tally.noKillsYet.map((name) => name.replace(/^.*::/, "")), ["five opens"])

    // The nextCall is run exactly as printed. It must score again, not echo the stored result.
    const argv = tally.nextCall?.argv ?? []
    const outAt = argv.indexOf("--out")
    assert.ok(outAt >= 0, JSON.stringify(argv))
    const rescoreOut = argv[outAt + 1]
    assert.notEqual(path.resolve(rescoreOut), path.resolve(out), "the rescore writes a fresh out dir")
    const rescore = JSON.parse(cli(argv).stdout) as RunBody
    assert.equal(rescore.ok, true, rescore.summary)
    assert.equal(rescore.killed, 1, rescore.summary)
    assert.ok(existsSync(path.join(rescoreOut, "baseline.json")), "the rescore ran its own baseline")

    const other = run(dir, path.join(dir, "other"), ["--only-test", "five opens"])
    assert.equal(other.killed ?? 0, 0, `only five opens ran, and it passes on the mutant: ${other.summary}`)
    assert.equal(other.survived, 1, other.summary)

    const missing = run(dir, path.join(dir, "missing"), ["--only-test", "no such test"])
    assert.equal(missing.killed ?? 0, 0, `a test that does not exist cannot kill: ${missing.summary}`)
    assert.equal(missing.survived ?? 0, 0, `a test that does not exist is not a survivor: ${missing.summary}`)

    const qualified = run(dir, path.join(dir, "qualified"), ["--only-test", tally.keep[0]])
    assert.equal(qualified.killed, 1, `the keep id as printed is runnable: ${qualified.summary}`)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a test file that no longer links under a mutant is unviable, not a kill and not a survivor", { timeout: 180_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-node-load-"))
  try {
    const before = "export function gate(n: number): boolean {\n  return n > 0\n}\nexport function shut(): boolean {\n  return !gate(0)\n}\n"
    // A reverted fix can remove an export a test imports. The module graph no longer links, and no test code runs.
    // A compiled language rejects the same change at compile time, so the verdict matches: it did not build.
    const after = "export function gate(n: number): boolean {\n  return n > 0\n}\n"
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), before)
    writeFileSync(
      path.join(dir, "tests", "gate.test.ts"),
      'import assert from "node:assert/strict"\nimport test from "node:test"\nimport { gate, shut } from "../src/gate.ts"\ntest("zero is shut", () => {\n  assert.equal(gate(0), false)\n  assert.equal(shut(), true)\n})\n',
    )
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-gone.patch"), forwardDiff("src/gate.ts", before, after))
    for (const [out, confirm] of [["fast", ["--no-confirm"]], ["confirmed", []]] as const) {
      const body = JSON.parse(
        cli(["mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, out), "--no-build", "--workers", "1", ...confirm]).stdout,
      ) as RunBody & { unviable?: number; flaky?: number }
      assert.equal(body.ok, true, body.summary)
      assert.equal(body.killed ?? 0, 0, `${out}: no assertion ran, so nothing killed it: ${body.summary}`)
      assert.equal(body.survived ?? 0, 0, `${out}: ${body.summary}`)
      assert.equal(body.flaky ?? 0, 0, `${out}: ${body.summary}`)
      assert.equal(body.unviable, 1, `${out}: ${body.summary}`)
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a test file whose top-level code throws under a mutant is a kill by that file", { timeout: 180_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-node-throw-"))
  try {
    const before = 'export function size(n: number): number {\n  if (n > 5) throw new Error("too big")\n  return n\n}\n'
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "size.ts"), before)
    // The mutated code runs at load and throws. That is the program breaking, and the suite saw it.
    writeFileSync(
      path.join(dir, "tests", "size.test.ts"),
      'import assert from "node:assert/strict"\nimport test from "node:test"\nimport { size } from "../src/size.ts"\nconst fixture = size(5)\ntest("five fits", () => {\n  assert.equal(fixture, 5)\n})\n',
    )
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/size.ts", before, before.replace("n > 5", "n >= 5")))
    for (const [out, confirm] of [["fast", ["--no-confirm"]], ["confirmed", []]] as const) {
      const body = JSON.parse(
        cli(["mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, out), "--no-build", "--workers", "1", ...confirm]).stdout,
      ) as RunBody & { unviable?: number; flaky?: number }
      assert.equal(body.ok, true, body.summary)
      assert.equal(body.killed, 1, `${out}: ${body.summary}`)
      assert.equal(body.unviable ?? 0, 0, `${out}: ${body.summary}`)
      assert.match(body.kills?.[0]?.killedBy?.[0] ?? "", /size\.test\.ts$/)
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a TypeScript source that imports its sibling as ./x.js loads on the node batch path", { timeout: 120_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-node-js-specifier-"))
  try {
    // tsc with NodeNext writes `./limit.js` for limit.ts. tsx resolves it, and so must the batch.
    const before = 'import { LIMIT } from "./limit.js"\nexport function gate(n: number): boolean {\n  return n > LIMIT\n}\n'
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "limit.ts"), "export const LIMIT: number = 0\n")
    // Such a project runs its own tests with tsx. The baseline uses it, and the batch must agree.
    symlinkSync(path.join(root, "node_modules"), path.join(dir, "node_modules"), "dir")
    writeFileSync(path.join(dir, ".gitignore"), "node_modules\n")
    writeFileSync(path.join(dir, "src", "gate.ts"), before)
    writeFileSync(
      path.join(dir, "tests", "gate.test.ts"),
      'import assert from "node:assert/strict"\nimport test from "node:test"\nimport { gate } from "../src/gate.ts"\ntest("zero stays shut", () => {\n  assert.equal(gate(0), false)\n})\n',
    )
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.ts", before, before.replace("n > LIMIT", "n >= LIMIT")))
    const body = run(dir, path.join(dir, "out"))
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.deepEqual(body.kills?.[0]?.killedBy?.map((name) => name.replace(/^.*::/, "")), ["zero stays shut"], "the named test killed it, not a file that failed to load")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function run(dir: string, out: string, extra: string[] = []): RunBody {
  const result = cli(["mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", out, "--no-build", "--no-confirm", "--workers", "1", ...extra])
  assert.equal(result.stdout.trim().startsWith("{"), true, result.stderr + result.stdout)
  return JSON.parse(result.stdout) as RunBody
}

function tallyOf(out: string): TallyBody {
  const result = cli(["mutate", "tally", "--out", out])
  assert.equal(result.status, 0, result.stderr + result.stdout)
  return JSON.parse(result.stdout) as TallyBody
}

function cli(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", timeout: 200_000 })
}

function commit(repo: string) {
  const git = (args: string[]) => {
    const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
    if (result.status !== 0) throw new Error(result.stderr || result.stdout)
  }
  git(["init", "-q"])
  git(["add", "."])
  git(["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
}

test("a link or parse error is static, and a SyntaxError thrown by running code is not", async () => {
  const { staticLoadFailure } = (await import("../src/mutate/load-failure.mjs")) as { staticLoadFailure: (text: string) => boolean }
  const link = "SyntaxError: The requested module './lib.mjs' does not provide an export named 'gone'\n    at ModuleJob._instantiate (node:internal/modules/esm/module_job:226:21)\n"
  assert.equal(staticLoadFailure(link), true)
  const missing = "Error [ERR_MODULE_NOT_FOUND]: Cannot find module '/repo/src/gone.ts' imported from /repo/tests/a.test.ts\n    at finalizeResolution (node:internal/modules/esm/resolve:275:11)\n"
  assert.equal(staticLoadFailure(missing), true)
  // tsx frames live under node_modules. They are the loader, not the project's code.
  const viaTsx = "SyntaxError: Unexpected token\n    at transform (/repo/node_modules/tsx/dist/index.mjs:1:200)\n"
  assert.equal(staticLoadFailure(viaTsx), true)
  const running = "SyntaxError: Unexpected token } in JSON at position 3\n    at JSON.parse (<anonymous>)\n    at file:///repo/tests/fixture.test.ts:4:22\n"
  assert.equal(staticLoadFailure(running), false)
  assert.equal(staticLoadFailure("Error: boom at load\n    at file:///repo/tests/d.test.ts:2:7\n"), false)
})

test("a kill in a nested test folder is confirmed against that file, not a flattened path", { timeout: 180_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-node-nested-"))
  try {
    const before = "export function gate(n: number): boolean {\n  return n > 0\n}\n"
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests", "unit"), { recursive: true })
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), before)
    writeFileSync(
      path.join(dir, "tests", "unit", "gate.test.ts"),
      'import assert from "node:assert/strict"\nimport test from "node:test"\nimport { gate } from "../../src/gate.ts"\ntest("zero stays shut", () => {\n  assert.equal(gate(0), false)\n})\n',
    )
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.ts", before, before.replace("n > 0", "n >= 0")))
    const body = JSON.parse(
      cli(["mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-build", "--workers", "1"]).stdout,
    ) as RunBody & { flaky?: number; commands?: Array<{ command: string }> }
    assert.equal(body.killed, 1, `${body.summary} ${body.commands?.[0]?.command}`)
    assert.equal(body.flaky ?? 0, 0, body.summary)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
