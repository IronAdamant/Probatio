import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { exportedFunctions } from "../src/golden/record.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

const source = [
  "export function slug(title: string): string {",
  '  return title.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "")',
  "}",
  "export function clamp(n: number, low: number, high: number): number {",
  '  if (low > high) throw new RangeError("low is above high")',
  "  if (n < low) return low",
  "  if (n > high) return high",
  "  return n",
  "}",
  "export function each(items: number[], visit: (n: number) => void): void {",
  "  for (const item of items) visit(item)",
  "}",
  "export async function later(n: number): Promise<number> {",
  "  return n * 2",
  "}",
  "",
].join("\n")

const unitTests = [
  'import assert from "node:assert/strict"',
  'import test from "node:test"',
  'import { clamp, each, later, slug } from "../src/text.ts"',
  'test("slug keeps words", () => { assert.equal(slug("  Hello, World "), "hello-world") })',
  'test("clamp keeps a middle value", () => { assert.equal(clamp(5, 0, 10), 5) })',
  'test("clamp lifts a low value", () => { assert.equal(clamp(-1, 0, 10), 0) })',
  'test("clamp caps a high value", () => { assert.equal(clamp(11, 0, 10), 10) })',
  'test("clamp refuses a reversed range", () => { assert.throws(() => clamp(1, 5, 0), /low is above high/) })',
  'test("each visits every item", () => { const seen: number[] = []; each([1, 2], (n) => seen.push(n)); assert.deepEqual(seen, [1, 2]) })',
  'test("later doubles", async () => { assert.equal(await later(2), 4) })',
  "",
].join("\n")

test("exported functions are read from the source, and classes and defaults stay as code", () => {
  const names = exportedFunctions(
    "src/m.ts",
    "export function a() {}\nexport const b = () => 1, c = 2\nconst d = 1\nexport { d }\nexport type T = string\nexport class K {}\nexport default function z() {}\n",
  )
  assert.deepEqual(names, ["a", "b", "c", "d"])
})

test("golden record turns unit tests into a replayable table, and compare scores it against them", { timeout: 300_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-golden-record-"))
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "text.ts"), source)
    writeFileSync(path.join(dir, "tests", "text.test.ts"), unitTests)
    git(dir, ["init", "-q"])
    commit(dir, "init")

    const recorded = cli(["golden", "record", "--package", dir, "--module", "src/text.ts", "--tests", "tests/text.test.ts"])
    assert.equal(recorded.ok, true, recorded.summary)
    assert.deepEqual(recorded.written, ["tests/golden/text.golden.json", "tests/golden/text.golden.test.ts"])
    // The callback call is a hidden input. It is counted, not turned into a row.
    assert.ok((recorded.hidden as number) >= 1, recorded.summary)
    const table = JSON.parse(readFileSync(path.join(dir, "tests", "golden", "text.golden.json"), "utf8")) as {
      module: string
      rows: Record<string, { fn: string; error?: { name: string }; async?: boolean; result?: { v?: unknown } }>
    }
    assert.equal(table.module, "src/text.ts")
    const rows = Object.values(table.rows)
    assert.ok(rows.some((row) => row.fn === "clamp" && row.error?.name === "RangeError"), JSON.stringify(table.rows))
    assert.ok(rows.some((row) => row.fn === "later" && row.async === true && row.result?.v === 4), JSON.stringify(table.rows))
    assert.ok(rows.some((row) => row.fn === "slug" && row.result?.v === "hello-world"), JSON.stringify(table.rows))
    assert.equal(rows.some((row) => row.fn === "each"), false, "a call with a callback is not a row")
    // The checkout itself was not wrapped. Only the throwaway worktree was.
    assert.equal(readFileSync(path.join(dir, "src", "text.ts"), "utf8"), source)
    assert.equal(existsSync(path.join(dir, "src", "text.probatio-real.ts")), false)

    const replay = replayIn(dir)
    assert.equal(replay.status, 0, replay.stdout + replay.stderr)

    // A wrong recorded answer fails its row. That is what makes the table a contract.
    const broken = structuredClone(table)
    const slugRow = Object.values(broken.rows).find((row) => row.fn === "slug")
    if (slugRow?.result) slugRow.result.v = "hello-there"
    writeFileSync(path.join(dir, "tests", "golden", "text.golden.json"), JSON.stringify(broken))
    const failing = replayIn(dir)
    assert.notEqual(failing.status, 0, "a changed row must fail")
    writeFileSync(path.join(dir, "tests", "golden", "text.golden.json"), `${JSON.stringify(table, null, 2)}\n`)
    commit(dir, "record goldens")

    const argv = (recorded.nextCall as { argv: string[] }).argv
    const outAt = argv.indexOf("--out")
    argv[outAt + 1] = path.join(dir, "compare")
    const compared = cli(argv)
    assert.equal(compared.ok, true, compared.summary)
    assert.ok((compared.unitKills as number) > 0, compared.summary)
    assert.deepEqual(compared.unitOnly, [], compared.summary)
    assert.equal(compared.both, compared.unitKills, compared.summary)
    assert.deepEqual(compared.pruning, { mode: "advisory", deletedTests: 0 })
    assert.match(compared.next, /Nothing was deleted/)
    assert.equal(existsSync(path.join(dir, "tests", "text.test.ts")), true, "the unit tests are still there")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

type Body = { ok: boolean; summary: string; next: string; nextCall: unknown; [key: string]: unknown }

function cli(args: string[]): Body {
  const result = spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", timeout: 280_000 })
  assert.equal(result.stdout.trim().startsWith("{"), true, result.stderr + result.stdout)
  return JSON.parse(result.stdout) as Body
}

/** A node --test child of node:test reports to its parent and exits 0. Drop that context so the exit code is the verdict. */
function replayIn(dir: string) {
  const env = { ...process.env }
  for (const key of Object.keys(env)) if (key.startsWith("NODE_TEST_")) delete env[key]
  return spawnSync(process.execPath, ["--experimental-strip-types", "--test", "tests/golden/text.golden.test.ts"], { cwd: dir, encoding: "utf8", env })
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}

function commit(repo: string, message: string) {
  git(repo, ["add", "."])
  git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", message])
}
