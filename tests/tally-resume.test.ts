import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { resumeArgv, runNextCall, type BudgetResume } from "../src/resume-call.ts"
import { tallyRun } from "../src/mutate/tally.ts"

test("tally nextCall repeats package and patches, and writes a fresh out dir", () => {
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
    // The stored out dir would skip every finished mutant and echo the old result.
    assert.equal(argv.includes(dir), false, argv.join(" "))
    assert.equal(argv[argv.indexOf("--out") + 1], `${dir}.rescore`, argv.join(" "))
    assert.equal(argv[argv.indexOf("--commit") + 1], "abc", argv.join(" "))
    assert.ok(argv.includes("--only-test"), argv.join(" "))
    assert.ok(argv.includes("--no-confirm"), argv.join(" "))
    assert.equal(argv.includes("~/"), false, argv.join(" "))

    // A gap reruns the batch on HEAD after the new test is committed. It is not a keep rescore.
    writeFileSync(
      path.join(dir, "results", "m2.json"),
      `${JSON.stringify({ id: "m2", outcome: "survived", killedBy: [], files: [{ file: "src/gate.ts", line: 9 }] })}\n`,
    )
    const gapped = tallyRun(dir)
    const rerun = gapped.nextCall?.argv ?? []
    assert.equal(rerun[rerun.indexOf("--out") + 1], `${dir}.after-gaps`, rerun.join(" "))
    assert.equal(rerun.includes("--only-test"), false, rerun.join(" "))
    assert.equal(rerun.includes("--commit"), false, rerun.join(" "))
    assert.match(String(gapped.next), /Add one test that fails on that mutant/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("budget resume repeats the flags that change meaning, and a sealed resume keeps every hide", () => {
  const full: BudgetResume = {
    verb: "run",
    packageDir: "/pkg",
    repoDir: "/repo",
    outDir: "/out",
    patchDirs: ["/patches", "/more"],
    commit: "abc123",
    suiteCommand: "npm test",
    build: "npm run build",
    hide: ["tests/a.test.ts"],
    suiteTimeoutMs: 30_000,
    testTimeoutMs: 1_000,
    timeoutMultiple: 4,
    budgetMs: 50,
    maxMinutes: 2,
    timeoutFloorMs: 4_000,
    maxMutants: 4,
    testsDir: "spec",
    workers: 3,
    confirm: false,
    direction: "forward",
    onlyTests: ["stays shut"],
    affected: true,
    unset: ["SECRET"],
    history: "/history",
    label: null,
    fix: null,
  }
  const argv = resumeArgv(full)
  assert.equal(argv[0], "mutate")
  assert.equal(argv[1], "run")
  assert.equal(argv[argv.indexOf("--out") + 1], "/out")
  assert.equal(argv[argv.indexOf("--commit") + 1], "abc123")
  assert.equal(argv[argv.indexOf("--suite-command") + 1], "npm test")
  assert.equal(argv[argv.indexOf("--build") + 1], "npm run build")
  assert.equal(argv[argv.indexOf("--hide") + 1], "tests/a.test.ts")
  assert.equal(argv[argv.indexOf("--suite-timeout-ms") + 1], "30000")
  assert.equal(argv[argv.indexOf("--test-timeout-ms") + 1], "1000")
  assert.equal(argv[argv.indexOf("--timeout-multiple") + 1], "4")
  assert.equal(argv[argv.indexOf("--budget-ms") + 1], "50")
  assert.equal(argv[argv.indexOf("--max-minutes") + 1], "2")
  assert.equal(argv[argv.indexOf("--timeout-floor-ms") + 1], "4000")
  assert.equal(argv[argv.indexOf("--max-mutants") + 1], "4")
  assert.equal(argv[argv.indexOf("--tests-dir") + 1], "spec")
  assert.equal(argv[argv.indexOf("--workers") + 1], "3")
  assert.ok(argv.includes("--no-confirm"))
  assert.equal(argv[argv.indexOf("--direction") + 1], "forward")
  assert.equal(argv[argv.indexOf("--patches") + 1], "/patches")
  assert.ok(argv.includes("/more"))

  const sealed = resumeArgv({
    ...full,
    verb: "sealed",
    hide: ["tests/shut.test.ts", "tests/other.test.ts"],
    label: "/tmp/label.txt",
    fix: "HEAD~1",
  })
  assert.deepEqual(sealed.slice(0, 2), ["mutate", "sealed"])
  assert.deepEqual(sealed.filter((arg) => arg === "--hide").length, 2)
  assert.ok(sealed.includes("tests/shut.test.ts"))
  assert.ok(sealed.includes("tests/other.test.ts"))
  assert.equal(sealed[sealed.indexOf("--label") + 1], "/tmp/label.txt")
  assert.equal(sealed[sealed.indexOf("--fix") + 1], "HEAD~1")
  assert.equal(sealed.includes("run"), false)

  const bare = resumeArgv({
    ...full,
    verb: "run",
    commit: "",
    suiteCommand: null,
    build: null,
    hide: [],
    timeoutMultiple: null,
    budgetMs: null,
    maxMinutes: null,
    timeoutFloorMs: null,
    maxMutants: null,
    testsDir: "tests",
    workers: 1,
    confirm: true,
    direction: "auto",
    onlyTests: [],
    affected: false,
    unset: [],
    history: null,
  })
  for (const flag of ["--commit", "--suite-command", "--build", "--hide", "--timeout-multiple", "--budget-ms", "--max-minutes", "--timeout-floor-ms", "--max-mutants", "--tests-dir", "--no-confirm", "--direction", "--label", "--fix"]) {
    assert.equal(bare.includes(flag), false, flag)
  }
  assert.equal(bare[bare.indexOf("--suite-timeout-ms") + 1], "30000")
  assert.equal(bare[bare.indexOf("--workers") + 1], "1")

  const resume = ["mutate", "run", "--out", "/out"]
  assert.deepEqual(runNextCall({ ok: true, budgetHit: true, survived: 3 }, "/out", resume), { argv: resume })
  assert.deepEqual(runNextCall({ ok: true, budgetHit: false, survived: 2 }, "/same", resume)?.argv, ["mutate", "tally", "--out", "/same"])
  assert.equal(runNextCall({ ok: true, budgetHit: false, survived: 0 }, "/same", resume), null)
  assert.equal(runNextCall({ ok: false, budgetHit: true, survived: 4 }, "/same", resume), null)
})
