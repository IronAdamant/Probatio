import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { tallyRun } from "../src/mutate/tally.ts"

test("tally nextCall repeats package, patches, and out", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-tally-"))
  try {
    mkdirSync(path.join(dir, "results"))
    writeFileSync(
      path.join(dir, "results", "m1.json"),
      `${JSON.stringify({ id: "m1", outcome: "killed", killedBy: ["tests/gate.test.ts::stays shut"], files: [{ file: "src/gate.ts", line: 4 }] })}\n`,
    )
    writeFileSync(
      path.join(dir, "run.json"),
      `${JSON.stringify({
        id: "run",
        commit: "abc",
        package: "/tmp/pkg",
        patches: ["/tmp/pkg/mutants"],
        out: dir,
        confirm: false,
        workers: 1,
      })}\n`,
    )
    const body = tallyRun(dir)
    const argv = body.nextCall?.argv ?? []
    assert.equal(body.ok, true, body.summary)
    assert.ok(argv.includes("--package"), argv.join(" "))
    assert.ok(argv.includes("/tmp/pkg"), argv.join(" "))
    assert.ok(argv.includes("--patches"), argv.join(" "))
    assert.ok(argv.includes("/tmp/pkg/mutants"), argv.join(" "))
    assert.ok(argv.includes("--out"), argv.join(" "))
    assert.ok(argv.includes(dir), argv.join(" "))
    assert.ok(argv.includes("--only-test"), argv.join(" "))
    assert.ok(argv.includes("--no-confirm"), argv.join(" "))
    assert.equal(argv.includes("~/"), false, argv.join(" "))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
