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

const source = ["export function open(n) {", "  return n > 0", "}", ""].join("\n")

test("mocha rebuilds a stale package entry so a source mutant is visible", { timeout: 180_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-mocha-"))
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "test"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "src", "gate.js"), source)
  writeFileSync(
    path.join(dir, "build.js"),
    ["import { mkdirSync, readFileSync, writeFileSync } from \"node:fs\"", "mkdirSync(\"dist\", { recursive: true })", "writeFileSync(\"dist/gate.js\", readFileSync(\"src/gate.js\", \"utf8\"))", ""].join("\n"),
  )
  writeFileSync(
    path.join(dir, "package.json"),
    `${JSON.stringify({
      type: "module",
      main: "./dist/gate.js",
      exports: { ".": "./dist/gate.js" },
      scripts: { bundle: "node build.js" },
      mocha: { spec: ["test/gate.test.js"] },
    }, null, 2)}\n`,
  )
  writeFileSync(
    path.join(dir, "test", "gate.test.js"),
    ["import assert from \"node:assert/strict\"", "import { open } from \"../dist/gate.js\"", "describe(\"gate\", function () {", "  it(\"stays closed at zero\", function () {", "    assert.equal(open(0), false)", "  })", "})", ""].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-test.patch"), forwardDiff("src/gate.js", source, source.replace("n > 0", "n >= 0")))
  writeFileSync(path.join(dir, ".gitignore"), "node_modules\ndist\n")
  const installed = spawnSync("npm", ["install", "--silent", "--no-fund", "--no-audit", "mocha@10"], { cwd: dir, encoding: "utf8", timeout: 120_000 })
  assert.equal(installed.status, 0, installed.stderr || installed.stdout)
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm", "--workers", "1", "--suite-timeout-ms", "60000"],
      { cwd: root, encoding: "utf8", timeout: 300_000 },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; survived: number; kills?: Array<{ id: string; cause: string; next: string }> }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.survived, 0, body.summary)
    const kill = (body.kills ?? []).find((item) => item.id === "m-test")
    assert.ok(kill, JSON.stringify(body.kills))
    assert.equal(kill.cause, "test")
    assert.match(kill.next, /stays closed at zero/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
