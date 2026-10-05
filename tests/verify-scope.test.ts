import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("verify-change diffs the package when the rest of the repo diff is huge", { timeout: 60_000 }, () => {
  const repo = mkdtempSync(path.join(tmpdir(), "probatio-verify-scope-"))
  const pkg = path.join(repo, "pkg")
  const out = path.join(repo, "out")
  try {
    mkdirSync(path.join(pkg, "src"), { recursive: true })
    mkdirSync(path.join(pkg, "tests"))
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0 || n < -1000\n}\n")
    writeFileSync(
      path.join(pkg, "tests", "gate.test.ts"),
      [
        "import test from \"node:test\"",
        "import assert from \"node:assert/strict\"",
        "import { gate } from \"../src/gate.ts\"",
        "test(\"zero stays shut\", () => { assert.equal(gate(0), false) })",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(repo, "noise.txt"), `${"a".repeat(80)}\n`.repeat(25000))
    git(repo, ["init", "-q"])
    git(repo, ["add", "."])
    git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0 || n <= -1000\n}\n")
    writeFileSync(path.join(repo, "noise.txt"), `${"b".repeat(80)}\n`.repeat(25000))
    git(repo, ["add", "."])
    git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "edit"])
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "verify-change", "--package", pkg, "--repo", repo, "--out", out, "--base", "HEAD~1", "--commit", "HEAD", "--max-mutants", "4"],
      { cwd: root, encoding: "utf8" },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; caught: Array<{ file: string }>; missed: Array<{ file: string }> }
    assert.equal(body.ok, true, body.summary)
    assert.doesNotMatch(body.summary, /git diff failed/)
    const files = [...body.caught, ...body.missed].map((item) => item.file)
    assert.ok(files.includes("src/gate.ts"), body.summary)
    assert.equal(files.some((file) => file.includes("noise")), false)
  } finally {
    rmSync(repo, { recursive: true, force: true })
  }
})

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
