import { spawn } from "node:child_process"
import { createHash, randomBytes } from "node:crypto"
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { affectedTests } from "./affected.js"
import { binaryFromCommand, readCoverageMap, selectionFor, writeLlvmCoverage, type CoverageMap } from "./coverage-map.js"
import { writeGoCoverage } from "./go-coverage.js"
import { writeJavaCoverage } from "./java-coverage.js"
import { applyPatch, changedLines, chooseDirection, filesInDiff, git, restoreTree, type PatchDirection } from "./patch.js"
import { killLabel, type KillCause } from "./kill-label.js"
import {
  TIMEOUT_FLOOR_MS,
  TIMEOUT_MULTIPLE,
  TIMEOUT_NEXT,
  WHOLE_PROGRAM_TEST,
  WHOLE_SUITE_NEXT,
  campaignSummary,
  decideSuite,
  hasLineMap,
  noCoverageNext,
  resumeNext,
  scaleStop,
  timeoutLimitMs,
} from "./suite-decision.js"
import { commandSuite, discoverSuite, runDiscoveredSuite, sourceCandidates, type SuiteSpec } from "./suites.js"
import { duplicateTitles } from "./titles.js"

export const reporterPath = fileURLToPath(new URL("./fail-reporter.mjs", import.meta.url))

export type RunOptions = {
  packageDir: string
  repoDir: string
  commit: string
  patchDirs: string[]
  outDir: string
  direction: "auto" | PatchDirection
  workers: number
  concurrency: number
  maxMinutes: number | null
  maxMutants: number | null
  build: string[] | null
  testsDir: string
  unset: string[]
  suiteTimeoutMs: number
  testTimeoutMs: number
  confirm: boolean
  affected: boolean
  agent: string | null
  /** Package-relative test files. When set, the baseline and the mutants run only these. */
  onlyTests?: string[] | null
  /** When set, only the patch whose basename is this id is run. */
  onlyPatch?: string | null
  /** Shell command that runs the suite. Overrides discovery. */
  suiteCommand?: string | null
  /** Previous out dir or history.json. Unchanged mutants are not executed again. */
  historyPath?: string | null
  /** Mutant timeout is this multiple of the stored baseline duration. */
  timeoutMultiple?: number
  /** Positive floor for the mutant timeout, in milliseconds. */
  timeoutFloorMs?: number
  /** Stop after this many milliseconds of mutant work. Null uses maxMinutes. */
  budgetMs?: number | null
  onProgress?: (line: string) => void
}

export type MutantOutcome = "killed" | "survived" | "flaky" | "timeout" | "error" | "build-failed" | "no coverage"

export type MutantResult = {
  id: string
  outcome: MutantOutcome
  direction: PatchDirection | null
  files: Array<{ file: string; line: number }>
  killedBy: string[]
  /** Set only when outcome is killed. A build cause is not a test name. */
  cause: KillCause | null
  /** Next action for this mutant. Empty when it was not killed. */
  next: string
  flaky: string[]
  error?: string
  seconds: number
  commit: string
  agent: string | null
  runId: string
  command: string
  wholeSuite: boolean
  reused: boolean
  selectedTests: string[]
  mutantHash: string
  testHashes: Record<string, string>
}

export type KillFact = {
  id: string
  cause: KillCause
  killedBy: string[]
  next: string
}

type Failure = { name: string; file: string; line: number; fileTimeout?: boolean }

export type TestReport = {
  tests: number
  pass: number
  fail: number
  failed: Failure[]
  names: string[]
}

export type CommandFact = {
  id: string
  outcome: string
  command: string
  wholeSuite: boolean
  reused: boolean
  cause: KillCause | null
  next: string
}

export type RunReport = {
  ok: boolean
  summary: string
  next: string
  budgetHit: boolean
  commit: string
  runId: string
  killed: number
  survived: number
  flaky: number
  timeouts: number
  errors: number
  noCoverage: number
  notStarted: string[]
  suiteCommand: string
  baselineMs: number | null
  commands: CommandFact[]
  gaps: Array<{ id: string; file: string; line: number; op?: string }>
  rest: number
  flakyIds: string[]
  kills: KillFact[]
  full: string
}

export function parseReport(stdout: string): TestReport | null {
  const lines = stdout
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("{"))
  if (lines.length === 0) return null
  try {
    const parsed = JSON.parse(lines[lines.length - 1]) as TestReport
    if (!Array.isArray(parsed.failed) || !Array.isArray(parsed.names)) return null
    return parsed
  } catch {
    return null
  }
}

export async function runMutants(options: RunOptions): Promise<RunReport> {
  const repo = realpathSync(path.resolve(options.repoDir))
  const packageDir = realpathSync(path.resolve(options.packageDir))
  const sha = git(repo, ["rev-parse", "--verify", `${options.commit}^{commit}`])
  if (sha.status !== 0) return failReport(options.outDir, "", options.commit, "that commit does not resolve")
  const commit = sha.stdout.trim()
  mkdirSync(options.outDir, { recursive: true })
  const run = loadRun(options.outDir, commit, options.agent)
  if ("error" in run) return failReport(options.outDir, run.id, commit, run.error)
  const listed = listPatches(options.patchDirs)
  if ("error" in listed) return failReport(options.outDir, run.id, commit, listed.error)
  const patchFiles = options.onlyPatch
    ? listed.files.filter((file) => path.basename(file, ".patch") === options.onlyPatch)
    : listed.files
  if (patchFiles.length === 0) return failReport(options.outDir, run.id, commit, "no patches")
  const gate = resolveSuite(packageDir, options)
  if (gate.action === "stop") return failReport(options.outDir, run.id, commit, gate.summary, undefined, gate.next, gate.suiteCommand)
  const testsHere = options.onlyTests && options.onlyTests.length > 0 ? options.onlyTests : gate.spec.files
  if (testsHere.length === 0) return failReport(options.outDir, run.id, commit, `no tests in ${options.testsDir}`)
  const scope = options.maxMutants === null ? patchFiles : patchFiles.slice(0, options.maxMutants)
  const meta = loadMeta(options.patchDirs)
  const pending = scope.filter((file) => !existsSync(resultPath(options.outDir, path.basename(file, ".patch"))))
  const progress = (line: string) => options.onProgress?.(line)
  const history = loadHistory(options.historyPath)
  const worktrees = await createWorktrees(repo, packageDir, run.id, commit, options.workers, options.outDir)
  if ("error" in worktrees) return failReport(options.outDir, run.id, commit, worktrees.error)
  const budgetMs = options.budgetMs ?? (options.maxMinutes == null ? null : options.maxMinutes * 60_000)
  let budgetStarted = 0
  const overBudget = () => budgetMs !== null && budgetStarted !== 0 && Date.now() - budgetStarted > budgetMs
  try {
    const pkg = packageIn(worktrees.dirs[0], repo, packageDir)
    const prep = await prepare(pkg, options, progress)
    if (!prep.ok) return failReport(options.outDir, run.id, commit, prep.error, prep.baselineFailed, prep.next, prep.suiteCommand)
    // The baseline already ran. A slow suite with no line map does not start the batch.
    // A kind that can collect a map still refuses when that map was not written.
    const mapped = prep.coverage !== null && Object.keys(prep.coverage.files).length > 0 && hasLineMap(prep.spec.kind)
    const scale = scaleStop({ kind: mapped ? prep.spec.kind : "unmapped", baselineMs: prep.baselineMs, pending: pending.length })
    if (scale.action === "stop") {
      const ids = pending.map((file) => path.basename(file, ".patch"))
      return failReport(options.outDir, run.id, commit, scale.summary, undefined, scale.next, prep.spec.command, prep.baselineMs, ids)
    }
    let budgetHit = false
    let startedMutants = 0
    const queue = [...pending]
    budgetStarted = Date.now()
    const worker = async (worktree: string) => {
      const treePkg = packageIn(worktree, repo, packageDir)
      for (;;) {
        if (startedMutants > 0 && overBudget()) {
          budgetHit = true
          return
        }
        const patch = queue.shift()
        if (!patch) return
        if (startedMutants > 0 && overBudget()) {
          queue.unshift(patch)
          budgetHit = true
          return
        }
        const id = path.basename(patch, ".patch")
        const raw = readFileSync(patch, "utf8")
        const prior = history.get(id)
        if (prior && reusable(prior, raw, treePkg)) {
          startedMutants += 1
          const reused = reusedResult(prior, commit, run.id, options.agent)
          writeResult(options.outDir, reused)
          progress(`reused ${id}`)
          continue
        }
        startedMutants += 1
        progress(`${id} start`)
        const result = await runOne(worktree, treePkg, patch, options, commit, run.id, prep)
        writeResult(options.outDir, result)
        progress(`${id} ${result.outcome}`)
      }
    }
    await Promise.all(worktrees.dirs.map((dir) => worker(dir)))
    const results = scope.map((file) => readResult(options.outDir, path.basename(file, ".patch"))).filter((item): item is MutantResult => item !== null)
    const notStarted = scope
      .map((file) => path.basename(file, ".patch"))
      .filter((id) => !existsSync(resultPath(options.outDir, id)))
    writeHistory(options.outDir, results)
    return reportOf(results, {
      outDir: options.outDir,
      commit,
      runId: run.id,
      budgetHit,
      meta,
      pendingLeft: notStarted.length,
      notStarted,
      suiteCommand: prep.spec.command,
      baselineMs: prep.baselineMs,
    })
  } finally {
    removeWorktrees(repo, worktrees.dirs, options.outDir)
  }
}

type Prepared = {
  ok: true
  tests: string[]
  affectedMap: Record<string, string[]> | null
  baselineMs: number | null
  spec: SuiteSpec
  coverage: CoverageMap | null
}

async function prepare(
  pkg: string,
  options: RunOptions,
  progress: (line: string) => void,
): Promise<Prepared | { ok: false; error: string; baselineFailed?: string[]; next?: string; suiteCommand?: string }> {
  const gate = resolveSuite(pkg, options)
  if (gate.action === "stop") return { ok: false, error: gate.summary, next: gate.next, suiteCommand: gate.suiteCommand }
  const tests = options.onlyTests && options.onlyTests.length > 0 ? [...options.onlyTests].sort() : gate.spec.files
  if (tests.length === 0) return { ok: false, error: `no tests in ${options.testsDir}` }
  const nodeTests = tests.filter((file) => isNodeTestFile(file))
  const dupes = duplicateTitles(
    nodeTests.map((file) => ({ file, text: readFileSync(path.join(pkg, file), "utf8") })),
  )
  if (dupes.length > 0) {
    const first = dupes[0]
    return { ok: false, error: `duplicate test title "${first.title}" in ${first.files.join(", ")}` }
  }
  const spec = gate.spec
  progress(`suite ${spec.command}`)
  const baselineFile = path.join(options.outDir, "baseline.json")
  const mapPath = path.join(options.outDir, "coverage-map.json")
  const profileDir = path.join(options.outDir, "llvm-profile")
  const historyMap = historyCoverage(options.historyPath)
  if (historyMap && !existsSync(mapPath)) copyFileSync(historyMap, mapPath)
  let baselineFailed: string[] = []
  let baselineMs: number | null = null
  if (!existsSync(baselineFile)) {
    if (options.build) {
      progress(`build ${options.build.join(" ")}`)
      const built = await spawnCollected(options.build[0], options.build.slice(1), pkg, childEnv(options.unset), Math.min(options.suiteTimeoutMs, 120_000))
      if (built.timedOut || built.code !== 0) {
        return { ok: false, error: built.timedOut ? "baseline build timed out" : `baseline build failed (${clip(built.stderr || built.stdout)})` }
      }
    }
    progress("baseline start")
    const collect = !existsSync(mapPath)
    if (collect) mkdirSync(profileDir, { recursive: true })
    const startedAt = Date.now()
    const baseline = await runSuite(pkg, tests, options, null, null, spec, options.suiteTimeoutMs, false, collect ? mapPath : null, collect ? path.join(profileDir, "baseline-%p.profraw") : null)
    baselineMs = Date.now() - startedAt
    if (baseline.timedOut) return { ok: false, error: "baseline suite timed out" }
    if (baseline.compileToken) return { ok: false, error: `baseline build failed (${baseline.compileToken})` }
    if (!baseline.report) return { ok: false, error: baselineReportError(baseline.detail) }
    baselineFailed = baseline.report.failed.map((item) => item.name)
    if (collect && !existsSync(mapPath)) {
      const binary = binaryFromCommand(baseline.command || spec.command, pkg)
      const written = writeLlvmCoverage(pkg, profileDir, binary, mapPath)
      if (!written.ok && written.detail !== "no profraw" && written.detail !== "no profile directory" && written.detail !== "no instrumented binary") {
        writeFileSync(path.join(options.outDir, "coverage-error.txt"), `${written.detail}\n`)
      }
    }
    writeFileSync(
      baselineFile,
      `${JSON.stringify({ ok: baselineFailed.length === 0, tests: baseline.report.tests, failed: baselineFailed, command: spec.command, durationMs: baselineMs }, null, 2)}\n`,
    )
    progress(`suite report tests=${baseline.report.tests}`)
    progress(baselineFailed.length > 0 ? `baseline failing ${baselineFailed.length}` : "baseline pass")
    if (baselineFailed.length > 0) {
      return { ok: false, error: "baseline already failing", baselineFailed, suiteCommand: spec.command }
    }
    if (collect && !existsSync(mapPath) && (spec.kind === "go" || spec.kind === "maven")) {
      progress(spec.kind === "go" ? "go line map" : "java line map")
      const written = spec.kind === "go"
        ? writeGoCoverage(pkg, baseline.report.names, mapPath)
        : writeJavaCoverage(pkg, baseline.report.names, mapPath)
      if (!written.ok) writeFileSync(path.join(options.outDir, "coverage-error.txt"), `${written.detail}\n`)
    }
  } else {
    const stored = JSON.parse(readFileSync(baselineFile, "utf8")) as { ok?: boolean; failed?: unknown; durationMs?: number }
    baselineMs = typeof stored.durationMs === "number" ? stored.durationMs : null
    if (stored.ok !== true) {
      const failed = Array.isArray(stored.failed) ? stored.failed.filter((name): name is string => typeof name === "string") : []
      return { ok: false, error: "baseline already failing", baselineFailed: failed, suiteCommand: spec.command }
    }
  }
  const affectedMap = options.affected ? affectedTests(pkg, { testsDir: options.testsDir }) : null
  return { ok: true, tests, affectedMap, baselineMs, spec, coverage: readCoverageMap(mapPath) }
}

async function runOne(
  worktree: string,
  pkg: string,
  patchPath: string,
  options: RunOptions,
  commit: string,
  runId: string,
  prep: Prepared,
): Promise<MutantResult> {
  const id = path.basename(patchPath, ".patch")
  const started = Date.now()
  const raw = readFileSync(patchPath, "utf8")
  const files = filesInDiff(raw)
  const edited = changedLines(raw)
  const mutantHash = sha256(raw)
  const base = (): MutantResult => ({
    id,
    outcome: "error",
    direction: null,
    files,
    killedBy: [],
    cause: null,
    next: "",
    flaky: [],
    seconds: (Date.now() - started) / 1000,
    commit,
    agent: options.agent,
    runId,
    command: "",
    wholeSuite: false,
    reused: false,
    selectedTests: [],
    mutantHash,
    testHashes: {},
  })
  const direction = chooseDirection(worktree, raw, options.direction)
  if (!direction) return { ...base(), error: "patch does not apply forward or in reverse" }
  const applied = applyPatch(worktree, raw, direction)
  if (!applied.ok) return { ...base(), direction, error: applied.error }
  const multiple = options.timeoutMultiple ?? TIMEOUT_MULTIPLE
  const floor = options.timeoutFloorMs ?? TIMEOUT_FLOOR_MS
  const limit = prep.baselineMs == null ? options.suiteTimeoutMs : Math.min(options.suiteTimeoutMs, timeoutLimitMs(prep.baselineMs, multiple, floor))
  try {
    if (options.build) {
      const built = await spawnCollected(options.build[0], options.build.slice(1), pkg, childEnv(options.unset), Math.min(options.suiteTimeoutMs, 120_000))
      if (built.timedOut) return { ...base(), direction, outcome: "timeout", error: "build timed out", next: TIMEOUT_NEXT }
      // The baseline build already succeeded. A mutant that no longer builds cannot pass the suite.
      if (built.code !== 0) return finishMutant(killed(base, direction, ["build"]), pkg, "", false, [])
    }
    const picked = selectionFor(prep.coverage, edited.length > 0 ? edited : files)
    if (picked.state === "uncovered") {
      return {
        ...base(),
        direction,
        outcome: "no coverage",
        next: noCoverageNext(picked.file, picked.line),
      }
    }
    const names = picked.state === "covered" ? picked.tests.filter((name) => name !== WHOLE_PROGRAM_TEST) : null
    const forceWhole = picked.state === "covered" && (names === null || names.length === 0)
    // node --test loads every file it is given, so a name pattern still pays for the other files.
    const selected = narrowNodeFiles(pkg, selectTests(files, prep.tests, prep.affectedMap), names, prep.spec.kind)
    const commands: string[] = []
    const main = await runSuite(pkg, selected, options, null, names && names.length > 0 ? names : null, prep.spec, limit, forceWhole, null, null)
    if (main.command) commands.push(main.command)
    if (main.timedOut) {
      return finishMutant(
        { ...base(), direction, outcome: "timeout", error: "suite timed out", next: TIMEOUT_NEXT },
        pkg,
        commands.join(" | "),
        main.wholeSuite || forceWhole,
        main.selectedTests,
      )
    }
    // A compile error can leave the previous run's report on disk. That report is not this mutant.
    if (main.compileToken) return finishMutant(killed(base, direction, [main.compileToken]), pkg, commands.join(" | "), false, [])
    if (!main.report) return { ...base(), direction, error: "test reporter returned nothing", command: commands.join(" | ") }
    const whole = main.wholeSuite || forceWhole
    const chosen = main.selectedTests
    if (main.report.failed.length === 0) return finishMutant({ ...base(), direction, outcome: "survived" }, pkg, commands.join(" | "), whole, chosen)
    if (!options.confirm) return finishMutant(killed(base, direction, main.report.failed.map((item) => failureId(pkg, item))), pkg, commands.join(" | "), whole, chosen)
    const confirmed = await confirmFailures(pkg, options, main.report.failed, prep, limit, commands)
    const killedBy = confirmed.killed.map((item) => failureId(pkg, item))
    const flaky = confirmed.flaky.map((item) => failureId(pkg, item))
    const outcome: MutantOutcome = killedBy.length > 0 ? "killed" : "flaky"
    const label = killLabel(killedBy)
    return finishMutant(
      { ...base(), direction, outcome, killedBy, flaky, cause: label?.cause ?? null, next: label?.next ?? "" },
      pkg,
      commands.join(" | "),
      whole || confirmed.wholeSuite,
      chosen,
    )
  } finally {
    restoreTree(worktree)
  }
}

function finishMutant(result: MutantResult, pkg: string, command: string, wholeSuite: boolean, selectedTests: string[]): MutantResult {
  const next = wholeSuite && !result.next.includes("cannot take a test name")
    ? `${result.next}${result.next ? " " : ""}${WHOLE_SUITE_NEXT}`.trim()
    : result.next
  return {
    ...result,
    command,
    wholeSuite,
    selectedTests,
    next,
    testHashes: hashesFor(pkg, result.killedBy),
  }
}

function killed(base: () => MutantResult, direction: PatchDirection | null, killedBy: string[]): MutantResult {
  const label = killLabel(killedBy)
  return { ...base(), direction, outcome: "killed", killedBy, cause: label?.cause ?? "test", next: label?.next ?? "" }
}

async function confirmFailures(
  pkg: string,
  options: RunOptions,
  failed: Failure[],
  prep: Prepared,
  timeoutMs: number,
  commands: string[],
): Promise<{ killed: Failure[]; flaky: Failure[]; wholeSuite: boolean }> {
  const counts = new Map<string, number>()
  let wholeSuite = false
  const named = new Map<string, Failure[]>()
  const hungFiles = new Map<string, Failure[]>()
  for (const failure of failed) {
    const key = failure.file || "*"
    // A name filter skips the tests that left the handle open, so the file
    // timeout never comes back. Rerun that file with no name pattern.
    const bucket = failure.fileTimeout ? hungFiles : named
    const list = bucket.get(key) ?? []
    list.push(failure)
    bucket.set(key, list)
  }
  for (const [file, failures] of named) {
    const pattern = `^(${failures.map((item) => escapeRegExp(item.name)).join("|")})$`
    if (await rerunGroup(pkg, options, file, failures, counts, pattern, prep, timeoutMs, commands)) wholeSuite = true
  }
  for (const [file, failures] of hungFiles) {
    if (await rerunGroup(pkg, options, file, failures, counts, null, prep, timeoutMs, commands)) wholeSuite = true
  }
  const killed: Failure[] = []
  const flaky: Failure[] = []
  for (const failure of failed) (counts.get(failureId(pkg, failure)) === 2 ? killed : flaky).push(failure)
  return { killed, flaky, wholeSuite }
}

async function rerunGroup(
  pkg: string,
  options: RunOptions,
  file: string,
  failures: Failure[],
  counts: Map<string, number>,
  pattern: string | null,
  prep: Prepared,
  timeoutMs: number,
  commands: string[],
): Promise<boolean> {
  const files = file === "*" ? listTestFiles(pkg, options.testsDir, prep.spec) : [testArg(pkg, file, options.testsDir)]
  const isolated = { ...options, concurrency: 1 }
  let wholeSuite = false
  // Pytest confirms by node id. Maven confirms by class#method. A regex is not a test name for either.
  const directNames = prep.spec.kind === "pytest" || prep.spec.kind === "maven" ? failures.map((item) => item.name) : null
  for (let attempt = 0; attempt < 2; attempt++) {
    const rerun = await runSuite(pkg, files, isolated, directNames ? null : pattern, directNames, prep.spec, timeoutMs, false, null, null)
    if (rerun.wholeSuite) wholeSuite = true
    if (rerun.command) commands.push(rerun.command)
    if (pattern === null && rerun.timedOut) {
      for (const failure of failures) bump(counts, failureId(pkg, failure))
      continue
    }
    if (!rerun.report) continue
    for (const hit of rerun.report.failed) {
      if (pattern === null && !hit.fileTimeout) continue
      const id = failureId(pkg, hit)
      if (failures.some((item) => failureId(pkg, item) === id)) bump(counts, id)
    }
  }
  return wholeSuite
}

function bump(counts: Map<string, number>, id: string): void {
  counts.set(id, (counts.get(id) ?? 0) + 1)
}

async function runSuite(
  pkg: string,
  files: string[],
  options: RunOptions,
  pattern: string | null,
  names: string[] | null,
  spec: SuiteSpec,
  timeoutMs: number,
  forceWhole: boolean,
  coverageMap: string | null,
  profileFile: string | null,
): Promise<{ report: TestReport | null; timedOut: boolean; detail: string; compileToken: string | null; command: string; wholeSuite: boolean; selectedTests: string[] }> {
  const result = await runDiscoveredSuite(pkg, files, options.testsDir, pattern, timeoutMs, childEnv(options.unset), {
    reporterPath,
    testTimeoutMs: options.testTimeoutMs,
    concurrency: options.concurrency,
  }, {
    spec,
    names,
    coverageMap,
    profileFile,
  })
  return {
    report: result.report,
    timedOut: result.timedOut,
    detail: result.detail,
    compileToken: result.compileToken,
    command: result.command,
    wholeSuite: result.wholeSuite || forceWhole,
    selectedTests: result.selectedTests,
  }
}

function isNodeTestFile(file: string): boolean {
  const base = path.posix.basename(file)
  return /\.(test|spec)\.(ts|tsx|mts|js|mjs|cjs)$/.test(base) || /_test\.(ts|tsx|js|mjs)$/.test(base)
}

function narrowNodeFiles(pkg: string, files: string[], names: string[] | null, kind: string): string[] {
  if (kind !== "node" || !names || names.length === 0) return files
  const hit = files.filter((file) => {
    let text: string
    try {
      text = readFileSync(path.join(pkg, file), "utf8")
    } catch {
      return false
    }
    return names.some((name) => text.includes(`"${name}"`) || text.includes(`'${name}'`) || text.includes("`" + name + "`"))
  })
  return hit.length > 0 ? hit : files
}

function selectTests(files: Array<{ file: string }>, allTests: string[], affectedMap: Record<string, string[]> | null): string[] {
  if (!affectedMap) return allTests
  const selected = new Set<string>()
  for (const file of files) {
    const key = Object.keys(affectedMap).find((candidate) => file.file === candidate || file.file.endsWith(`/${candidate}`))
    if (!key) return allTests
    for (const test of affectedMap[key] ?? []) selected.add(test)
  }
  if (selected.size === 0) return allTests
  return [...selected].sort()
}

function childEnv(unset: string[]): NodeJS.ProcessEnv {
  const env = { ...process.env }
  // A suite launched from inside node:test inherits this and then refuses to run.
  for (const key of Object.keys(env)) {
    if (key === "NODE_TEST_CONTEXT" || key.startsWith("NODE_TEST_")) delete env[key]
  }
  for (const key of unset) delete env[key]
  return env
}

function spawnCollected(
  bin: string,
  args: string[],
  cwd: string,
  env: NodeJS.ProcessEnv,
  timeoutMs: number,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  return new Promise((resolve) => {
    const child = spawn(bin, args, { cwd, env, detached: true, stdio: ["ignore", "pipe", "pipe"] })
    let stdout = ""
    let stderr = ""
    let settled = false
    let timedOut = false
    const finish = (code: number) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      resolve({ code, stdout, stderr, timedOut })
    }
    const timer = setTimeout(() => {
      timedOut = true
      try {
        if (child.pid) process.kill(-child.pid, "SIGKILL")
      } catch {
        child.kill("SIGKILL")
      }
    }, timeoutMs)
    child.stdout?.on("data", (chunk) => {
      stdout += chunk
    })
    child.stderr?.on("data", (chunk) => {
      stderr += chunk
    })
    child.on("error", (error) => finishFromError(error))
    child.on("close", (code) => finish(code ?? 1))
    function finishFromError(error: Error) {
      stderr += error.message
      finish(1)
    }
  })
}

function listPatches(dirs: string[]): { files: string[] } | { error: string } {
  const files: string[] = []
  for (const dir of dirs) {
    if (!existsSync(dir)) return { error: `patch directory not found: ${dir}` }
    for (const name of readdirSync(dir)) if (name.endsWith(".patch")) files.push(path.join(dir, name))
  }
  files.sort((left, right) => path.basename(left).localeCompare(path.basename(right)))
  const names = files.map((file) => path.basename(file))
  const duplicate = names.find((name, index) => names.indexOf(name) !== index)
  if (duplicate) return { error: `two patches share the name ${duplicate}` }
  if (files.length === 0) return { error: "no patches" }
  return { files }
}

function loadMeta(dirs: string[]): Map<string, { file: string; line: number; op?: string }> {
  const meta = new Map<string, { file: string; line: number; op?: string }>()
  for (const dir of dirs) {
    const file = path.join(dir, "mutants.json")
    if (!existsSync(file)) continue
    const parsed = JSON.parse(readFileSync(file, "utf8")) as { mutants?: Array<{ id: string; file: string; line: number; op?: string }> }
    for (const mutant of parsed.mutants ?? []) meta.set(mutant.id, mutant)
  }
  return meta
}

function loadRun(outDir: string, commit: string, agent: string | null): { id: string } | { error: string; id: string } {
  const file = path.join(outDir, "run.json")
  if (existsSync(file)) {
    const stored = JSON.parse(readFileSync(file, "utf8")) as { id: string; commit: string }
    if (stored.commit !== commit) return { error: `out dir is for commit ${stored.commit.slice(0, 12)}`, id: stored.id }
    return { id: stored.id }
  }
  const id = randomBytes(4).toString("hex")
  writeFileSync(file, `${JSON.stringify({ id, commit, agent, started: new Date().toISOString() }, null, 2)}\n`)
  return { id }
}

async function createWorktrees(
  repo: string,
  packageDir: string,
  runId: string,
  commit: string,
  workers: number,
  outDir: string,
): Promise<{ dirs: string[] } | { error: string }> {
  removeWorktrees(repo, recordedWorktrees(outDir), outDir)
  const dirs: string[] = []
  for (let index = 0; index < workers; index++) {
    const dir = path.join(tmpdir(), `probatio-${runId}-${index}`)
    rmSync(dir, { recursive: true, force: true })
    const added = git(repo, ["worktree", "add", "--detach", dir, commit])
    if (added.status !== 0) {
      removeWorktrees(repo, dirs, outDir)
      return { error: (added.stderr || "worktree add failed").trim() }
    }
    dirs.push(dir)
  }
  writeFileSync(path.join(outDir, "worktrees.json"), `${JSON.stringify(dirs)}\n`)
  const linked = nodeModulesOf(repo, packageDir)
  for (const dir of dirs) {
    const dest = path.join(packageIn(dir, repo, packageDir), "node_modules")
    if (linked && !existsSync(dest)) symlinkSync(linked, dest, "dir")
  }
  return { dirs }
}

function removeWorktrees(repo: string, dirs: string[], outDir: string): void {
  for (const dir of dirs) {
    git(repo, ["worktree", "remove", "--force", dir])
    rmSync(dir, { recursive: true, force: true })
  }
  git(repo, ["worktree", "prune"])
  rmSync(path.join(outDir, "worktrees.json"), { force: true })
}

function recordedWorktrees(outDir: string): string[] {
  const file = path.join(outDir, "worktrees.json")
  if (!existsSync(file)) return []
  try {
    const parsed = JSON.parse(readFileSync(file, "utf8")) as unknown
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : []
  } catch {
    return []
  }
}

/** A fresh worktree has no installed dependencies. Use the main checkout's node_modules. */
function nodeModulesOf(repo: string, packageDir: string): string | null {
  const local = path.join(packageDir, "node_modules")
  if (existsSync(local)) return local
  const common = git(repo, ["rev-parse", "--path-format=absolute", "--git-common-dir"])
  if (common.status !== 0) return null
  const gitDir = common.stdout.trim()
  if (!gitDir.endsWith(`${path.sep}.git`) && !gitDir.endsWith("/.git")) return null
  const rel = path.relative(repo, packageDir)
  if (rel.startsWith("..")) return null
  const candidate = path.join(path.dirname(gitDir), rel, "node_modules")
  return existsSync(candidate) ? candidate : null
}

function packageIn(worktree: string, repo: string, packageDir: string): string {
  const rel = path.relative(repo, packageDir)
  return rel && !rel.startsWith("..") ? path.join(worktree, rel) : worktree
}

function listTestFiles(pkg: string, testsDir: string, spec?: SuiteSpec | null): string[] {
  if (spec && spec.files.length > 0 && spec.files[0] !== "suite-command") return spec.files
  return discoverSuite(pkg, testsDir)?.files ?? []
}

function resultPath(outDir: string, id: string): string {
  return path.join(outDir, "results", `${id}.json`)
}

function writeResult(outDir: string, result: MutantResult): void {
  const dest = resultPath(outDir, result.id)
  mkdirSync(path.dirname(dest), { recursive: true })
  const temp = `${dest}.${process.pid}.tmp`
  writeFileSync(temp, `${JSON.stringify(result, null, 2)}\n`)
  renameSync(temp, dest)
}

function readResult(outDir: string, id: string): MutantResult | null {
  const file = resultPath(outDir, id)
  if (!existsSync(file)) return null
  return JSON.parse(readFileSync(file, "utf8")) as MutantResult
}

function reportOf(
  results: MutantResult[],
  context: {
    outDir: string
    commit: string
    runId: string
    budgetHit: boolean
    meta: Map<string, { file: string; line: number; op?: string }>
    pendingLeft: number
    notStarted: string[]
    suiteCommand: string
    baselineMs: number | null
  },
): RunReport {
  const count = (outcome: MutantOutcome) => results.filter((result) => result.outcome === outcome).length
  const killed = count("killed")
  const survived = count("survived")
  const flaky = count("flaky")
  const timeouts = count("timeout")
  const noCoverage = count("no coverage")
  const errors = results.filter((result) => result.outcome === "error" || result.outcome === "build-failed").length
  const gapResults = results.filter((result) => result.outcome === "survived")
  const gaps = gapResults.slice(0, 10).map((result) => {
    const known = context.meta.get(result.id)
    const file = known?.file ?? result.files[0]?.file ?? ""
    const line = known?.line ?? result.files[0]?.line ?? 0
    return { id: result.id, file, line, ...(known?.op ? { op: known.op } : {}) }
  })
  const budgetHit = context.budgetHit || context.pendingLeft > 0
  const summary = campaignSummary({ noCoverage, survived, killed, flaky, timeouts, errors, finished: results.length })
  const notes: string[] = []
  const uncovered = results.find((result) => result.outcome === "no coverage")
  if (uncovered?.next) notes.push(uncovered.next)
  if (gapResults.length > 0) notes.push(`First gap is ${gaps[0].id} at ${gaps[0].file}:${gaps[0].line}. A survivor is not a pass.`)
  if (timeouts > 0) notes.push(TIMEOUT_NEXT)
  if (budgetHit) notes.push(resumeNext(context.notStarted))
  if (results.some((result) => result.wholeSuite)) notes.push(WHOLE_SUITE_NEXT)
  if (notes.length === 0) notes.push("No survivor in this batch.")
  return {
    ok: true,
    summary,
    next: notes.join(" "),
    budgetHit,
    commit: context.commit,
    runId: context.runId,
    killed,
    survived,
    flaky,
    timeouts,
    errors,
    noCoverage,
    notStarted: context.notStarted,
    suiteCommand: context.suiteCommand,
    baselineMs: context.baselineMs,
    commands: results.map((result) => ({
      id: result.id,
      outcome: result.outcome,
      command: result.command,
      wholeSuite: result.wholeSuite,
      reused: result.reused,
      cause: result.cause,
      next: result.next,
    })),
    gaps,
    rest: Math.max(gapResults.length - gaps.length, 0),
    flakyIds: results.filter((result) => result.outcome === "flaky").slice(0, 10).map((result) => result.id),
    kills: results
      .filter((result): result is MutantResult & { cause: KillCause } => result.outcome === "killed" && result.cause !== null)
      .map((result) => ({ id: result.id, cause: result.cause, killedBy: result.killedBy, next: result.next })),
    full: path.join(context.outDir, "results"),
  }
}

function failReport(
  outDir: string,
  runId: string,
  commit: string,
  error: string,
  baselineFailed?: string[],
  next?: string,
  suiteCommand = "",
  baselineMs: number | null = null,
  notStarted: string[] = [],
): RunReport {
  const summary = baselineFailed && baselineFailed.length > 0 ? `${error}: ${baselineFailed.join("; ")}` : error
  return {
    ok: false,
    summary,
    next: next ?? "Fix that and run the same command again.",
    budgetHit: false,
    commit,
    runId,
    killed: 0,
    survived: 0,
    flaky: 0,
    timeouts: 0,
    errors: 0,
    noCoverage: 0,
    notStarted,
    suiteCommand,
    baselineMs,
    commands: [],
    gaps: [],
    rest: 0,
    flakyIds: [],
    kills: [],
    full: path.join(outDir, "results"),
  }
}

type HistoryEntry = {
  id: string
  outcome: MutantOutcome
  direction: PatchDirection | null
  files: Array<{ file: string; line: number }>
  killedBy: string[]
  cause: KillCause | null
  next: string
  flaky: string[]
  error?: string
  command: string
  wholeSuite: boolean
  selectedTests: string[]
  mutantHash: string
  testHashes: Record<string, string>
}

function resolveSuite(pkg: string, options: RunOptions): { action: "run"; spec: SuiteSpec } | { action: "stop"; summary: string; next: string; suiteCommand: string } {
  const discovered = discoverSuite(pkg, options.testsDir)
  const makefilePath = path.join(pkg, "Makefile")
  const makefile = existsSync(makefilePath) ? readFileSync(makefilePath, "utf8") : null
  const decision = decideSuite({
    discovered: discovered ? { kind: discovered.kind, command: discovered.command } : null,
    suiteCommand: options.suiteCommand ?? null,
    candidates: sourceCandidates(pkg),
    makefile,
    artifactExists: (rel) => existsSync(path.resolve(pkg, rel)),
  })
  if (decision.action === "unknown") return { action: "stop", summary: decision.summary, next: decision.next, suiteCommand: "" }
  if (decision.action === "cannot-run") return { action: "stop", summary: decision.summary, next: decision.next, suiteCommand: decision.command }
  if (decision.action === "no-tests") {
    return { action: "stop", summary: `no tests in ${options.testsDir}`, next: "Fix that and run the same command again.", suiteCommand: "" }
  }
  const command = options.suiteCommand?.trim() ?? ""
  if (command) {
    const files = discovered && discovered.files.length > 0 ? discovered.files : sourceCandidates(pkg)
    return { action: "run", spec: commandSuite(command, files) }
  }
  if (!discovered) return { action: "stop", summary: `no tests in ${options.testsDir}`, next: "Fix that and run the same command again.", suiteCommand: "" }
  return { action: "run", spec: discovered }
}

function loadHistory(historyPath: string | null | undefined): Map<string, HistoryEntry> {
  const found = new Map<string, HistoryEntry>()
  if (!historyPath) return found
  const file = historyPath.endsWith(".json") ? historyPath : path.join(historyPath, "history.json")
  if (!existsSync(file)) return found
  try {
    const parsed = JSON.parse(readFileSync(file, "utf8")) as { entries?: HistoryEntry[] }
    for (const entry of parsed.entries ?? []) {
      if (entry && typeof entry.id === "string") found.set(entry.id, entry)
    }
  } catch {
    return found
  }
  return found
}

function historyCoverage(historyPath: string | null | undefined): string | null {
  if (!historyPath) return null
  const dir = historyPath.endsWith(".json") ? path.dirname(historyPath) : historyPath
  const file = path.join(dir, "coverage-map.json")
  return existsSync(file) ? file : null
}

function reusable(entry: HistoryEntry, raw: string, pkg: string): boolean {
  if (!entry.mutantHash || entry.mutantHash !== sha256(raw)) return false
  if (entry.outcome === "error" || entry.outcome === "build-failed") return false
  for (const [file, hash] of Object.entries(entry.testHashes ?? {})) {
    if (hashFile(pkg, file) !== hash) return false
  }
  return true
}

function reusedResult(entry: HistoryEntry, commit: string, runId: string, agent: string | null): MutantResult {
  return {
    id: entry.id,
    outcome: entry.outcome,
    direction: entry.direction,
    files: entry.files ?? [],
    killedBy: entry.killedBy ?? [],
    cause: entry.cause,
    next: entry.next ?? "",
    flaky: entry.flaky ?? [],
    error: entry.error,
    seconds: 0,
    commit,
    agent,
    runId,
    command: "",
    wholeSuite: entry.wholeSuite === true,
    reused: true,
    selectedTests: entry.selectedTests ?? [],
    mutantHash: entry.mutantHash,
    testHashes: entry.testHashes ?? {},
  }
}

function writeHistory(outDir: string, results: MutantResult[]): void {
  const entries: HistoryEntry[] = results.map((result) => ({
    id: result.id,
    outcome: result.outcome,
    direction: result.direction,
    files: result.files,
    killedBy: result.killedBy,
    cause: result.cause,
    next: result.next,
    flaky: result.flaky,
    error: result.error,
    command: result.command,
    wholeSuite: result.wholeSuite,
    selectedTests: result.selectedTests,
    mutantHash: result.mutantHash,
    testHashes: result.testHashes,
  }))
  writeFileSync(path.join(outDir, "history.json"), `${JSON.stringify({ entries }, null, 2)}\n`)
}

function hashesFor(pkg: string, killedBy: string[]): Record<string, string> {
  const out: Record<string, string> = {}
  for (const id of killedBy) {
    const mark = id.indexOf("::")
    if (mark <= 0) continue
    const file = id.slice(0, mark)
    const full = findTestFile(pkg, file)
    if (!full) continue
    const rel = path.relative(pkg, full).split(path.sep).join("/")
    const key = rel.startsWith("..") ? file : rel
    out[key] = sha256(readFileSync(full))
  }
  return out
}

function hashFile(pkg: string, rel: string): string | null {
  const full = findTestFile(pkg, rel)
  if (!full) return null
  return sha256(readFileSync(full))
}

function findTestFile(pkg: string, rel: string): string | null {
  const candidates = [path.isAbsolute(rel) ? rel : "", path.join(pkg, rel), path.join(pkg, "tests", path.basename(rel)), path.join(pkg, "test", path.basename(rel))].filter((file) => file.length > 0)
  return candidates.find((file) => existsSync(file)) ?? null
}

function sha256(text: string | Buffer): string {
  return createHash("sha256").update(text).digest("hex")
}

function failureId(pkg: string, failure: Failure): string {
  const rel = failure.file ? path.relative(pkg, failure.file) : ""
  const file = !rel || rel.startsWith("..") ? path.basename(failure.file || "") : rel.split(path.sep).join("/")
  return `${file}::${failure.name}`
}

function testArg(pkg: string, file: string, testsDir: string): string {
  const rel = path.relative(pkg, file)
  if (rel && !rel.startsWith("..")) return rel.split(path.sep).join("/")
  return path.join(testsDir, path.basename(file))
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

function baselineReportError(detail: string): string {
  const line = detail.trim()
  // The first line of a Python or Node crash is a stack fragment. The summary
  // states the limit and does not carry that fragment.
  if (!line || isStackFragment(line)) return "baseline suite did not return a test report"
  return `baseline suite did not return a test report (${line})`
}

function isStackFragment(line: string): boolean {
  return line.startsWith("Traceback") || /^\s+at \S/.test(line) || /File ".+", line \d+/.test(line)
}

function clip(text: string): string {
  const line = text.trim().split("\n").find((item) => item.trim()) ?? "build failed"
  return line.slice(0, 240)
}
