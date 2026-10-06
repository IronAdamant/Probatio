import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { missingTool } from "./require-tool.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("mutate run kills a Java mutant that fails javac", { timeout: 180_000, skip: missingTool("mvn") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-java-"))
  const source = "package example;\npublic class Gate {\n  public static boolean open(int n) { return n > 0; }\n}\n"
  const broken = "package example;\npublic class Gate {\n  public static boolean open(int n) { return n > ; }\n}\n"
  mkdirSync(path.join(dir, "src", "main", "java", "example"), { recursive: true })
  mkdirSync(path.join(dir, "src", "test", "java", "example"), { recursive: true })
  writeFileSync(path.join(dir, "pom.xml"), pom())
  writeFileSync(path.join(dir, "src", "main", "java", "example", "Gate.java"), source)
  writeFileSync(
    path.join(dir, "src", "test", "java", "example", "GateTest.java"),
    [
      "package example;",
      "import static org.junit.jupiter.api.Assertions.assertFalse;",
      "import org.junit.jupiter.api.Test;",
      "class GateTest {",
      "  @Test void low() { assertFalse(Gate.open(0)); }",
      "}",
      "",
    ].join("\n"),
  )
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  const patches = path.join(dir, "patches")
  mkdirSync(patches)
  writeFileSync(path.join(patches, "m-javac.patch"), forwardDiff("src/main/java/example/Gate.java", source, broken))
  try {
    const result = launch(dir, patches, path.join(dir, "out"))
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; survived: number }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.survived, 0, body.summary)
    assert.match(body.summary, /\b1 killed\b/)
    const saved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-javac.json"), "utf8")) as { outcome: string; killedBy: string[] }
    assert.equal(saved.outcome, "killed")
    assert.ok(saved.killedBy.length > 0, JSON.stringify(saved))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function pom(): string {
  return [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<project xmlns="http://maven.apache.org/POM/4.0.0">',
    "  <modelVersion>4.0.0</modelVersion>",
    "  <groupId>example</groupId>",
    "  <artifactId>gate</artifactId>",
    "  <version>1.0</version>",
    "  <properties>",
    "    <maven.compiler.release>17</maven.compiler.release>",
    "    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>",
    "  </properties>",
    "  <dependencies>",
    "    <dependency>",
    "      <groupId>org.junit.jupiter</groupId>",
    "      <artifactId>junit-jupiter</artifactId>",
    "      <version>5.11.4</version>",
    "      <scope>test</scope>",
    "    </dependency>",
    "  </dependencies>",
    "  <build>",
    "    <plugins>",
    "      <plugin>",
    "        <artifactId>maven-compiler-plugin</artifactId>",
    "        <version>3.13.0</version>",
    "      </plugin>",
    "      <plugin>",
    "        <artifactId>maven-surefire-plugin</artifactId>",
    "        <version>3.5.2</version>",
    "      </plugin>",
    "    </plugins>",
    "  </build>",
    "</project>",
    "",
  ].join("\n")
}

function launch(dir: string, patches: string, out: string) {
  return spawnSync(
    tsx,
    [
      "src/cli.ts",
      "mutate",
      "run",
      "--package",
      dir,
      "--repo",
      dir,
      "--patches",
      patches,
      "--out",
      out,
      "--no-build",
      "--no-confirm",
      "--max-mutants",
      "1",
      "--workers",
      "1",
      "--suite-timeout-ms",
      "180000",
    ],
    { cwd: root, encoding: "utf8" },
  )
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
