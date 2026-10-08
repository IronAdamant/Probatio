import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"
import { isBuildName } from "./kill-label.js"
import { normalizeResult, type MutantResult } from "./run.js"
import { leadSummary, TIMEOUT_NEXT, WHOLE_PROGRAM_TEST } from "./suite-decision.js"

export type TallyGap = {
  id: string
  file: string
  line: number
  outcome: "survived"
}

const NOT_A_TEST = new Set(["suite", "pytest", "unittest", "command", WHOLE_PROGRAM_TEST])

/**
 * Read a finished mutate run. Name tests that killed a mutant, tests that killed
 * nothing, and one gap per survivor. An unseen line is not a gap. This never deletes a file.
 */
export function tallyRun(outDir: string): Envelope {
  const resultsDir = path.join(outDir, "results")
  const fullPath = path.join(outDir, "tally.json")
  if (!existsSync(resultsDir)) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.tally",
      summary: "No mutant results in that directory.",
      next: "Run mutate run, then tally the same --out directory.",
      nextCall: null,
      keep: [],
      noKillsYet: [],
      gaps: [],
      rest: 0,
      pruning: { mode: "advisory", deletedTests: 0, advice: [] },
      full: fullPath,
    }
  }
  const results = readResults(resultsDir)
  if (results.length === 0) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.tally",
      summary: "No mutant results in that directory.",
      next: "Run mutate run, then tally the same --out directory.",
      nextCall: null,
      keep: [],
      noKillsYet: [],
      gaps: [],
      rest: 0,
      pruning: { mode: "advisory", deletedTests: 0, advice: [] },
      full: fullPath,
    }
  }
  const killCounts = new Map<string, number>()
  for (const result of results) {
    if (result.outcome !== "killed") continue
    for (const name of result.killedBy ?? []) {
      if (!isTestName(name)) continue
      killCounts.set(name, (killCounts.get(name) ?? 0) + 1)
    }
  }
  // One test can be spelled two ways: the kill names its file, the line map does not.
  // The first spelling seen wins, and kill spellings are added first.
  const seen: string[] = []
  const see = (name: string) => {
    if (!isTestName(name)) return
    if (seen.some((known) => namesMatch(known, name))) return
    seen.push(name)
  }
  for (const name of killCounts.keys()) see(name)
  for (const name of coverageNames(path.join(outDir, "coverage-map.json"))) see(name)
  for (const result of results) {
    for (const name of result.selectedTests ?? []) see(name)
  }
  const counts = new Map<string, number>()
  for (const name of seen) counts.set(name, countFor(name, killCounts))
  const keep = [...counts.entries()]
    .filter(([, killed]) => killed > 0)
    .map(([name]) => name)
    .sort()
  // One batch is not evidence for a deletion. These tests saw no mutant die in this batch, nothing more.
  const noKillsYet = [...counts.entries()]
    .filter(([, killed]) => killed === 0)
    .map(([name]) => name)
    .sort()
  const pruning = { mode: "advisory" as const, deletedTests: 0 as const, advice: [] as string[] }
  const noCoverage = results.filter((result) => result.outcome === "no coverage").length
  const survived = results.filter((result) => result.outcome === "survived").length
  const timeouts = results.filter((result) => result.outcome === "timeout").length
  const gaps = results
    .filter((result): result is MutantResult & { outcome: "survived" } => result.outcome === "survived")
    .map((result) => ({
      id: result.id,
      file: result.files?.[0]?.file ?? "",
      line: result.files?.[0]?.line ?? 0,
      outcome: "survived" as const,
    }))
    .sort((a, b) => a.file.localeCompare(b.file) || a.line - b.line || a.id.localeCompare(b.id))
  const shown = gaps.slice(0, 10)
  const summary = leadSummary(noCoverage, survived, "survived", `${keep.length} tests to keep, ${noKillsYet.length} tests saw no kill in this batch, ${gaps.length} gaps. No test file was deleted.`)
  const notes: string[] = []
  if (noCoverage > 0) notes.push(`${noCoverage} no coverage. That line was not seen. It is not a pass and not a gap.`)
  if (gaps.length > 0) {
    notes.push(`First gap is ${gaps[0].id} at ${gaps[0].file}:${gaps[0].line}. A survivor is not a pass. Add one test that fails on that mutant and passes on the code, commit it, then run nextCall.`)
  }
  if (timeouts > 0) notes.push(TIMEOUT_NEXT)
  if (notes.length === 0) notes.push("No survivor in this run. No test file was deleted.")
  const next = notes.join(" ")
  const complete: Envelope = {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "mutate.tally",
    summary,
    next,
    nextCall: nextArgv(outDir, keep, gaps.length > 0),
    keep,
    noKillsYet,
    gaps,
    rest: 0,
    pruning,
    full: fullPath,
  }
  mkdirSync(outDir, { recursive: true })
  writeFileSync(fullPath, `${JSON.stringify(complete, null, 2)}\n`)
  return { ...complete, gaps: shown, rest: gaps.length - shown.length }
}

/**
 * The command after this tally. A gap reruns the batch on HEAD once the new test is committed.
 * Otherwise the keep ids are rescored at the same commit. Either way the out dir is fresh:
 * the stored out dir would skip every finished mutant and echo the old result.
 */
function nextArgv(outDir: string, keep: string[], hasGaps: boolean): { argv: string[] } | null {
  if (!hasGaps && keep.length === 0) return null
  let stored: {
    package?: string
    patches?: string[]
    out?: string
    repo?: string
    commit?: string
    workers?: number
    confirm?: boolean
    testsDir?: string
    suiteTimeoutMs?: number
    testTimeoutMs?: number
    direction?: string
    suiteCommand?: string | null
  } = {}
  try {
    stored = JSON.parse(readFileSync(path.join(outDir, "run.json"), "utf8")) as typeof stored
  } catch {
    stored = {}
  }
  const base = stored.out || outDir
  const argv = ["mutate", "run"]
  if (stored.package) argv.push("--package", stored.package)
  for (const dir of stored.patches ?? []) argv.push("--patches", dir)
  argv.push("--out", freshDir(base, hasGaps ? "after-gaps" : "rescore"))
  if (stored.repo) argv.push("--repo", stored.repo)
  if (!hasGaps && stored.commit) argv.push("--commit", stored.commit)
  if (typeof stored.workers === "number") argv.push("--workers", String(stored.workers))
  if (stored.confirm === false) argv.push("--no-confirm")
  if (stored.testsDir && stored.testsDir !== "tests") argv.push("--tests-dir", stored.testsDir)
  if (typeof stored.suiteTimeoutMs === "number") argv.push("--suite-timeout-ms", String(stored.suiteTimeoutMs))
  if (typeof stored.testTimeoutMs === "number") argv.push("--test-timeout-ms", String(stored.testTimeoutMs))
  if (stored.direction && stored.direction !== "auto") argv.push("--direction", stored.direction)
  // --only-test refuses --suite-command, so a keep rescore uses discovery.
  if (stored.suiteCommand && hasGaps) argv.push("--suite-command", stored.suiteCommand)
  if (!hasGaps) argv.push(...keep.flatMap((id) => ["--only-test", id]))
  return { argv }
}

/** First sibling of the out dir that does not exist yet. The same tally prints the same path. */
function freshDir(base: string, label: string): string {
  for (let index = 1; ; index++) {
    const candidate = index === 1 ? `${base}.${label}` : `${base}.${label}-${index}`
    if (!existsSync(candidate)) return candidate
  }
}

function readResults(dir: string): MutantResult[] {
  const out: MutantResult[] = []
  for (const name of readdirSync(dir)) {
    if (!name.endsWith(".json")) continue
    try {
      const parsed = JSON.parse(readFileSync(path.join(dir, name), "utf8")) as MutantResult
      if (!parsed || typeof parsed.outcome !== "string") continue
      if (!parsed.id) parsed.id = name.slice(0, -".json".length)
      out.push(normalizeResult(parsed))
    } catch {
      continue
    }
  }
  return out
}

function coverageNames(file: string): string[] {
  if (!existsSync(file)) return []
  try {
    const parsed = JSON.parse(readFileSync(file, "utf8")) as { files?: Record<string, Record<string, string[]>> }
    const names = new Set<string>()
    for (const lines of Object.values(parsed.files ?? {})) {
      for (const tests of Object.values(lines ?? {})) {
        for (const name of tests ?? []) names.add(name)
      }
    }
    return [...names]
  } catch {
    return []
  }
}

function countFor(name: string, kills: Map<string, number>): number {
  let total = 0
  for (const [killed, count] of kills) {
    if (namesMatch(name, killed)) total += count
  }
  return total
}

function namesMatch(a: string, b: string): boolean {
  if (a === b) return true
  return a.endsWith(`::${b}`) || b.endsWith(`::${a}`)
}

function isTestName(name: string): boolean {
  if (!name || isBuildName(name)) return false
  const leaf = name.split("::").pop() ?? name
  return !NOT_A_TEST.has(leaf) && !NOT_A_TEST.has(name)
}
