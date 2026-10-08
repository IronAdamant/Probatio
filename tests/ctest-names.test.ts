import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"
import { firstSkip, missingTool } from "./require-tool.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("ctest kill names the failing test, not the whole command", { timeout: 90_000, skip: firstSkip(missingTool("cmake"), missingTool("ctest")) }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-ctest-"))
  try {
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "gate.txt"), "shut\n")
    writeFileSync(path.join(dir, "stays_open.sh"), "#!/bin/sh\nexit 0\n")
    writeFileSync(path.join(dir, "stays_closed.sh"), "#!/bin/sh\ncd \"$(dirname \"$0\")\"\ngrep -qx shut gate.txt\n")
    writeFileSync(
      path.join(dir, "CMakeLists.txt"),
      [
        "cmake_minimum_required(VERSION 3.16)",
        "project(gate NONE)",
        "enable_testing()",
        "add_test(NAME stays_open COMMAND sh \"${CMAKE_SOURCE_DIR}/stays_open.sh\")",
        "add_test(NAME stays_closed COMMAND sh \"${CMAKE_SOURCE_DIR}/stays_closed.sh\")",
        "",
      ].join("\n"),
    )
    writeFileSync(path.join(dir, "patches", "m-open.patch"), forwardDiff("gate.txt", "shut\n", "open\n"))
    commit(dir)
    const ran = launch(dir, [
      "--suite-command",
      "cmake -S . -B build >/dev/null && ctest --test-dir build --output-on-failure",
    ])
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as {
      ok: boolean
      summary: string
      killed: number
      kills?: Array<{ killedBy?: string[] }>
    }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    const killedBy = body.kills?.[0]?.killedBy ?? []
    assert.deepEqual(killedBy, ["stays_closed"], JSON.stringify(body))
    assert.ok(!killedBy.includes("::command"), JSON.stringify(killedBy))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a suite that prints no test names stays ::command and tally keeps nothing", { timeout: 60_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-noname-"))
  try {
    mkdirSync(path.join(dir, "patches"))
    writeFileSync(path.join(dir, "gate.txt"), "ok\n")
    writeFileSync(path.join(dir, "patches", "m-bad.patch"), forwardDiff("gate.txt", "ok\n", "bad\n"))
    commit(dir)
    const ran = launch(dir, ["--suite-command", "grep -qx ok gate.txt"])
    assert.equal(ran.status, 0, ran.stderr + ran.stdout)
    const body = JSON.parse(ran.stdout) as { ok: boolean; summary: string; kills?: Array<{ killedBy?: string[] }> }
    assert.equal(body.ok, true, body.summary)
    assert.deepEqual(body.kills?.[0]?.killedBy, ["::command"], JSON.stringify(body))
    const tally = spawnSync(tsx, ["src/cli.ts", "mutate", "tally", "--out", path.join(dir, "out")], {
      cwd: root,
      encoding: "utf8",
    })
    assert.equal(tally.status, 0, tally.stderr + tally.stdout)
    const report = JSON.parse(tally.stdout) as { keep: string[]; noKillsYet: string[]; pruning: { deletedTests: number } }
    assert.deepEqual(report.keep, [])
    assert.deepEqual(report.noKillsYet, [])
    assert.equal(report.pruning.deletedTests, 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function launch(dir: string, extra: string[]) {
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
      path.join(dir, "patches"),
      "--out",
      path.join(dir, "out"),
      "--no-build",
      "--no-confirm",
      "--workers",
      "1",
      "--suite-timeout-ms",
      "30000",
      ...extra,
    ],
    { cwd: root, encoding: "utf8" },
  )
}

function commit(repo: string): void {
  const git = (args: string[]) => {
    const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
    if (result.status !== 0) throw new Error(result.stderr || result.stdout)
  }
  git(["init", "-q"])
  git(["add", "."])
  git(["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
}
