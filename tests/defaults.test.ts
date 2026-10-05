import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

const source = "export function opens(n: number): boolean {\n  return n > 0\n}\n"

test("omitting --build does not run npm run build:mcp", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-nobuild-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "package.json"), "{ \"type\": \"module\" }\n")
  writeFileSync(path.join(dir, "src", "gate.ts"), source)
  writeFileSync(
    path.join(dir, "tests", "gate.test.ts"),
    [
      "import assert from \"node:assert/strict\"",
      "import test from \"node:test\"",
      "import { opens } from \"../src/gate.ts\"",
      "test(\"zero stays closed\", () => {",
      "  assert.equal(opens(0), false)",
      "})",
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.ts", source, source.replace("n > 0", "n >= 0")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-confirm", "--workers", "1"],
      { cwd: root, encoding: "utf8" },
    )
    const text = `${result.stdout}\n${result.stderr}`
    assert.equal(result.status, 0, text)
    assert.doesNotMatch(text, /build:mcp/)
    assert.doesNotMatch(text, /Missing script/)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
