import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { missingPytest, missingTool } from "./require-tool.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

const cases: Array<{ name: string; src: string; skip: string | false; timeout: number }> = [
  { name: "node-ts", src: "src", skip: false, timeout: 90_000 },
  { name: "pytest", src: "src", skip: missingPytest(), timeout: 120_000 },
  { name: "go", src: ".", skip: missingTool("go"), timeout: 120_000 },
  { name: "maven", src: "src", skip: missingTool("mvn"), timeout: 300_000 },
  { name: "rust", src: "src", skip: missingTool("cargo"), timeout: 300_000 },
]

for (const item of cases) {
  test(`example ${item.name} kills a mutant`, { timeout: item.timeout, skip: item.skip }, () => {
    const dir = mkdtempSync(path.join(tmpdir(), `probatio-example-${item.name}-`))
    try {
      cpSync(path.join(root, "examples", item.name), dir, { recursive: true })
      git(dir, ["init", "-q"])
      git(dir, ["add", "."])
      git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
      const generated = cli(dir, ["mutate", "generate", "--package", dir, "--src", item.src, "--out", path.join(dir, "gen"), "--max-mutants", "4", "--max-minutes", "1"])
      assert.equal(generated.status, 0, generated.stderr + generated.stdout)
      const ran = cli(dir, [
        "mutate", "run",
        "--package", dir,
        "--repo", dir,
        "--patches", path.join(dir, "gen", "mutants"),
        "--out", path.join(dir, "out"),
        "--no-build",
        "--no-confirm",
        "--workers", "1",
        "--max-mutants", "4",
        "--max-minutes", "2",
      ])
      assert.equal(ran.status, 0, ran.stderr + ran.stdout)
      const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string; killed: number }
      const expected = JSON.parse(readFileSync(path.join(dir, "expected.json"), "utf8")) as { minKilled: number }
      assert.equal(body.ok, true, body.summary)
      assert.ok(body.killed >= expected.minKilled, body.summary)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
}

function cli(dir: string, args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8" })
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
