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
  // The project says how its shell suite names tests. Probatio does not guess one project's output.
  writeFileSync(path.join(dir, ".probatio.json"), `${JSON.stringify({ testLine: "^Testing .+$", passLine: "All done, tests as expected" })}\n`)
  writeFileSync(path.join(dir, "patches", "m-test.patch"), forwardDiff("tests/main.c", source, source.replace("closed == 0", "closed == 1")))
  writeFileSync(path.join(dir, "patches", "m-build.patch"), forwardDiff("tests/main.c", source, source.replace("n > 0", "n >= 0")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-build", "--no-confirm", "--workers", "1", "--suite-timeout-ms", "30000"],
      { cwd: root, encoding: "utf8", timeout: 300_000 },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as {
      ok: boolean
      summary: string
      killed: number
      unviable: number
      survived: number
      suiteCommand?: string
      kills?: Array<{ id: string; cause: string; next: string }>
    }
    assert.equal(body.ok, true, body.summary)
    // The compiler rejecting a mutant is not a test catching it.
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.unviable, 1, body.summary)
    assert.equal(body.survived, 0, body.summary)
    const byTest = (body.kills ?? []).find((item) => item.id === "m-test")
    assert.ok(byTest, JSON.stringify(body.kills))
    assert.equal((body.kills ?? []).some((item) => item.id === "m-build"), false, JSON.stringify(body.kills))
    assert.equal(byTest.cause, "test")
    assert.match(byTest.next, /Testing stays closed/)
    const saved = JSON.parse(readFileSync(path.join(dir, "out", "results", "m-build.json"), "utf8")) as { outcome: string; cause: string; next: string; killedBy: string[]; command: string }
    assert.equal(saved.outcome, "unviable")
    assert.equal(saved.cause, "build")
    assert.match(saved.next, /did not build/)
    assert.doesNotMatch(saved.next, /add a test named (javac|compiler|build|cobc|nasm|clang|cargo|tsc|dotnet|swiftc)/i)
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
    ["#!/bin/bash", "set -euo pipefail", "# docker run -it --rm example", "cc -Werror -Wtype-limits -o suite tests/main.c", "./suite", ""].join("\n"),
  )
  writeFileSync(path.join(dir, ".probatio.json"), `${JSON.stringify({ testLine: "^Testing .+$" })}\n`)
  writeFileSync(path.join(dir, "patches", "m-test.patch"), forwardDiff("tests/main.c", source, source.replace("closed == 0", "closed == 1")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, "out"), "--no-confirm", "--workers", "1", "--suite-timeout-ms", "30000"],
      { cwd: root, encoding: "utf8", timeout: 300_000 },
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

test("a shell suite with no test lines is one command, TAP names tests, and .probatio.json env reaches the suite", { timeout: 60_000 }, () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-script-plain-"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  const gate = "#!/bin/bash\nopen() { [ \"$1\" -gt 0 ]; }\n"
  writeFileSync(path.join(dir, "tests", "gate.sh"), gate)
  writeFileSync(
    path.join(dir, "run_tests.sh"),
    [
      "#!/bin/bash",
      "source tests/gate.sh",
      // The project's CI sets this. Without it the suite refuses to run.
      '[ "${GATE_CI:-}" = "1" ] || { echo "set GATE_CI"; exit 2; }',
      'if [ -n "${GATE_TAP:-}" ]; then',
      '  if open 0; then echo "not ok 1 - zero stays shut"; exit 1; else echo "ok 1 - zero stays shut"; fi',
      "else",
      "  if open 0; then exit 1; fi",
      "fi",
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, ".probatio.json"), `${JSON.stringify({ env: { GATE_CI: "1" } })}\n`)
  writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("tests/gate.sh", gate, gate.replace("-gt", "-ge")))
  git(dir, ["init", "-q"])
  git(dir, ["add", "."])
  git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  const launch = (out: string, env: NodeJS.ProcessEnv = process.env) =>
    spawnSync(
      tsx,
      ["src/cli.ts", "mutate", "run", "--package", dir, "--repo", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, out), "--no-confirm", "--workers", "1", "--suite-timeout-ms", "30000"],
      { cwd: root, encoding: "utf8", env, timeout: 300_000 },
    )
  try {
    type Body = { ok: boolean; summary: string; killed: number; kills?: Array<{ killedBy: string[] }> }
    const plain = JSON.parse(launch("plain").stdout) as Body
    assert.equal(plain.ok, true, plain.summary)
    assert.equal(plain.killed, 1, plain.summary)
    assert.deepEqual(plain.kills?.[0]?.killedBy, ["::command"], "no test line, so no invented test name")
    const tap = JSON.parse(launch("tap", { ...process.env, GATE_TAP: "1" }).stdout) as Body
    assert.equal(tap.ok, true, tap.summary)
    assert.deepEqual(tap.kills?.[0]?.killedBy, ["zero stays shut"])
    writeFileSync(path.join(dir, ".probatio.json"), '{ "testLine": "([" }\n')
    const broken = JSON.parse(launch("broken").stdout) as Body
    assert.equal(broken.ok, false, broken.summary)
    assert.match(broken.summary, /\.probatio\.json testLine/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
