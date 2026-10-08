import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("a plain-script child dump is scored under the parent test", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-child-"))
  const childLines = pathToFileURL(path.join(root, "src", "mutate", "child-lines.mjs")).href
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(path.join(dir, "src", "seen.ts"), "export function seen(n: number): boolean {\n  return n > 0 || n < -1000\n}\n")
    writeFileSync(path.join(dir, "src", "dark.ts"), "export function dark(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(
      path.join(dir, "child-run.ts"),
      `import ${JSON.stringify(childLines)}\nimport { gate } from "./src/gate.ts"\nif (!gate(1)) process.exit(1)\n`,
    )
    writeFileSync(
      path.join(dir, "tests", "parent.test.ts"),
      [
        "import { spawnSync } from \"node:child_process\"",
        "import test from \"node:test\"",
        "import assert from \"node:assert/strict\"",
        "import { seen } from \"../src/seen.ts\"",
        "test(\"parent passes\", () => {",
        "  assert.equal(seen(1), true)",
        "  const child = spawnSync(process.execPath, [\"--experimental-strip-types\", \"child-run.ts\"], { cwd: process.cwd() })",
        "  assert.equal(child.status, 0, child.stderr?.toString())",
        "})",
        "",
      ].join("\n"),
    )
    commit(dir)
    const generated = spawnSync(tsx, ["src/cli.ts", "mutate", "generate", "--package", dir, "--src", "src", "--out", path.join(dir, "gen")], { cwd: root, encoding: "utf8", timeout: 300_000 })
    assert.equal(generated.status, 0, generated.stderr + generated.stdout)
    const ran = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--patches", path.join(dir, "gen", "mutants"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm"],
      { cwd: root, encoding: "utf8", timeout: 300_000 },
    )
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string; next: string; noCoverage: number; survived: number }
    assert.equal(body.ok, true)
    assert.ok(body.noCoverage >= 1, body.summary)
    assert.ok(body.survived >= 1, body.summary)
    assert.match(body.summary, new RegExp(`^${body.noCoverage} no coverage, ${body.survived} survived`))
    assert.ok(body.next.indexOf("No coverage") < body.next.indexOf("A survivor is not a pass"), body.next)
    const results = path.join(dir, "out", "results")
    const saved = readdirSync(results)
      .filter((name) => name.endsWith(".json"))
      .map((name) => JSON.parse(readFileSync(path.join(results, name), "utf8")) as { outcome: string; command: string; selectedTests?: string[]; files?: Array<{ file: string }> })
    const gate = saved.filter((item) => item.files?.some((file) => file.file.endsWith("gate.ts")))
    assert.ok(gate.length > 0, ran.stdout)
    assert.ok(gate.every((item) => item.outcome !== "no coverage" && item.command !== "" && item.selectedTests?.includes("parent passes")), JSON.stringify(gate))
    const dark = saved.filter((item) => item.files?.some((file) => file.file.endsWith("dark.ts")))
    assert.ok(dark.length > 0 && dark.every((item) => item.outcome === "no coverage" && item.command === ""), JSON.stringify(dark))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a plain node child is mapped under the parent test, and a child with no env stays no coverage", { timeout: 120_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-child-quiet-"))
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(path.join(dir, "src", "seen.ts"), "export function seen(n: number): boolean {\n  return n > 0 || n < -1000\n}\n")
    writeFileSync(path.join(dir, "src", "dark.ts"), "export function dark(n: number): boolean {\n  return n > 0\n}\n")
    // No Probatio import, and the child ends with process.exit, the way most CLIs do.
    writeFileSync(path.join(dir, "child-run.ts"), "import { gate } from \"./src/gate.ts\"\nprocess.exit(gate(1) ? 0 : 1)\n")
    // An empty env drops what the collector needs. That child cannot be seen.
    writeFileSync(path.join(dir, "child-bare.ts"), "import { dark } from \"./src/dark.ts\"\nprocess.exit(dark(1) ? 0 : 1)\n")
    writeFileSync(
      path.join(dir, "tests", "parent.test.ts"),
      [
        "import { spawnSync } from \"node:child_process\"",
        "import test from \"node:test\"",
        "import assert from \"node:assert/strict\"",
        "import { seen } from \"../src/seen.ts\"",
        "test(\"parent passes\", () => {",
        "  assert.equal(seen(1), true)",
        "  const child = spawnSync(process.execPath, [\"--experimental-strip-types\", \"child-run.ts\"], { cwd: process.cwd() })",
        "  assert.equal(child.status, 0, child.stderr?.toString())",
        "  const bare = spawnSync(process.execPath, [\"--experimental-strip-types\", \"child-bare.ts\"], { cwd: process.cwd(), env: {} })",
        "  assert.equal(bare.status, 0, bare.stderr?.toString())",
        "})",
        "",
      ].join("\n"),
    )
    commit(dir)
    const generated = spawnSync(tsx, ["src/cli.ts", "mutate", "generate", "--package", dir, "--src", "src", "--out", path.join(dir, "gen")], { cwd: root, encoding: "utf8", timeout: 300_000 })
    assert.equal(generated.status, 0, generated.stderr + generated.stdout)
    const ran = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--patches", path.join(dir, "gen", "mutants"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm"],
      { cwd: root, encoding: "utf8", timeout: 300_000 },
    )
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const results = path.join(dir, "out", "results")
    const saved = readdirSync(results)
      .filter((name) => name.endsWith(".json"))
      .map((name) => JSON.parse(readFileSync(path.join(results, name), "utf8")) as { outcome: string; command: string; selectedTests?: string[]; files?: Array<{ file: string }> })
    const gate = saved.filter((item) => item.files?.some((file) => file.file.endsWith("gate.ts")))
    assert.ok(gate.length > 0, ran.stdout)
    assert.ok(gate.every((item) => item.outcome !== "no coverage" && item.selectedTests?.includes("parent passes")), JSON.stringify(gate))
    const dark = saved.filter((item) => item.files?.some((file) => file.file.endsWith("dark.ts")))
    assert.ok(dark.length > 0 && dark.every((item) => item.outcome === "no coverage" && item.command === ""), JSON.stringify(dark))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function commit(repo: string): void {
  const git = (args: string[]) => {
    const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
    if (result.status !== 0) throw new Error(result.stderr || result.stdout)
  }
  git(["init", "-q"])
  git(["add", "."])
  git(["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
}
