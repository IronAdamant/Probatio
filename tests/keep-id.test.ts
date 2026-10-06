import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { missingPytest, missingTool } from "./require-tool.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

type RunBody = {
  ok: boolean
  summary: string
  killed?: number
  kills?: Array<{ killedBy?: string[] }>
  commands?: Array<{ outcome: string; command: string }>
}
type TallyBody = {
  ok: boolean
  keep: string[]
  nextCall: { argv: string[] } | null
  pruning: { deletedTests: number }
}

test("pytest keep id is rerun through --only-test and a doubled id is not a kill", { timeout: 90_000, skip: missingPytest() }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-keep-py-"))
  try {
    const before = "def gate(n):\n    return n > 0\n"
    const after = "def gate(n):\n    return n >= 0\n"
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "src", "gate.py"), before)
    writeFileSync(path.join(dir, "tests", "test_gate.py"), "import pytest\nfrom gate import gate\n\ndef test_low():\n    assert gate(0) is False\n")
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.py", before, after))
    const first = run(dir, path.join(dir, "out"))
    assert.equal(first.ok, true, first.summary)
    const nodeId = "tests/test_gate.py::test_low"
    assert.deepEqual(first.kills?.[0]?.killedBy, [nodeId], JSON.stringify(first.kills))
    const tally = tallyOf(path.join(dir, "out"))
    assert.equal(tally.pruning.deletedTests, 0)
    assert.deepEqual(tally.keep, [nodeId], JSON.stringify(tally.keep))
    assert.ok(tally.nextCall?.argv.includes("--only-test"), JSON.stringify(tally.nextCall))
    assert.ok(tally.nextCall?.argv.includes(nodeId), JSON.stringify(tally.nextCall))
    const doubled = `test_gate.py::${nodeId}`
    const bad = run(dir, path.join(dir, "bad"), ["--only-test", doubled])
    assert.equal(bad.killed ?? 0, 0, bad.summary)
    const again = run(dir, path.join(dir, "again"), ["--only-test", nodeId])
    assert.equal(again.ok, true, again.summary)
    assert.equal(again.killed, 1, again.summary)
    assert.match(again.commands?.[0]?.command ?? "", /test_low/)
    assert.equal((again.commands?.[0]?.command ?? "").includes(doubled), false, again.commands?.[0]?.command)
    const refused = cli(["mutate", "run", "--package", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "mix"), "--only-test", nodeId, "--suite-command", "python3 -m pytest", "--no-confirm", "--no-build"])
    const mix = JSON.parse(refused.stdout) as RunBody
    assert.equal(mix.ok, false, refused.stdout)
    assert.match(mix.summary, /only-test/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("go keep id is the -run name, not a package prefix", { timeout: 120_000, skip: missingTool("go") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-keep-go-"))
  try {
    const before = "package gate\n\nfunc Gate(n int) bool {\n\treturn n > 0\n}\n"
    const after = "package gate\n\nfunc Gate(n int) bool {\n\treturn n >= 0\n}\n"
    writeFileSync(path.join(dir, "go.mod"), "module example.com/gate\n\ngo 1.22\n")
    writeFileSync(path.join(dir, "gate.go"), before)
    writeFileSync(path.join(dir, "gate_test.go"), "package gate\n\nimport \"testing\"\n\nfunc TestShut(t *testing.T) {\n\tif Gate(0) {\n\t\tt.Fatal(\"shut\")\n\t}\n}\nfunc TestOther(t *testing.T) {\n\tif 1 != 1 {\n\t\tt.Fatal(\"other\")\n\t}\n}\n")
    mkdirSync(path.join(dir, "patches"))
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("gate.go", before, after))
    const first = run(dir, path.join(dir, "out"))
    assert.equal(first.ok, true, first.summary)
    const id = "TestShut"
    assert.deepEqual(first.kills?.[0]?.killedBy, [id], JSON.stringify(first.kills))
    const tally = tallyOf(path.join(dir, "out"))
    assert.equal(tally.pruning.deletedTests, 0)
    assert.deepEqual(tally.keep, [id], JSON.stringify(tally.keep))
    const again = run(dir, path.join(dir, "again"), ["--only-test", id])
    assert.equal(again.killed, 1, again.summary)
    assert.match(again.commands?.[0]?.command ?? "", /-run/)
    assert.match(again.commands?.[0]?.command ?? "", /TestShut/)
    assert.equal((again.commands?.[0]?.command ?? "").includes("::"), false, again.commands?.[0]?.command)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("cargo keep id is the exact test name, not an empty :: prefix", { timeout: 180_000, skip: missingTool("cargo") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-keep-rs-"))
  try {
    const source = "pub fn open(n: i32) -> bool {\n    n > 0\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn stays_closed() {\n        assert!(!open(0));\n    }\n    #[test]\n    fn other() {\n        assert_eq!(2 + 2, 4);\n    }\n}\n"
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "Cargo.toml"), "[package]\nname = \"gate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
    writeFileSync(path.join(dir, "src", "lib.rs"), source)
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/lib.rs", source, source.replace("    n > 0\n", "    n >= 0\n")))
    const first = run(dir, path.join(dir, "out"), ["--suite-timeout-ms", "120000"])
    assert.equal(first.ok, true, first.summary)
    const killedBy = first.kills?.[0]?.killedBy ?? []
    assert.equal(killedBy.some((name) => name.startsWith("::")), false, JSON.stringify(killedBy))
    assert.ok(killedBy.some((name) => name.includes("stays_closed")), JSON.stringify(killedBy))
    const tally = tallyOf(path.join(dir, "out"))
    assert.equal(tally.pruning.deletedTests, 0)
    assert.equal(tally.keep.some((name) => name.startsWith("::")), false, JSON.stringify(tally.keep))
    const id = tally.keep.find((name) => name.includes("stays_closed"))
    assert.ok(id, JSON.stringify(tally.keep))
    const again = run(dir, path.join(dir, "again"), ["--only-test", id as string, "--suite-timeout-ms", "120000"])
    assert.equal(again.killed, 1, again.summary)
    assert.match(again.commands?.[0]?.command ?? "", /stays_closed/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("dotnet keep id is the fact name, not an empty :: prefix", { timeout: 240_000, skip: missingTool("dotnet") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-keep-cs-"))
  const major = dotnetTfm()
  try {
    const source = "public static class Gate {\n    public static bool Open(int n) { return n > 0; }\n}\n"
    const flipped = source.replace("n > 0", "n >= 0")
    mkdirSync(path.join(dir, "tests"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "tests", "Gate.csproj"), `<Project Sdk="Microsoft.NET.Sdk">\n  <PropertyGroup>\n    <TargetFramework>net${major}.0</TargetFramework>\n    <ImplicitUsings>disable</ImplicitUsings>\n    <Nullable>disable</Nullable>\n    <IsPackable>false</IsPackable>\n  </PropertyGroup>\n  <ItemGroup>\n    <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.12.0" />\n    <PackageReference Include="xunit" Version="2.9.2" />\n    <PackageReference Include="xunit.runner.visualstudio" Version="2.8.2" />\n    <PackageReference Include="coverlet.collector" Version="6.0.2" />\n  </ItemGroup>\n</Project>\n`)
    writeFileSync(path.join(dir, "tests", "Gate.cs"), source)
    writeFileSync(path.join(dir, "tests", "GateTests.cs"), "using Xunit;\npublic class GateTests {\n    [Fact]\n    public void StaysClosed() { Assert.False(Gate.Open(0)); }\n    [Fact]\n    public void Other() { Assert.Equal(4, 2 + 2); }\n}\n")
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("tests/Gate.cs", source, flipped))
    const first = run(dir, path.join(dir, "out"), ["--suite-timeout-ms", "180000"])
    assert.equal(first.ok, true, first.summary)
    const killedBy = first.kills?.[0]?.killedBy ?? []
    assert.equal(killedBy.some((name) => name.startsWith("::")), false, JSON.stringify(killedBy))
    assert.ok(killedBy.some((name) => name.includes("StaysClosed")), JSON.stringify(killedBy))
    const tally = tallyOf(path.join(dir, "out"))
    assert.equal(tally.pruning.deletedTests, 0)
    const id = tally.keep.find((name) => name.includes("StaysClosed"))
    assert.ok(id, JSON.stringify(tally.keep))
    assert.equal(tally.keep.filter((name) => name.includes("StaysClosed")).length, 1, JSON.stringify(tally.keep))
    const again = run(dir, path.join(dir, "again"), ["--only-test", id as string, "--suite-timeout-ms", "180000"])
    assert.equal(again.killed, 1, again.summary)
    assert.match(again.commands?.[0]?.command ?? "", /StaysClosed/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("junit keep id is the class.method selector, not a doubled name", { timeout: 300_000, skip: missingTool("mvn") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-keep-java-"))
  try {
    const source = "package gate;\npublic class Gate {\n  public static boolean allow(int n) {\n    return n > 0;\n  }\n}\n"
    const flipped = source.replace("n > 0", "n >= 0")
    mkdirSync(path.join(dir, "src", "main", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "src", "test", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "pom.xml"), "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>example</groupId>\n  <artifactId>gate</artifactId>\n  <version>1.0</version>\n  <dependencies>\n    <dependency>\n      <groupId>junit</groupId>\n      <artifactId>junit</artifactId>\n      <version>4.13.2</version>\n      <scope>test</scope>\n    </dependency>\n  </dependencies>\n</project>\n")
    writeFileSync(path.join(dir, "src", "main", "java", "gate", "Gate.java"), source)
    writeFileSync(path.join(dir, "src", "test", "java", "gate", "GateTest.java"), "package gate;\nimport org.junit.Test;\nimport static org.junit.Assert.*;\npublic class GateTest {\n  @Test public void testShut() { assertFalse(Gate.allow(0)); }\n  @Test public void testOther() { assertEquals(1, 1); }\n}\n")
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/main/java/gate/Gate.java", source, flipped))
    const first = run(dir, path.join(dir, "out"), ["--suite-timeout-ms", "180000"])
    assert.equal(first.ok, true, first.summary)
    const killedBy = first.kills?.[0]?.killedBy ?? []
    assert.equal(killedBy.some((name) => name.includes("::")), false, JSON.stringify(killedBy))
    assert.ok(killedBy.some((name) => name.includes("testShut")), JSON.stringify(killedBy))
    const tally = tallyOf(path.join(dir, "out"))
    assert.equal(tally.pruning.deletedTests, 0)
    const id = tally.keep.find((name) => name.includes("testShut"))
    assert.ok(id, JSON.stringify(tally.keep))
    assert.equal(id?.includes("::"), false, id)
    const again = run(dir, path.join(dir, "again"), ["--only-test", id as string, "--suite-timeout-ms", "180000"])
    assert.equal(again.killed, 1, again.summary)
    assert.match(again.commands?.[0]?.command ?? "", /testShut/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("junit parametrized keep id keeps the invocation Surefire recorded", { timeout: 300_000, skip: missingTool("mvn") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-keep-junit-param-"))
  try {
    const source = "package gate;\npublic class Gate {\n  public static boolean allow(int n) {\n    return n > 0;\n  }\n}\n"
    const flipped = source.replace("n > 0", "n >= 0")
    mkdirSync(path.join(dir, "src", "main", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "src", "test", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "pom.xml"), "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>example</groupId>\n  <artifactId>gate</artifactId>\n  <version>1.0</version>\n  <dependencies>\n    <dependency>\n      <groupId>junit</groupId>\n      <artifactId>junit</artifactId>\n      <version>4.13.2</version>\n      <scope>test</scope>\n    </dependency>\n  </dependencies>\n</project>\n")
    writeFileSync(path.join(dir, "src", "main", "java", "gate", "Gate.java"), source)
    writeFileSync(
      path.join(dir, "src", "test", "java", "gate", "GateTest.java"),
      [
        "package gate;",
        "import org.junit.Test;",
        "import org.junit.runner.RunWith;",
        "import org.junit.runners.Parameterized;",
        "import java.util.Arrays;",
        "import java.util.Collection;",
        "import static org.junit.Assert.*;",
        "@RunWith(Parameterized.class)",
        "public class GateTest {",
        "  private final int n;",
        "  public GateTest(int n) { this.n = n; }",
        "  @Parameterized.Parameters",
        "  public static Collection<Object[]> data() {",
        "    return Arrays.asList(new Object[][] { {0}, {1} });",
        "  }",
        "  @Test public void testShut() {",
        "    if (n == 0) assertFalse(Gate.allow(n));",
        "    else assertTrue(Gate.allow(n));",
        "  }",
        "}",
        "",
      ].join("\n"),
    )
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/main/java/gate/Gate.java", source, flipped))
    const first = run(dir, path.join(dir, "out"), ["--suite-timeout-ms", "180000"])
    assert.equal(first.ok, true, first.summary)
    assert.equal(first.killed, 1, `${first.summary} ${JSON.stringify(first.kills)} ${JSON.stringify(first.commands)}`)
    const tally = tallyOf(path.join(dir, "out"))
    assert.equal(tally.pruning.deletedTests, 0)
    const id = tally.keep.find((name) => name.includes("testShut["))
    assert.ok(id, `${JSON.stringify(tally.keep)} ${first.summary}`)
    assert.equal(id?.includes("::"), false, id)
    const again = run(dir, path.join(dir, "again"), ["--only-test", id as string, "--suite-timeout-ms", "180000"])
    assert.equal(again.killed, 1, again.summary)
    const command = again.commands?.[0]?.command ?? ""
    assert.match(command, /#testShut(?:[, ]|$)/)
    assert.equal(command.includes("testShut["), false, command)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("::command is refused as --only-test", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-keep-cmd-"))
  try {
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "patches", "m.patch"), "# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug\n")
    const result = cli(["mutate", "run", "--package", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--only-test", "::command", "--no-confirm", "--no-build"])
    const body = JSON.parse(result.stdout) as RunBody
    assert.equal(body.ok, false, result.stdout)
    assert.match(body.summary, /::command/)
    assert.equal(body.killed ?? 0, 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function run(dir: string, out: string, extra: string[] = []): RunBody {
  const result = cli(["mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", out, "--no-build", "--no-confirm", "--workers", "1", ...extra])
  assert.equal(result.stdout.trim().startsWith("{"), true, result.stderr + result.stdout)
  return JSON.parse(result.stdout) as RunBody
}

function tallyOf(out: string): TallyBody {
  const result = cli(["mutate", "tally", "--out", out])
  assert.equal(result.status, 0, result.stderr + result.stdout)
  return JSON.parse(result.stdout) as TallyBody
}

function cli(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", timeout: 280_000 })
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
