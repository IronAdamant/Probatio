import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("a line that runs only in a child process stays no coverage and leads the summary", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-child-"))
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(dir, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(path.join(dir, "src", "seen.ts"), "export function seen(n: number): boolean {\n  return n > 0 || n < -1000\n}\n")
    writeFileSync(
      path.join(dir, "child-run.ts"),
      `import { gate } from "./src/gate.ts"\nif (!gate(1)) process.exit(1)\n`,
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
        `  const child = spawnSync(process.execPath, ["--experimental-strip-types", "child-run.ts"], { cwd: ${JSON.stringify(dir)} })`,
        "  assert.equal(child.status, 0, child.stderr)",
        "})",
        "",
      ].join("\n"),
    )
    commit(dir)
    const generated = spawnSync(tsx, ["src/cli.ts", "mutate", "generate", "--package", dir, "--src", "src", "--out", path.join(dir, "gen")], { cwd: root, encoding: "utf8" })
    assert.equal(generated.status, 0, generated.stderr + generated.stdout)
    const ran = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--patches", path.join(dir, "gen", "mutants"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm"],
      { cwd: root, encoding: "utf8" },
    )
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string; next: string; noCoverage: number; survived: number; commands: Array<{ outcome: string; command: string }> }
    assert.equal(body.ok, true)
    assert.ok(body.noCoverage >= 1, body.summary)
    assert.ok(body.survived >= 1, body.summary)
    assert.match(body.summary, new RegExp(`^${body.noCoverage} no coverage, ${body.survived} survived`))
    assert.ok(body.next.indexOf("No coverage") < body.next.indexOf("A survivor is not a pass"), body.next)
    const results = path.join(dir, "out", "results")
    const dark = readdirSync(results)
      .filter((name) => name.endsWith(".json"))
      .map((name) => JSON.parse(readFileSync(path.join(results, name), "utf8")) as { outcome: string; command: string; files?: Array<{ file: string }> })
      .filter((item) => item.files?.some((file) => file.file.endsWith("gate.ts")) && item.outcome === "no coverage")
    assert.ok(dark.length > 0, ran.stdout)
    assert.ok(dark.every((item) => item.command === ""), JSON.stringify(dark))
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
