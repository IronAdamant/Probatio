import { existsSync, readdirSync } from "node:fs"
import path from "node:path"

export type BudgetResume = {
  verb: "run" | "sealed"
  packageDir: string
  repoDir: string
  outDir: string
  patchDirs: string[]
  /** Resolved commit that was scored. Empty omits `--commit`. */
  commit: string
  suiteCommand: string | null
  build: string | null
  /** Package-relative. Absolute paths are not repeated: `--hide` refuses them. */
  hide: string[]
  suiteTimeoutMs: number
  testTimeoutMs: number
  timeoutMultiple: number | null
  budgetMs: number | null
  maxMinutes: number | null
  timeoutFloorMs: number | null
  /** When set, the resume stays inside the same cap. Null means the whole batch. */
  maxMutants: number | null
  testsDir: string
  workers: number
  confirm: boolean
  direction: string
  onlyTests: string[]
  affected: boolean
  unset: string[]
  history: string | null
  label: string | null
  fix: string | null
}

/** Argv for a budget resume. A sealed resume stays `mutate sealed` so every hide is kept. */
export function resumeArgv(input: BudgetResume): string[] {
  const argv = ["mutate", input.verb, "--package", input.packageDir, "--repo", input.repoDir, "--out", input.outDir]
  for (const dir of input.patchDirs) argv.push("--patches", dir)
  if (input.commit) argv.push("--commit", input.commit)
  if (input.suiteCommand) argv.push("--suite-command", input.suiteCommand)
  if (input.build) argv.push("--build", input.build)
  for (const rel of input.hide) {
    if (rel) argv.push("--hide", rel)
  }
  argv.push("--suite-timeout-ms", String(input.suiteTimeoutMs))
  argv.push("--test-timeout-ms", String(input.testTimeoutMs))
  if (input.timeoutMultiple !== null) argv.push("--timeout-multiple", String(input.timeoutMultiple))
  if (input.budgetMs !== null) argv.push("--budget-ms", String(input.budgetMs))
  if (input.maxMinutes !== null) argv.push("--max-minutes", String(input.maxMinutes))
  if (input.timeoutFloorMs !== null) argv.push("--timeout-floor-ms", String(input.timeoutFloorMs))
  if (input.maxMutants !== null) argv.push("--max-mutants", String(input.maxMutants))
  if (input.testsDir !== "tests") argv.push("--tests-dir", input.testsDir)
  argv.push("--workers", String(input.workers))
  if (!input.confirm) argv.push("--no-confirm")
  if (input.direction !== "auto") argv.push("--direction", input.direction)
  for (const id of input.onlyTests) argv.push("--only-test", id)
  if (input.affected) argv.push("--affected")
  for (const name of input.unset) argv.push("--unset", name)
  if (input.history) argv.push("--history", input.history)
  if (input.verb === "sealed" && input.label) argv.push("--label", input.label)
  if (input.verb === "sealed" && input.fix) argv.push("--fix", input.fix)
  return argv
}

export function tallyCall(outDir: string): string[] {
  return ["mutate", "tally", "--out", outDir]
}

/**
 * Next command after mutate run. A budget hit resumes, even when survivors were already written.
 * A finished run with a survivor tallies that same out dir. Anything else is done.
 */
export function runNextCall(
  report: { ok: boolean; budgetHit: boolean; survived: number },
  outDir: string,
  resume: string[],
): { argv: string[] } | null {
  if (!report.ok) return null
  if (report.budgetHit) return { argv: resume }
  if (report.survived > 0) return { argv: tallyCall(outDir) }
  return null
}

/** `<out>/runs`, or `runs-2`, `runs-3`, … when that directory already holds mutant results. */
export function freshGenerateOut(outDir: string): string {
  const first = path.join(outDir, "runs")
  if (!directoryHasMutantResults(first)) return first
  for (let index = 2; ; index++) {
    const candidate = path.join(outDir, `runs-${index}`)
    if (!directoryHasMutantResults(candidate)) return candidate
  }
}

function directoryHasMutantResults(dir: string): boolean {
  if (existsSync(path.join(dir, "run.json"))) return true
  const results = path.join(dir, "results")
  if (!existsSync(results)) return false
  try {
    return readdirSync(results, { withFileTypes: true }).some((entry) => entry.isFile() && entry.name.endsWith(".json"))
  } catch {
    return true
  }
}
