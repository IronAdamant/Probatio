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

const source = [
  "#include <stdio.h>",
  "int main(void) {",
  "  unsigned n = 0;",
  "  int closed = 1;",
  "  if (n > 0) {",
  "    printf(\"Testing stays closed\\n\");",
  "    return 1;",
  "  }",
  "  if (closed == 0) {",
  "    printf(\"Testing stays closed\\n\");",
  "    return 1;",
  "  }",
  "  printf(\"Testing stays closed\\n\");",
  "  printf(\"All done, tests as expected\\n\");",
  "  return 0;",
  "}",
  "",
].join("\n")

test("a shell suite labels a compiler error separately from a named test", { timeout: 60_000, skip: missingTool("cc") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-script-"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "tests", "main.c"), source)
  writeFileSync(
    path.join(dir, "run_tests.sh"),
    ["#!/bin/bash", "set -euo pipefail", "cc -Werror -Wtype-limits -o suite tests/main.c", "./suite", ""].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-test.patch"), forwardDiff("tests/main.c", source, source.replace("closed == 0", "closed == 1")))
  writeFileSync(path.join(dir, "patches", "m-build.patch"), forwardDiff("tests/main.c", source, source.replace("n > 0", "n >= 0")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm", "--workers", "1", "--suite-timeout-ms", "30000"],
      { cwd: root, encoding: "utf8" },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as {
      ok: boolean
      summary: string
      killed: number
      survived: number
      suiteCommand?: string
      kills?: Array<{ id: string; cause: string; next: string }>
    }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 2, body.summary)
    assert.equal(body.survived, 0, body.summary)
    const byTest = (body.kills ?? []).find((item) => item.id === "m-test")
    const byBuild = (body.kills ?? []).find((item) => item.id === "m-build")
    assert.ok(byTest, JSON.stringify(body.kills))
    assert.ok(byBuild, JSON.stringify(body.kills))
    assert.equal(byTest.cause, "test")
    assert.equal(byBuild.cause, "build")
    assert.match(byTest.next, /Testing stays closed/)
    assert.match(byBuild.next, /build/i)
    assert.doesNotMatch(byBuild.next, /add a test named (javac|compiler|build|cobc|nasm|clang|cargo|tsc|dotnet|swiftc)/i)
    const saved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-build.json"), "utf8")) as { killedBy: string[]; command: string }
    assert.deepEqual(saved.killedBy, ["clang"])
    assert.equal(body.suiteCommand, "bash run_tests.sh")
    assert.equal(saved.command, "bash run_tests.sh")
    assert.doesNotMatch(saved.command, /\bscript\b/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a shell suite that runs docker with a tty is wrapped for both BSD and GNU script", { timeout: 60_000, skip: missingTool("cc") }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-script-tty-"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "tests", "main.c"), source)
  writeFileSync(
    path.join(dir, "run_tests.sh"),
    ["#!/bin/bash", "set -euo pipefail", "# tini: docker run -it --rm example", "cc -Werror -Wtype-limits -o suite tests/main.c", "./suite", ""].join("\n"),
  )
  writeFileSync(path.join(dir, "patches", "m-test.patch"), forwardDiff("tests/main.c", source, source.replace("closed == 0", "closed == 1")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-confirm", "--workers", "1", "--suite-timeout-ms", "30000"],
      { cwd: root, encoding: "utf8" },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; commands?: Array<{ id: string; command: string }> }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    const command = body.commands?.find((item) => item.id === "m-test")?.command ?? ""
    assert.match(command, /^script /)
    assert.match(command, /bash run_tests\.sh/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
