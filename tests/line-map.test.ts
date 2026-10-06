import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { missingTool } from "./require-tool.ts"
import { fileURLToPath } from "node:url"
import { selectionFor } from "../src/mutate/coverage-map.ts"
import { parseCoverProfile } from "../src/mutate/go-coverage.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("the selector keeps the tests on a covered line and skips a line the map does not have", () => {
  const map = { files: { "gate.go": { "4": ["TestOpens", "TestShut"] }, "src/main/java/gate/Gate.java": { "4": ["gate.GateTest.testOpens", "gate.GateTest.testShut"] } } }
  const go = selectionFor(map, [{ file: "gate.go", line: 4 }])
  assert.equal(go.state, "covered")
  if (go.state === "covered") assert.deepEqual(go.tests, ["TestOpens", "TestShut"])
  const dark = selectionFor(map, [{ file: "unused.go", line: 4 }])
  assert.equal(dark.state, "uncovered")
  const java = selectionFor(map, [{ file: "src/main/java/gate/Gate.java", line: 4 }])
  assert.equal(java.state, "covered")
  if (java.state === "covered") assert.equal(java.tests.includes("gate.GateTest.testOther"), false)
  const hits = parseCoverProfile("mode: set\nexample.com/gate/gate.go:4.2,4.16 1 1\nexample.com/gate/unused.go:3.2,3.10 1 0\n", "example.com/gate")
  assert.deepEqual(hits, [{ file: "gate.go", line: 4 }])
})

test("a go line map runs only the tests that hit the line", { timeout: 120_000, skip: missingTool("go") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-go-map-"))
  try {
    writeFileSync(path.join(dir, "go.mod"), "module example.com/gate\n\ngo 1.22\n")
    writeFileSync(path.join(dir, "gate.go"), "package gate\n\nfunc Gate(n int) bool {\n\treturn n > 0\n}\n")
    writeFileSync(path.join(dir, "unused.go"), "package gate\n\nfunc Unused(n int) bool {\n\treturn n > 0\n}\n")
    writeFileSync(
      path.join(dir, "gate_test.go"),
      [
        "package gate",
        "",
        'import "testing"',
        "",
        "func TestOpens(t *testing.T) {",
        '  if !Gate(1) { t.Fatal("open") }',
        "}",
        "func TestShut(t *testing.T) {",
        '  if Gate(0) { t.Fatal("shut") }',
        "}",
        "func TestOther(t *testing.T) {",
        '  if 1 != 1 { t.Fatal("other") }',
        "}",
        "",
      ].join("\n"),
    )
    commit(dir)
    const generated = cli(["mutate", "generate", "--package", dir, "--src", ".", "--out", path.join(dir, "gen")])
    assert.equal(generated.status, 0, generated.stderr + generated.stdout)
    const ran = cli(["mutate", "run", "--package", dir, "--patches", path.join(dir, "gen", "mutants"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm"])
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string; commands: Array<{ outcome: string; command: string }> }
    assert.equal(body.ok, true, body.summary)
    assert.match(body.summary, /^\d+ no coverage, \d+ survived/)
    const results = readResults(path.join(dir, "out", "results"))
    const gate = results.find((item) => item.files.some((file) => file.file === "gate.go") && item.command)
    const unused = results.find((item) => item.files.some((file) => file.file === "unused.go"))
    assert.ok(gate, JSON.stringify(results))
    assert.match(gate.command, /-run/)
    assert.match(gate.command, /TestOpens/)
    assert.match(gate.command, /TestShut/)
    assert.equal(gate.command.includes("TestOther"), false, gate.command)
    console.log(`go command: ${gate.command}`)
    assert.ok(unused, JSON.stringify(results))
    assert.equal(unused.outcome, "no coverage")
    assert.equal(unused.command, "")
    const map = JSON.parse(readFileSync(path.join(dir, "out", "coverage-map.json"), "utf8")) as { files: Record<string, Record<string, string[]>> }
    const names = new Set(Object.values(map.files).flatMap((lines) => Object.values(lines).flat()))
    assert.ok(names.has("TestOpens"))
    assert.ok(names.has("TestShut"))
    assert.equal(names.has("TestOther"), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a maven line map runs only the tests that hit the line", { timeout: 180_000, skip: missingTool("mvn") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-java-map-"))
  try {
    mkdirSync(path.join(dir, "src", "main", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "src", "test", "java", "gate"), { recursive: true })
    writeFileSync(
      path.join(dir, "pom.xml"),
      [
        "<project>",
        "  <modelVersion>4.0.0</modelVersion>",
        "  <groupId>example</groupId>",
        "  <artifactId>gate</artifactId>",
        "  <version>1.0</version>",
        "  <dependencies>",
        "    <dependency>",
        "      <groupId>junit</groupId>",
        "      <artifactId>junit</artifactId>",
        "      <version>4.13.2</version>",
        "      <scope>test</scope>",
        "    </dependency>",
        "  </dependencies>",
        "</project>",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(dir, "src", "main", "java", "gate", "Gate.java"), "package gate;\npublic class Gate {\n  public static boolean allow(int n) {\n    return n > 0;\n  }\n}\n")
    writeFileSync(path.join(dir, "src", "main", "java", "gate", "Unused.java"), "package gate;\npublic class Unused {\n  public static boolean dark(int n) {\n    return n > 0;\n  }\n}\n")
    writeFileSync(
      path.join(dir, "src", "test", "java", "gate", "GateTest.java"),
      [
        "package gate;",
        "import org.junit.Test;",
        "import static org.junit.Assert.*;",
        "public class GateTest {",
        "  @Test public void testOpens() { assertTrue(Gate.allow(1)); }",
        "  @Test public void testShut() { assertFalse(Gate.allow(0)); }",
        "  @Test public void testOther() { assertEquals(1, 1); }",
        "}",
        "",
      ].join("\n"),
    )
    commit(dir)
    const generated = cli(["mutate", "generate", "--package", dir, "--src", "src/main/java", "--out", path.join(dir, "gen")])
    assert.equal(generated.status, 0, generated.stderr + generated.stdout)
    const ran = cli(["mutate", "run", "--package", dir, "--patches", path.join(dir, "gen", "mutants"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm", "--suite-timeout-ms", "120000"])
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string }
    assert.equal(body.ok, true, `${body.summary}\n${ran.stderr}`)
    const results = readResults(path.join(dir, "out", "results"))
    const gate = results.find((item) => item.files.some((file) => file.file.endsWith("Gate.java")) && item.command)
    const unused = results.find((item) => item.files.some((file) => file.file.endsWith("Unused.java")))
    assert.ok(gate, `${body.summary}\n${JSON.stringify(results, null, 2)}`)
    assert.match(gate.command, /-Dtest=/)
    assert.match(gate.command, /testOpens/)
    assert.match(gate.command, /testShut/)
    assert.equal(gate.command.includes("testOther"), false, gate.command)
    console.log(`java command: ${gate.command}`)
    assert.ok(unused)
    assert.equal(unused.outcome, "no coverage", unused.command)
    assert.equal(unused.command, "")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a junit 5 line map names the test on the line and narrows -Dtest", { timeout: 300_000, skip: missingTool("mvn") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-jupiter-map-"))
  try {
    mkdirSync(path.join(dir, "src", "main", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "src", "test", "java", "gate"), { recursive: true })
    writeFileSync(
      path.join(dir, "pom.xml"),
      [
        "<project>",
        "  <modelVersion>4.0.0</modelVersion>",
        "  <groupId>example</groupId>",
        "  <artifactId>gate</artifactId>",
        "  <version>1.0</version>",
        "  <dependencies>",
        "    <dependency>",
        "      <groupId>org.junit.jupiter</groupId>",
        "      <artifactId>junit-jupiter</artifactId>",
        "      <version>5.11.4</version>",
        "      <scope>test</scope>",
        "    </dependency>",
        "    <dependency>",
        "      <groupId>org.junit.jupiter</groupId>",
        "      <artifactId>junit-jupiter-params</artifactId>",
        "      <version>5.11.4</version>",
        "      <scope>test</scope>",
        "    </dependency>",
        "  </dependencies>",
        "  <build>",
        "    <plugins>",
        "      <plugin>",
        "        <groupId>org.apache.maven.plugins</groupId>",
        "        <artifactId>maven-surefire-plugin</artifactId>",
        "        <version>3.5.2</version>",
        "      </plugin>",
        "    </plugins>",
        "  </build>",
        "</project>",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(dir, "src", "main", "java", "gate", "Gate.java"), "package gate;\npublic class Gate {\n  public static boolean allow(int n) {\n    return n > 0;\n  }\n}\n")
    writeFileSync(path.join(dir, "src", "main", "java", "gate", "Unused.java"), "package gate;\npublic class Unused {\n  public static boolean dark(int n) {\n    return n > 0;\n  }\n}\n")
    writeFileSync(
      path.join(dir, "src", "test", "java", "gate", "GateTest.java"),
      [
        "package gate;",
        "import org.junit.jupiter.api.Test;",
        "import org.junit.jupiter.params.ParameterizedTest;",
        "import org.junit.jupiter.params.provider.ValueSource;",
        "import static org.junit.jupiter.api.Assertions.*;",
        "class GateTest {",
        "  @Test void testOpens() { assertTrue(Gate.allow(1)); }",
        "  @Test void testShut() { assertFalse(Gate.allow(0)); }",
        "  @Test void testOther() { assertEquals(1, 1); }",
        "  @ParameterizedTest",
        "  @ValueSource(ints = {1, 2})",
        "  void testPositive(int n) { assertTrue(Gate.allow(n)); }",
        "}",
        "",
      ].join("\n"),
    )
    commit(dir)
    const generated = cli(["mutate", "generate", "--package", dir, "--src", "src/main/java", "--out", path.join(dir, "gen")])
    assert.equal(generated.status, 0, generated.stderr + generated.stdout)
    const ran = cli(["mutate", "run", "--package", dir, "--patches", path.join(dir, "gen", "mutants"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm", "--suite-timeout-ms", "180000"])
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string }
    assert.equal(body.ok, true, `${body.summary}\n${ran.stderr}`)
    const mapPath = path.join(dir, "out", "coverage-map.json")
    const errorPath = path.join(dir, "out", "coverage-error.txt")
    const hitsPath = path.join(dir, "out", "java-cover", "hits.txt")
    const hits = existsSync(hitsPath) ? readFileSync(hitsPath, "utf8").slice(0, 1500) : "no hits file"
    assert.equal(existsSync(mapPath), true, `${body.summary}\n${existsSync(errorPath) ? readFileSync(errorPath, "utf8") : ""}\n${hits}`)
    const map = JSON.parse(readFileSync(mapPath, "utf8")) as { files: Record<string, Record<string, string[]>> }
    const gateFile = Object.keys(map.files).find((file) => file.endsWith("Gate.java"))
    assert.ok(gateFile, JSON.stringify(Object.keys(map.files)))
    const lines = map.files[gateFile]
    const onReturn = lines["4"] ?? []
    assert.ok(onReturn.some((name) => name.includes("testOpens")), JSON.stringify(lines))
    assert.ok(onReturn.some((name) => name.includes("testShut")), JSON.stringify(lines))
    assert.ok(onReturn.some((name) => name.endsWith("testPositive")), JSON.stringify(lines))
    assert.equal(onReturn.some((name) => name.includes("testOther")), false, JSON.stringify(lines))
    const results = readResults(path.join(dir, "out", "results"))
    const gate = results.find((item) => item.files.some((file) => file.file.endsWith("Gate.java")) && item.command)
    const unused = results.find((item) => item.files.some((file) => file.file.endsWith("Unused.java")))
    assert.ok(gate, JSON.stringify(results, null, 2))
    assert.match(gate.command, /-Dtest=/)
    assert.match(gate.command, /testOpens/)
    assert.match(gate.command, /testShut/)
    assert.match(gate.command, /testPositive/)
    assert.equal(gate.command.includes("testOther"), false, gate.command)
    assert.equal(gate.command.includes("[1]"), false, gate.command)
    assert.equal(gate.command.includes("(int)"), false, gate.command)
    for (const name of onReturn) {
      const leaf = name.slice(name.lastIndexOf(".") + 1)
      assert.match(gate.command, new RegExp(leaf.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")))
    }
    console.log(`jupiter command: ${gate.command}`)
    assert.ok(unused)
    assert.equal(unused.outcome, "no coverage", unused.command)
    assert.equal(unused.command, "")
    assert.equal(existsSync(path.join(dir, "out", "coverage-error.txt")), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("maven confirm reruns the failing test by name", { timeout: 300_000, skip: missingTool("mvn") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-java-confirm-"))
  try {
    mkdirSync(path.join(dir, "src", "main", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "src", "test", "java", "gate"), { recursive: true })
    writeFileSync(
      path.join(dir, "pom.xml"),
      [
        "<project>",
        "  <modelVersion>4.0.0</modelVersion>",
        "  <groupId>example</groupId>",
        "  <artifactId>gate</artifactId>",
        "  <version>1.0</version>",
        "  <dependencies>",
        "    <dependency>",
        "      <groupId>junit</groupId>",
        "      <artifactId>junit</artifactId>",
        "      <version>4.13.2</version>",
        "      <scope>test</scope>",
        "    </dependency>",
        "  </dependencies>",
        "</project>",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(dir, "src", "main", "java", "gate", "Gate.java"), "package gate;\npublic class Gate {\n  public static boolean allow(int n) {\n    return n > 0;\n  }\n}\n")
    writeFileSync(
      path.join(dir, "src", "test", "java", "gate", "GateTest.java"),
      [
        "package gate;",
        "import org.junit.Test;",
        "import static org.junit.Assert.*;",
        "public class GateTest {",
        "  @Test public void testOpens() { assertTrue(Gate.allow(1)); }",
        "  @Test public void testShut() { assertFalse(Gate.allow(0)); }",
        "}",
        "",
      ].join("\n"),
    )
    commit(dir)
    const generated = cli(["mutate", "generate", "--package", dir, "--src", "src/main/java", "--out", path.join(dir, "gen")])
    assert.equal(generated.status, 0, generated.stderr + generated.stdout)
    const patches = path.join(dir, "gen", "mutants")
    const one = readdirSync(patches).find((name) => name.endsWith(".patch") && readFileSync(path.join(patches, name), "utf8").includes("n >= 0"))
    assert.ok(one, readdirSync(patches).join(","))
    const slice = path.join(dir, "slice")
    mkdirSync(slice)
    writeFileSync(path.join(slice, one), readFileSync(path.join(patches, one)))
    const ran = cli(["mutate", "run", "--package", dir, "--patches", slice, "--out", path.join(dir, "out"), "--no-build", "--suite-timeout-ms", "120000"])
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const results = readResults(path.join(dir, "out", "results"))
    const killed = results.find((item) => item.outcome === "killed")
    assert.ok(killed, JSON.stringify(results, null, 2))
    const parts = killed.command.split(" | ")
    assert.ok(parts.length >= 2, killed.command)
    for (const part of parts) {
      assert.match(part, /^mvn -B test -Dtest=/)
      assert.equal(part.includes("\\"), false, part)
      assert.match(part, /GateTest#test/)
    }
    console.log(`java confirm: ${killed.command}`)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function readResults(dir: string): Array<{ outcome: string; command: string; files: Array<{ file: string; line: number }> }> {
  return readdirSync(dir)
    .filter((name) => name.endsWith(".json"))
    .map((name) => JSON.parse(readFileSync(path.join(dir, name), "utf8")) as { outcome: string; command: string; files: Array<{ file: string; line: number }> })
}

function cli(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8" })
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
