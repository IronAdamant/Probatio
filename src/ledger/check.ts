import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import path from "node:path"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"
import { goldenChanges } from "../golden/check.js"
import { normalizeResult, runMutants, type MutantResult } from "../mutate/run.js"
import type { LedgerEntry } from "./build.js"

/**
 * Every fixed bug in the ledger, run against the suite, compared with the outcome recorded for it.
 * "The suite must catch every ledger bug, forever" is checked here, not remembered.
 *
 * A bug that was caught and is not caught now is a regression, and the check fails. Any other change
 * (a survivor that is now caught, a patch that no longer applies) also fails until it is re-recorded
 * with `--update` and a `Golden-Change: <id>: <why>` line in `--message`, the same guard golden check uses.
 */

export type LedgerCheckOptions = {
  packageDir: string
  repoDir: string
  commit: string
  ledgerDir: string
  outDir: string
  update: boolean
  message: string
  concurrency: number
  suiteTimeoutMs: number
  testTimeoutMs: number
  onProgress?: (line: string) => void
}

type Row = { ok: true; outcome: string; reason: string; next: string }

/** The contract is the outcome. The reason follows from it, so a renamed test does not fail the check. */
const REASON: Record<string, string> = {
  killed: "A test the suite already had caught this ledger bug.",
  survived: "The suite ran the line and did not catch this ledger bug.",
  "no coverage": "No test runs the line this ledger bug changes.",
  unviable: "This ledger bug does not build as it stands. A hand-made mutant that keeps the API would guard it.",
  flaky: "A test failed once and passed on the rerun. That is not a catch.",
  timeout: "The run passed the baseline clock. That is not a catch.",
  error: "The ledger patch did not run. It may no longer apply: rebuild the ledger or refresh the hand-made mutant.",
  missing: "No result was written for this ledger bug.",
}

export async function checkLedger(options: LedgerCheckOptions): Promise<Envelope> {
  const fail = (summary: string, next = "Fix that and run the same command again."): Envelope => ({
    schemaVersion: SCHEMA_VERSION,
    ok: false,
    command: "ledger.check",
    summary,
    next,
    nextCall: null,
  })
  const ledgerFile = path.join(options.ledgerDir, "ledger.json")
  if (!existsSync(ledgerFile)) return fail(`No ledger.json in ${options.ledgerDir}.`, "Run probatio ledger build first.")
  const ledger = JSON.parse(readFileSync(ledgerFile, "utf8")) as { entries?: LedgerEntry[] }
  const guarded = (ledger.entries ?? []).filter((item) => (item.status === "clean" || item.status === "handmade") && item.patch)
  if (guarded.length === 0) return fail("The ledger has no bug that applies.", "Rebuild the ledger, or write a hand-made mutant for a fix.")
  const outDir = path.resolve(options.outDir)
  rmSync(outDir, { recursive: true, force: true })
  const patchDir = path.join(outDir, "patches")
  mkdirSync(patchDir, { recursive: true })
  for (const item of guarded) {
    const source = path.join(options.ledgerDir, item.patch as string)
    if (!existsSync(source)) return fail(`${item.patch} is listed in ledger.json and is not in ${options.ledgerDir}.`)
    copyFileSync(source, path.join(patchDir, item.patch as string))
  }
  const report = await runMutants({
    packageDir: options.packageDir,
    repoDir: options.repoDir,
    commit: options.commit,
    patchDirs: [patchDir],
    outDir: path.join(outDir, "run"),
    direction: "forward",
    workers: 1,
    concurrency: options.concurrency,
    maxMinutes: null,
    maxMutants: null,
    build: null,
    testsDir: "tests",
    unset: [],
    suiteTimeoutMs: options.suiteTimeoutMs,
    testTimeoutMs: options.testTimeoutMs,
    confirm: true,
    affected: false,
    agent: null,
    onProgress: options.onProgress,
  })
  if (!report.ok) return fail(`The ledger run did not score: ${report.summary}`, report.next)
  const allowed = goldenChanges(options.message)
  const rows: Array<{ id: string; recorded: string | null; actual: string; next: string }> = []
  const regressed: string[] = []
  const changed: string[] = []
  const unrecorded: string[] = []
  const unauthorized: string[] = []
  for (const item of guarded) {
    const patchId = path.basename(item.patch as string, ".patch")
    const resultFile = path.join(outDir, "run", "results", `${patchId}.json`)
    const result = existsSync(resultFile) ? normalizeResult(JSON.parse(readFileSync(resultFile, "utf8")) as MutantResult) : null
    const outcome = result?.outcome ?? "missing"
    const next = outcome === "killed" ? `Killed by ${(result?.killedBy ?? []).join(", ")}.` : result?.next || result?.error || REASON[outcome] || outcome
    const actual: Row = { ok: true, outcome, reason: REASON[outcome] ?? outcome, next }
    const goldenFile = path.join(options.ledgerDir, `${item.id}.golden.json`)
    const recorded = existsSync(goldenFile)
      ? ((JSON.parse(readFileSync(goldenFile, "utf8")) as { rows?: Record<string, { outcome?: string }> }).rows?.[item.id]?.outcome ?? null)
      : null
    rows.push({ id: item.id, recorded, actual: outcome, next })
    if (recorded === null) unrecorded.push(item.id)
    else if (recorded === "killed" && outcome !== "killed") regressed.push(item.id)
    else if (recorded !== outcome) changed.push(item.id)
    const differs = recorded !== outcome
    if (differs && recorded !== null && !allowed.has(item.id)) unauthorized.push(item.id)
    if (options.update && (!differs || recorded === null || allowed.has(item.id))) {
      writeFileSync(goldenFile, `${JSON.stringify({ rows: { [item.id]: actual } }, null, 2)}\n`)
    }
  }
  const killed = rows.filter((row) => row.actual === "killed").length
  const written = options.update ? rows.filter((row) => !unauthorized.includes(row.id)).length : 0
  const ok = options.update ? unauthorized.length === 0 : regressed.length === 0 && changed.length === 0 && unrecorded.length === 0
  const parts = [`${killed} of ${rows.length} ledger bugs caught.`]
  if (regressed.length > 0) parts.push(`Regressed: ${regressed.join(", ")}.`)
  if (changed.length > 0) parts.push(`Changed: ${changed.join(", ")}.`)
  if (unrecorded.length > 0) parts.push(`Not recorded yet: ${unrecorded.join(", ")}.`)
  if (options.update) parts.push(`${written} golden row(s) written.`)
  let next = "Every ledger bug has the outcome recorded for it."
  if (regressed.length > 0) next = `A fixed bug is back in reach of the suite: ${regressed[0]} was caught and is not now. Find the test that stopped guarding it.`
  else if (unauthorized.length > 0 && options.update) next = `Not written: ${unauthorized.join(", ")} changed outcome. Add Golden-Change: <id>: <why> to --message for each.`
  else if (changed.length > 0 || unrecorded.length > 0) next = "Review the changed outcomes, then re-record with --update and a Golden-Change line for each changed id."
  return {
    schemaVersion: SCHEMA_VERSION,
    ok,
    command: "ledger.check",
    summary: parts.join(" "),
    next,
    nextCall: null,
    caught: killed,
    total: rows.length,
    regressed,
    changed,
    unrecorded,
    rows,
    run: { summary: report.summary, commit: report.commit, full: report.full },
  }
}
