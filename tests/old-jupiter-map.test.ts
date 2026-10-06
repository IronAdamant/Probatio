import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { missingTool } from "./require-tool.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("an older junit platform still writes a line map and narrows -Dtest", { timeout: 300_000, skip: missingTool("mvn") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-jupiter-17-"))
  try {
    mkdirSync(path.join(dir, "src", "main", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "src", "test", "java", "gate"), { recursive: true })
    mkdirSync(path.join(dir, "patches"))
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
        "      <version>5.7.2</version>",
        "      <scope>test</scope>",
        "    </dependency>",
        "  </dependencies>",
        "  <build>",
        "    <plugins>",
        "      <plugin>",
        "        <groupId>org.apache.maven.plugins</groupId>",
        "        <artifactId>maven-surefire-plugin</artifactId>",
        "        <version>2.22.2</version>",
        "      </plugin>",
        "    </plugins>",
        "  </build>",
        "</project>",
        "",
      ].join("\n"),
    )
    const source = "package gate;\npublic class Gate {\n  public static boolean allow(int n) {\n    return n > 0;\n  }\n}\n"
    writeFileSync(path.join(dir, "src", "main", "java", "gate", "Gate.java"), source)
    writeFileSync(
      path.join(dir, "src", "test", "java", "gate", "GateTest.java"),
      [
        "package gate;",
        "import org.junit.jupiter.api.Test;",
        "import static org.junit.jupiter.api.Assertions.*;",
        "class GateTest {",
        "  @Test void testShut() { assertFalse(Gate.allow(0)); }",
        "  @Test void testOther() { assertEquals(1, 1); }",
        "}",
        "",
      ].join("\n"),
    )
    commit(dir)
    writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/main/java/gate/Gate.java", source, source.replace("n > 0", "n >= 0")))
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm", "--workers", "1", "--suite-timeout-ms", "180000"],
      { cwd: root, encoding: "utf8", timeout: 280_000 },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; commands?: Array<{ command: string }> }
    assert.equal(body.ok, true, body.summary)
    const errorPath = path.join(dir, "out", "coverage-error.txt")
    assert.equal(existsSync(path.join(dir, "out", "coverage-map.json")), true, existsSync(errorPath) ? readFileSync(errorPath, "utf8") : body.summary)
    assert.equal(existsSync(errorPath), false)
    const command = body.commands?.[0]?.command ?? ""
    assert.match(command, /-Dtest=/)
    assert.match(command, /testShut/)
    assert.equal(command.includes("testOther"), false, command)
    assert.equal(body.killed, 1, body.summary)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function commit(repo: string) {
  const git = (args: string[]) => {
    const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
    if (result.status !== 0) throw new Error(result.stderr || result.stdout)
  }
  git(["init", "-q"])
  git(["add", "."])
  git(["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
}
