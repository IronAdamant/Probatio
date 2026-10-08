import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { missingTool } from "./require-tool.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")
const dotnetMajor = dotnetTfm()

const source = ["int n = 0;", "if (n > 0) return 1;", "return 0;", ""].join("\n")

test("dotnet discovers a suite under test/ and scores it", { timeout: 120_000, skip: missingTool("dotnet") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-dotnet-test-"))
  mkdirSync(path.join(dir, "test"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(
    path.join(dir, "test", "Gate.csproj"),
    [
      "<Project Sdk=\"Microsoft.NET.Sdk\">",
      "  <PropertyGroup>",
      "    <OutputType>Exe</OutputType>",
      `    <TargetFramework>net${dotnetMajor}.0</TargetFramework>`,
      "    <ImplicitUsings>disable</ImplicitUsings>",
      "    <Nullable>disable</Nullable>",
      "  </PropertyGroup>",
      "</Project>",
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "test", "Program.cs"), source)
  writeFileSync(path.join(dir, "patches", "m-test.patch"), forwardDiff("test/Program.cs", source, source.replace("n > 0", "n >= 0")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm", "--workers", "1", "--suite-timeout-ms", "90000"],
      { cwd: root, encoding: "utf8", timeout: 300_000 },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; survived: number }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.survived, 0, body.summary)
    assert.doesNotMatch(body.summary, /cobc|no tests/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function dotnetTfm(): string {
  const result = spawnSync("dotnet", ["--version"], { encoding: "utf8" })
  const major = Number((result.stdout || "8").split(".")[0])
  return String(Number.isFinite(major) && major > 0 ? major : 8)
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
