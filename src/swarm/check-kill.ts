import { existsSync, readFileSync } from "node:fs"
import path from "node:path"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"
import { killLabel } from "../mutate/kill-label.js"
import { normalizeResult, runMutants, type RunOptions, type RunReport } from "../mutate/run.js"
import { campaignSummary, UNVIABLE_NEXT } from "../mutate/suite-decision.js"

/** Done only when this mutant is killed after confirm and the full suite baseline is green. */
export async function checkKill(id: string, options: RunOptions): Promise<Envelope> {
  const patch = options.patchDirs.map((dir) => path.join(dir, `${id}.patch`)).find((file) => existsSync(file))
  if (!patch) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "check-kill",
      summary: `${id} is not done. The patch is missing.`,
      next: "The task is not done.",
      nextCall: null,
      done: false,
      outcome: null,
      suiteGreen: false,
      killedBy: [],
    }
  }
  // The baseline file is the discovered suite. Importers do not replace it. Confirm stays as passed in.
  const report = await runMutants({ ...options, onlyTests: null, onlyPatch: id, affected: false })
  const baselineFile = path.join(options.outDir, "baseline.json")
  let suiteGreen = false
  if (existsSync(baselineFile)) {
    const baseline = JSON.parse(readFileSync(baselineFile, "utf8")) as { ok?: boolean }
    suiteGreen = baseline.ok === true
  }
  // A red baseline stops inside the run. Do not read a result that names the broken test.
  if (!report.ok) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "check-kill",
      summary: `${id} is not done. ${report.summary}`,
      next: "The task is not done.",
      nextCall: null,
      done: false,
      outcome: null,
      suiteGreen,
      killedBy: [],
    }
  }
  const resultFile = path.join(options.outDir, "results", `${id}.json`)
  let outcome: string | null = null
  let killedBy: string[] = []
  if (existsSync(resultFile)) {
    const result = normalizeResult(JSON.parse(readFileSync(resultFile, "utf8")) as { outcome: string; cause?: string | null; killedBy?: string[] })
    outcome = result.outcome ?? null
    killedBy = Array.isArray(result.killedBy) ? result.killedBy : []
  }
  // A mutant that did not build is not a kill and not done.
  const unviableMutant = outcome === "unviable"
  const done = suiteGreen && outcome === "killed"
  const label = unviableMutant ? { cause: "build" as const, next: UNVIABLE_NEXT } : done ? killLabel(killedBy) : null
  const head = checkHead(report)
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: done,
    command: "check-kill",
    summary: done ? `${head} ${id} is dead and the suite is green.` : `${head} ${id} is not done.`,
    next: label?.next ?? (done ? "The gap-fixing task is done." : "The task is not done."),
    nextCall: null,
    done,
    outcome,
    cause: label?.cause ?? null,
    killedBy,
    suiteGreen,
  }
}

function checkHead(report: RunReport): string {
  const finished = report.killed + report.unviable + report.survived + report.flaky + report.timeouts + report.errors + report.noCoverage
  return campaignSummary({
    noCoverage: report.noCoverage,
    survived: report.survived,
    killed: report.killed,
    unviable: report.unviable,
    flaky: report.flaky,
    timeouts: report.timeouts,
    errors: report.errors,
    finished,
  })
}


