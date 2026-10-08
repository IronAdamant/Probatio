import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { missingTool } from "./require-tool.ts"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")
const dotnetMajor = dotnetTfm()

const source = ["int n = 0;", "if (n > 0) return 1;", "return 0;", ""].join("\n")

test("dotnet test builds a sibling exe the suite launches by path", { timeout: 180_000, skip: missingTool("dotnet") }, () => {
  const repo = mkdtempSync(path.join(tmpdir(), "probatio-dotnet-sibling-"))
  const pkg = path.join(repo, "CLI")
  mkdirSync(path.join(pkg, "src"), { recursive: true })
  mkdirSync(path.join(pkg, "test"), { recursive: true })
  mkdirSync(path.join(repo, "patches"), { recursive: true })
  writeFileSync(
    path.join(pkg, "src", "App.csproj"),
    [
      "<Project Sdk=\"Microsoft.NET.Sdk\">",
      "  <PropertyGroup>",
      "    <OutputType>Exe</OutputType>",
      `    <TargetFramework>net${dotnetMajor}.0</TargetFramework>`,
      "    <AppendTargetFrameworkToOutputPath>false</AppendTargetFrameworkToOutputPath>",
      "    <AssemblyName>App</AssemblyName>",
      "    <ImplicitUsings>disable</ImplicitUsings>",
      "    <Nullable>disable</Nullable>",
      "  </PropertyGroup>",
      "</Project>",
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(pkg, "src", "Program.cs"), source)
  writeFileSync(
    path.join(pkg, "test", "App.Test.csproj"),
    [
      "<Project Sdk=\"Microsoft.NET.Sdk\">",
      "  <PropertyGroup>",
      `    <TargetFramework>net${dotnetMajor}.0</TargetFramework>`,
      "    <AppendTargetFrameworkToOutputPath>false</AppendTargetFrameworkToOutputPath>",
      "    <ImplicitUsings>disable</ImplicitUsings>",
      "    <Nullable>disable</Nullable>",
      "    <IsPackable>false</IsPackable>",
      "  </PropertyGroup>",
      "  <ItemGroup>",
      "    <PackageReference Include=\"Microsoft.NET.Test.Sdk\" Version=\"17.3.2\" />",
      "    <PackageReference Include=\"MSTest.TestAdapter\" Version=\"2.2.10\" />",
      "    <PackageReference Include=\"MSTest.TestFramework\" Version=\"2.2.10\" />",
      "  </ItemGroup>",
      "</Project>",
      "",
    ].join("\n"),
  )
  writeFileSync(
    path.join(pkg, "test", "GateTest.cs"),
    [
      "using System;",
      "using System.Diagnostics;",
      "using System.IO;",
      "using Microsoft.VisualStudio.TestTools.UnitTesting;",
      "",
      "namespace Gate",
      "{",
      "    [TestClass]",
      "    public class GateTest",
      "    {",
      "        [TestMethod]",
      "        public void StaysClosed()",
      "        {",
      "            string currentDirectory = Environment.CurrentDirectory;",
      "            string configuration = Path.GetFileName(currentDirectory);",
      "            string root = Path.GetFullPath(Path.Combine(currentDirectory, \"..\", \"..\", \"..\", \"..\"));",
      "            string dll = Path.Combine(root, \"CLI\", \"src\", \"bin\", configuration, \"App.dll\");",
      "            var start = new ProcessStartInfo(\"dotnet\", dll);",
      "            start.UseShellExecute = false;",
      "            var process = Process.Start(start);",
      "            process.WaitForExit();",
      "            if (process.ExitCode != 0) throw new Exception(\"open\");",
      "        }",
      "    }",
      "}",
      "",
    ].join("\n"),
  )
  writeFileSync(
    path.join(repo, "patches", "m-exe.patch"),
    forwardDiff("CLI/src/Program.cs", source, source.replace("n > 0", "n >= 0")),
  )
  git(repo, ["init", "-q"])
  git(repo, ["add", "."])
  git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      [
        "src/cli.ts",
        "mutate",
        "run",
        "--package",
        pkg,
        "--repo",
        repo,
        "--patches",
        path.join(repo, "patches"),
        "--out",
        path.join(repo, "out"),
        "--no-build",
        "--no-confirm",
        "--workers",
        "1",
        "--suite-timeout-ms",
        "120000",
      ],
      { cwd: root, encoding: "utf8", timeout: 300_000 },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as {
      ok: boolean
      summary: string
      killed: number
      survived: number
      kills?: Array<{ id: string; cause: string; killedBy: string[]; next: string }>
    }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.survived, 0, body.summary)
    assert.doesNotMatch(body.summary, /cobc|no tests/)
    const kill = (body.kills ?? []).find((item) => item.id === "m-exe")
    assert.ok(kill, JSON.stringify(body.kills))
    assert.equal(kill.cause, "test")
    assert.match(kill.next, /StaysClosed/)
    assert.doesNotMatch(kill.next, /add a test named dotnet/i)
    const saved = JSON.parse(readFileSync(path.join(repo, "out", "results", "m-exe.json"), "utf8")) as {
      outcome: string
      cause: string
      next: string
    }
    assert.equal(saved.outcome, "killed")
    assert.equal(saved.cause, "test")
    assert.equal(saved.next, kill.next)
  } finally {
    rmSync(repo, { recursive: true, force: true })
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
