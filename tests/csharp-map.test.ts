import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { missingTool } from "./require-tool.ts"
import { fileURLToPath } from "node:url"
import { changedLines, forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")
const dotnetMajor = dotnetTfm()

const source = [
  "public static class Gate",
  "{",
  "    public static bool Open(int n)",
  "    {",
  "        return n > 0;",
  "    }",
  "",
  "    public static int Idle()",
  "    {",
  "        return 1;",
  "    }",
  "}",
  "",
].join("\n")

const tests = [
  "using Xunit;",
  "",
  "public class GateTests",
  "{",
  "    [Fact]",
  "    public void StaysClosed()",
  "    {",
  "        Assert.False(Gate.Open(0));",
  "    }",
  "",
  "    [Fact]",
  "    public void StaysOpen()",
  "    {",
  "        Assert.True(Gate.Open(1));",
  "    }",
  "",
  "    [Fact]",
  "    public void Other()",
  "    {",
  "        Assert.Equal(4, 2 + 2);",
  "    }",
  "}",
  "",
].join("\n")

test("a csharp line map names the killing test and reports an unexecuted line as no coverage", { timeout: 420_000, skip: missingTool("dotnet") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-csharp-map-"))
  try {
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(
      path.join(dir, "tests", "Gate.csproj"),
      [
        "<Project Sdk=\"Microsoft.NET.Sdk\">",
        "  <PropertyGroup>",
        `    <TargetFramework>net${dotnetMajor}.0</TargetFramework>`,
        "    <ImplicitUsings>disable</ImplicitUsings>",
        "    <Nullable>disable</Nullable>",
        "    <IsPackable>false</IsPackable>",
        "  </PropertyGroup>",
        "  <ItemGroup>",
        "    <PackageReference Include=\"Microsoft.NET.Test.Sdk\" Version=\"17.12.0\" />",
        "    <PackageReference Include=\"xunit\" Version=\"2.9.2\" />",
        "    <PackageReference Include=\"xunit.runner.visualstudio\" Version=\"2.8.2\" />",
        "    <PackageReference Include=\"coverlet.collector\" Version=\"6.0.2\">",
        "      <PrivateAssets>all</PrivateAssets>",
        "      <IncludeAssets>runtime; build; native; contentfiles; analyzers; buildtransitive</IncludeAssets>",
        "    </PackageReference>",
        "  </ItemGroup>",
        "</Project>",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(dir, "tests", "Gate.cs"), source)
    writeFileSync(path.join(dir, "tests", "GateTests.cs"), tests)
    writeFileSync(path.join(dir, "patches", "m-open.patch"), forwardDiff("tests/Gate.cs", source, source.replace("        return n > 0;\n", "        return n >= 0;\n")))
    writeFileSync(path.join(dir, "patches", "m-idle.patch"), forwardDiff("tests/Gate.cs", source, source.replace("        return 1;\n", "        return 2;\n")))
    commit(dir)
    const ran = cli([
      "mutate",
      "run",
      "--package",
      dir,
      "--repo",
      dir,
      "--patches",
      path.join(dir, "patches"),
      "--out",
      path.join(dir, "out"),
      "--no-build",
      "--no-confirm",
      "--workers",
      "1",
      "--suite-timeout-ms",
      "180000",
    ])
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string; kills?: Array<{ id: string; killedBy?: string[] }> }
    assert.equal(body.ok, true, `${body.summary}\n${ran.stderr}`)
    const results = readResults(path.join(dir, "out", "results"))
    const open = results.find((item) => item.id === "m-open")
    const idle = results.find((item) => item.id === "m-idle")
    assert.ok(open, JSON.stringify(results, null, 2))
    assert.equal(open.outcome, "killed", `${open.outcome} ${open.command}`)
    assert.ok(open.killedBy.some((name) => name.includes("StaysClosed")), JSON.stringify(open.killedBy))
    assert.match(open.command, /StaysClosed/)
    assert.equal(/Other/.test(open.command), false, open.command)
    assert.ok(idle, JSON.stringify(results, null, 2))
    assert.equal(idle.outcome, "no coverage", `${idle.outcome} ${idle.command}`)
    assert.equal(idle.command, "")
    const map = JSON.parse(readFileSync(path.join(dir, "out", "coverage-map.json"), "utf8")) as { files: Record<string, Record<string, string[]>> }
    const file = Object.keys(map.files).find((name) => name.endsWith("Gate.cs"))
    assert.ok(file, JSON.stringify(Object.keys(map.files)))
    const lines = map.files[file]
    const openLine = String(changedLines(readFileSync(path.join(dir, "patches", "m-open.patch"), "utf8"))[0]?.line)
    const idleLine = String(changedLines(readFileSync(path.join(dir, "patches", "m-idle.patch"), "utf8"))[0]?.line)
    assert.ok((lines[openLine] ?? []).some((name) => name.includes("StaysClosed")), JSON.stringify(lines))
    assert.equal(lines[idleLine], undefined, JSON.stringify(lines))
    const named = (body.kills ?? []).find((item) => item.id === "m-open")
    assert.ok(named?.killedBy?.some((name) => name.includes("StaysClosed")), JSON.stringify(body.kills))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function readResults(dir: string): Array<{ id: string; outcome: string; command: string; killedBy: string[]; files: Array<{ file: string; line: number }> }> {
  return readdirSync(dir)
    .filter((name) => name.endsWith(".json"))
    .map((name) => JSON.parse(readFileSync(path.join(dir, name), "utf8")) as { id: string; outcome: string; command: string; killedBy: string[]; files: Array<{ file: string; line: number }> })
}

function cli(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", timeout: 400_000 })
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

function dotnetTfm(): string {
  const result = spawnSync("dotnet", ["--version"], { encoding: "utf8" })
  const major = result.status === 0 ? result.stdout.trim().split(".")[0] : "8"
  return major || "8"
}
