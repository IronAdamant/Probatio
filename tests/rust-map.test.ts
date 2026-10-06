import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { firstSkip, missingTool } from "./require-tool.ts"
import { fileURLToPath } from "node:url"
import { changedLines, forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

const source = [
  "pub fn open(n: i32) -> bool {",
  "    n > 0",
  "}",
  "",
  "pub fn idle() -> i32 {",
  "    1",
  "}",
  "",
  "#[cfg(test)]",
  "mod tests {",
  "    use super::*;",
  "",
  "    #[test]",
  "    fn stays_closed() {",
  "        assert!(!open(0));",
  "    }",
  "",
  "    #[test]",
  "    fn stays_open() {",
  "        assert!(open(1));",
  "    }",
  "",
  "    #[test]",
  "    fn other() {",
  "        assert_eq!(2 + 2, 4);",
  "    }",
  "}",
  "",
].join("\n")

test("a rust line map names the killing test and reports an unexecuted line as no coverage", { timeout: 300_000, skip: firstSkip(missingTool("cargo"), missingTool("llvm-cov"), missingTool("llvm-profdata")) }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-rust-map-"))
  try {
    mkdirSync(path.join(dir, "src"))
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "Cargo.toml"), "[package]\nname = \"gate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
    writeFileSync(path.join(dir, "src", "lib.rs"), source)
    writeFileSync(path.join(dir, "patches", "m-open.patch"), forwardDiff("src/lib.rs", source, source.replace("    n > 0\n", "    n >= 0\n")))
    writeFileSync(path.join(dir, "patches", "m-idle.patch"), forwardDiff("src/lib.rs", source, source.replace("    1\n", "    2\n")))
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
      "120000",
    ])
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string; kills?: Array<{ id: string; killedBy?: string[] }> }
    assert.equal(body.ok, true, `${body.summary}\n${ran.stderr}`)
    const results = readResults(path.join(dir, "out", "results"))
    const open = results.find((item) => item.id === "m-open")
    const idle = results.find((item) => item.id === "m-idle")
    assert.ok(open, JSON.stringify(results, null, 2))
    assert.equal(open.outcome, "killed", open.command)
    assert.ok(open.killedBy.some((name) => name.includes("stays_closed")), JSON.stringify(open.killedBy))
    assert.match(open.command, /stays_closed/)
    assert.equal(open.command.includes("other"), false, open.command)
    assert.ok(idle, JSON.stringify(results, null, 2))
    assert.equal(idle.outcome, "no coverage", `${idle.outcome} ${idle.command}`)
    assert.equal(idle.command, "")
    const mapPath = path.join(dir, "out", "coverage-map.json")
    const map = JSON.parse(readFileSync(mapPath, "utf8")) as { files: Record<string, Record<string, string[]>> }
    const file = Object.keys(map.files).find((name) => name.endsWith("lib.rs"))
    assert.ok(file, JSON.stringify(Object.keys(map.files)))
    const lines = map.files[file]
    const openLine = String(changedLines(readFileSync(path.join(dir, "patches", "m-open.patch"), "utf8"))[0]?.line)
    const idleLine = String(changedLines(readFileSync(path.join(dir, "patches", "m-idle.patch"), "utf8"))[0]?.line)
    assert.ok((lines[openLine] ?? []).some((name) => name.includes("stays_closed")), JSON.stringify(lines))
    assert.equal(lines[idleLine], undefined, JSON.stringify(lines))
    const named = (body.kills ?? []).find((item) => item.id === "m-open")
    assert.ok(named?.killedBy?.some((name) => name.includes("stays_closed")), JSON.stringify(body.kills))
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
