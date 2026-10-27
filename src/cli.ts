#!/usr/bin/env node
import path from "node:path"
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { SCHEMA_VERSION, render, type Envelope } from "./contract.js"
import { bool, int, parseArgs, requireText, text, texts, unknownFlags, type FlagValue } from "./flags.js"
import { asTable, checkGolden, goldenShape, readGolden, writeGolden } from "./golden/check.js"
import { compareGoldens, recordGoldens } from "./golden/record.js"
import { buildLedger } from "./ledger/build.js"
import { checkLedger } from "./ledger/check.js"
import { readKills, reportMatrix } from "./matrix/report.js"
import {
  appendFinding,
  chooseSealed,
  fixGap,
  guardAllows,
  lineHash,
  loadState,
  parseGuard,
  readSourceLine,
  revertGap,
  saveState,
  sealIds,
  statusEnvelope,
  type FindingStatus,
  type KillScore,
} from "./memory/state.js"
import { git } from "./mutate/patch.js"
import { generateMutants } from "./mutate/generate.js"
import type { OperatorSet } from "./mutate/operators.js"
import { runMutants } from "./mutate/run.js"
import { fixEditedKillers, judgeSealedCatch, LABEL_LEAK_SUMMARY, labelLeaked, readSealedLabel, sealedTreeLeaked } from "./mutate/sealed.js"
import { generateNext } from "./mutate/suite-decision.js"
import { discoverSuite } from "./mutate/suites.js"
import { tallyRun } from "./mutate/tally.js"
import { freshGenerateOut, resumeArgv, runNextCall, type BudgetResume } from "./resume-call.js"
import { checkKill } from "./swarm/check-kill.js"
import { claimItem, reapClaims, seedQueue } from "./swarm/queue.js"
import { verifyChange } from "./verify/change.js"
import { nodeTooOld } from "./node-version.js"

const tooOld = nodeTooOld(process.versions.node)
if (tooOld) {
  process.stdout.write(
    `${JSON.stringify({
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.help",
      summary: tooOld,
      next: "Install Node 22.6 or newer and run the command again.",
      nextCall: null,
    })}\n`,
  )
  process.exit(1)
}

const parsed = parseArgs(process.argv.slice(2))
const [group, action] = parsed.command

const GENERATE_FLAGS = ["package", "out", "per-file", "src", "skip", "seed", "max-mutants", "max-minutes", "skip-file", "commit", "working-tree", "operators", "tests-dir"]
const RUN_FLAGS = ["hide", "only-test", "suite-command", "package", "patches", "out", "repo", "workers", "direction", "concurrency", "max-minutes", "max-mutants", "build", "tests-dir", "unset", "suite-timeout-ms", "test-timeout-ms", "confirm", "affected", "agent", "history", "timeout-multiple", "timeout-floor-ms", "budget-ms", "commit"]
const SEALED_FLAGS = [...RUN_FLAGS, "label", "fix"]
const TALLY_FLAGS = ["out"]
const LEDGER_BUILD_FLAGS = ["package", "out", "repo", "commit", "src", "tests-dir", "max-lines", "max-commits"]
const LEDGER_CHECK_FLAGS = ["package", "repo", "commit", "ledger", "out", "update", "message", "concurrency", "suite-timeout-ms", "test-timeout-ms"]
const MATRIX_FLAGS = ["scores", "direct", "out", "kills", "patches"]
const GOLDEN_CHECK_FLAGS = ["recorded", "update", "actual", "message", "root"]
const GOLDEN_RECORD_FLAGS = ["package", "repo", "commit", "module", "tests", "dir", "suite-timeout-ms"]
const GOLDEN_COMPARE_FLAGS = ["package", "repo", "commit", "module", "tests", "golden", "out", "operators", "suite-timeout-ms"]
const GAP_FIX_FLAGS = ["state", "line", "id", "file", "guard", "commit", "root"]
const GAP_REVERT_FLAGS = ["state", "id"]
const GUARD_FLAGS = ["state", "guard", "message", "root"]
const FINDINGS_FLAGS = ["state", "status", "id", "reason", "agent", "file", "line", "root"]
const STATUS_FLAGS = ["state", "root", "page", "scores", "agent"]
const SEAL_FLAGS = ["state", "from", "id", "fraction", "seed"]
const QUEUE_SEED_FLAGS = ["state", "id"]
const QUEUE_CLAIM_FLAGS = ["state", "now", "lease-ms", "id", "agent"]
const QUEUE_REAP_FLAGS = ["state", "now"]
const CHECK_KILL_FLAGS = ["package", "patches", "repo", "commit", "out", "max-minutes", "build", "tests-dir", "suite-timeout-ms", "test-timeout-ms", "confirm", "agent"]
const VERIFY_FLAGS = ["package", "operators", "repo", "out", "base", "commit", "max-mutants", "max-minutes", "max-tests"]

if (group === "mcp" && !action) {
  await import("./mcp.js")
} else try {
  if (group === "mutate" && action === "generate") finish(refuse("mutate.generate", GENERATE_FLAGS) ?? await generateCommand(parsed.flags), parsed.human)
  if (group === "mutate" && action === "run") finish(refuse("mutate.run", RUN_FLAGS) ?? await runCommand(parsed.flags), parsed.human)
  if (group === "mutate" && action === "sealed") finish(refuse("mutate.sealed", SEALED_FLAGS) ?? await sealedCommand(parsed.flags), parsed.human)
  if (group === "mutate" && action === "tally") finish(refuse("mutate.tally", TALLY_FLAGS) ?? tallyCommand(parsed.flags), parsed.human)
  if (group === "ledger" && action === "build") finish(refuse("ledger.build", LEDGER_BUILD_FLAGS) ?? await ledgerCommand(parsed.flags), parsed.human)
  if (group === "ledger" && action === "check") finish(refuse("ledger.check", LEDGER_CHECK_FLAGS) ?? await ledgerCheckCommand(parsed.flags), parsed.human)
  if (group === "matrix" && action === "report") finish(refuse("matrix.report", MATRIX_FLAGS) ?? matrixCommand(parsed.flags), parsed.human)
  if (group === "golden" && action === "check") finish(refuse("golden.check", GOLDEN_CHECK_FLAGS) ?? goldenCommand(parsed.flags), parsed.human)
  if (group === "golden" && action === "record") finish(refuse("golden.record", GOLDEN_RECORD_FLAGS) ?? await goldenRecordCommand(parsed.flags), parsed.human)
  if (group === "golden" && action === "compare") finish(refuse("golden.compare", GOLDEN_COMPARE_FLAGS) ?? await goldenCompareCommand(parsed.flags), parsed.human)
  if (group === "gap" && action === "fix") finish(refuse("gap.fix", GAP_FIX_FLAGS) ?? gapFixCommand(parsed.flags), parsed.human)
  if (group === "gap" && action === "revert") finish(refuse("gap.revert", GAP_REVERT_FLAGS) ?? gapRevertCommand(parsed.flags), parsed.human)
  if (group === "guard" && action === "check") finish(refuse("guard.check", GUARD_FLAGS) ?? guardCommand(parsed.flags), parsed.human)
  if (group === "findings" && action === "add") finish(refuse("findings.add", FINDINGS_FLAGS) ?? findingsCommand(parsed.flags), parsed.human)
  if (group === "status") finish(refuse("status", STATUS_FLAGS) ?? statusCommand(parsed.flags), parsed.human)
  if (group === "seal") finish(refuse("seal", SEAL_FLAGS) ?? sealCommand(parsed.flags), parsed.human)
  if (group === "queue" && action === "seed") finish(refuse("queue.seed", QUEUE_SEED_FLAGS) ?? queueSeedCommand(parsed.flags), parsed.human)
  if (group === "queue" && action === "claim") finish(refuse("queue.claim", QUEUE_CLAIM_FLAGS) ?? queueClaimCommand(parsed.flags), parsed.human)
  if (group === "queue" && action === "reap") finish(refuse("queue.reap", QUEUE_REAP_FLAGS) ?? queueReapCommand(parsed.flags), parsed.human)
  if (group === "check-kill") finish(refuse("check-kill", CHECK_KILL_FLAGS) ?? await checkKillCommand(parsed.flags, action), parsed.human)
  if (group === "verify-change") finish(refuse("verify-change", VERIFY_FLAGS) ?? await verifyCommand(parsed.flags), parsed.human)
  if (group === "schema") finish(refuse("schema", []) ?? schemaCommand(action), parsed.human)
  finish(usage(false, "Use mutate, ledger, matrix, golden, gap, guard, findings, status, queue, check-kill, verify-change, or schema."), parsed.human)
} catch (error) {
  const message = error instanceof Error ? error.message : String(error)
  finish(
    {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: `${group ?? "mutate"}.${action ?? "help"}`,
      summary: message,
      next: "Fix the command and run it again.",
      nextCall: null,
    },
    parsed.human,
  )
}

function usage(ok: boolean, summary: string): Envelope {
  return {
    schemaVersion: SCHEMA_VERSION,
    ok,
    command: "mutate.help",
    summary,
    next: "probatio mutate generate writes operator mutants. probatio mutate run confirms each kill twice. probatio mutate sealed runs one forward bug and keeps the label file out of the report. probatio mutate tally reads a finished run and does not delete a test. probatio ledger build reverts fix commits. probatio matrix report reads a recorded score table.",
    nextCall: null,
  }
}

/** Unknown flags stop the command before any work. `--no-confirm` is stored as `confirm`. */
function refuse(command: string, known: readonly string[]): Envelope | null {
  const unknown = unknownFlags(parsed.flags, known)
  if (unknown.length === 0) return null
  const named = unknown.map((name) => `--${name}`).join(", ")
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: false,
    command,
    summary: unknown.length === 1 ? `Unknown flag ${named}.` : `Unknown flags ${named}.`,
    next: "Drop the unknown flag and run the command again.",
    nextCall: null,
  }
}

async function generateCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
  const packageDir = path.resolve(requireText(flags, "package"))
  const outDir = path.resolve(requireText(flags, "out"))
  const perFile = int(flags, "per-file")
  const result = generateMutants({
    packageDir,
    srcDir: text(flags, "src") ?? "src",
    outDir,
    perFile: perFile === undefined ? null : perFile,
    skip: int(flags, "skip") ?? 0,
    seed: int(flags, "seed") ?? 20261003,
    maxMutants: int(flags, "max-mutants") ?? null,
    maxMinutes: int(flags, "max-minutes") ?? null,
    skipFiles: texts(flags, "skip-file"),
    commit: text(flags, "commit") ?? "HEAD",
    workingTree: bool(flags, "working-tree", false),
    operators: operatorSet(flags),
  })
  if (result.error) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.generate",
      summary: result.error,
      next: "Commit the file, or pass --working-tree to mutate the checkout on disk. mutate run scores the commit.",
      nextCall: null,
    }
  }
  if (result.violations.length > 0) {
    const first = result.violations[0]
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.generate",
      summary: `Refusing to write mutants: ${first.file}:${first.line} sits inside a string or a comment.`,
      next: "That is a generator bug. Do not use the batch.",
      nextCall: null,
      stringLiteralMutants: result.violations.length,
    }
  }
  const shown = result.mutants.slice(0, 10).map(({ id, file, line, op }) => ({ id, file, line, op }))
  const patches = path.join(outDir, "mutants")
  const stopped = result.budgetHit
  const givenCommit = text(flags, "commit")
  const givenTests = text(flags, "tests-dir")
  const suite = discoverSuite(packageDir, givenTests ?? "tests")
  const followUp = ["mutate", "run", "--package", packageDir, "--patches", patches, "--out", freshGenerateOut(outDir)]
  if (givenCommit) followUp.push("--commit", givenCommit)
  if (givenTests) followUp.push("--tests-dir", givenTests)
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "mutate.generate",
    summary: stopped
      ? `Stopped by the time budget after ${result.filesVisited} files. ${result.mutants.length} mutants written.`
      : `${result.mutants.length} mutants in ${result.filesVisited} files. No string-literal mutant.`,
    next: [
      stopped
        ? "Narrow --src or raise --max-minutes, then run the same command."
        : generateNext(result.mutants.length, suite ? { kind: suite.kind, command: suite.command } : null),
      result.source === "working-tree"
        ? "These patches match the working tree. mutate run scores the commit, so commit the files before mutate run."
        : "",
    ].filter((line) => line.length > 0).join(" "),
    nextCall: result.mutants.length === 0 ? null : { argv: followUp },
    mutantCount: result.mutants.length,
    operators: operatorSet(flags),
    stringLiteralMutants: 0,
    filesVisited: result.filesVisited,
    budgetHit: result.budgetHit,
    mutants: shown,
    rest: Math.max(result.mutants.length - shown.length, 0),
    full: path.join(patches, "mutants.json"),
  }
}

async function runCommand(flags: ReturnType<typeof parseArgs>["flags"], hide: string[] | null = null): Promise<Envelope> {
  const hidden = hide ?? texts(flags, "hide")
  const escaped = hidden.find((rel) => rel.split(/[\\/]/).includes(".."))
  if (escaped) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.run",
      summary: `--hide stays inside the package. Refused ${escaped}.`,
      next: "Pass a relative path with no .. segment.",
      nextCall: null,
      killed: 0,
    }
  }
  const onlyNames = texts(flags, "only-test")
  if (onlyNames.some((id) => id === "::command" || id.endsWith("::command"))) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.run",
      summary: "::command is not a test id.",
      next: "Pass a test name the suite can collect.",
      nextCall: null,
      killed: 0,
    }
  }
  if (onlyNames.length > 0 && text(flags, "suite-command")) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.run",
      summary: "--only-test cannot be combined with --suite-command.",
      next: "Drop --suite-command and pass the keep ids with --only-test.",
      nextCall: null,
      killed: 0,
    }
  }
  const packageDir = path.resolve(requireText(flags, "package"))
  const patchDirs = texts(flags, "patches").map((dir) => path.resolve(dir))
  if (patchDirs.length === 0) throw new Error("--patches is required")
  const outDir = path.resolve(requireText(flags, "out"))
  const repoDir = path.resolve(text(flags, "repo") ?? gitRoot(packageDir))
  const workers = int(flags, "workers") ?? 1
  if (workers < 1) throw new Error("--workers must be at least 1")
  const direction = text(flags, "direction") ?? "auto"
  if (direction !== "auto" && direction !== "forward" && direction !== "reverse") {
    throw new Error("--direction must be auto, forward, or reverse")
  }
  const build = buildCommand(flags)
  const report = await runMutants({
    packageDir,
    repoDir,
    commit: text(flags, "commit") ?? "HEAD",
    patchDirs,
    outDir,
    direction,
    workers,
    concurrency: int(flags, "concurrency") ?? 3,
    maxMinutes: int(flags, "max-minutes") ?? null,
    maxMutants: int(flags, "max-mutants") ?? null,
    build,
    testsDir: text(flags, "tests-dir") ?? "tests",
    unset: texts(flags, "unset"),
    suiteTimeoutMs: int(flags, "suite-timeout-ms") ?? 600_000,
    testTimeoutMs: int(flags, "test-timeout-ms") ?? 60_000,
    confirm: bool(flags, "confirm", true),
    affected: bool(flags, "affected", false),
    agent: text(flags, "agent") ?? null,
    suiteCommand: text(flags, "suite-command") ?? null,
    onlyNames: onlyNames.length > 0 ? onlyNames : null,
    historyPath: text(flags, "history") ? path.resolve(text(flags, "history") as string) : null,
    timeoutMultiple: int(flags, "timeout-multiple") ?? 5,
    timeoutFloorMs: int(flags, "timeout-floor-ms") ?? 20_000,
    budgetMs: int(flags, "budget-ms") ?? null,
    hide: hidden,
    onProgress: (line) => process.stderr.write(`${line}\n`),
  })
  const resume = resumeArgv(resumeInput(flags, "run", {
    packageDir,
    repoDir,
    outDir,
    patchDirs,
    commit: report.commit,
    hide: hidden,
  }))
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: report.ok,
    command: "mutate.run",
    summary: report.summary,
    next: report.next,
    nextCall: runNextCall(report, outDir, resume),
    killed: report.killed,
    unviable: report.unviable,
    unviableIds: report.unviableIds,
    survived: report.survived,
    flaky: report.flaky,
    timeouts: report.timeouts,
    errors: report.errors,
    noCoverage: report.noCoverage,
    notStarted: report.notStarted,
    suiteCommand: report.suiteCommand,
    baselineMs: report.baselineMs,
    commands: report.commands,
    gaps: report.gaps,
    rest: report.rest,
    flakyIds: report.flakyIds,
    budgetHit: report.budgetHit,
    commit: report.commit,
    runId: report.runId,
    kills: report.kills,
    full: report.full,
  }
}

function tallyCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  return tallyRun(path.resolve(requireText(flags, "out")))
}

async function sealedCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
  const labelPath = path.resolve(requireText(flags, "label"))
  const label = readSealedLabel(labelPath)
  const hide = texts(flags, "hide")
  const outDir = path.resolve(requireText(flags, "out"))
  const report = await runCommand(flags, hide)
  const packageDir = path.resolve(requireText(flags, "package"))
  const repoDir = path.resolve(text(flags, "repo") ?? gitRoot(packageDir))
  const kills = Array.isArray(report.kills) ? (report.kills as Array<{ killedBy: string[] }>) : []
  // The fix commit is the scored commit unless --fix names an earlier one (new tests landed on top of it).
  const scored = typeof report.commit === "string" ? report.commit : ""
  const fixRef = text(flags, "fix")
  const fixSha = fixRef ? git(repoDir, ["rev-parse", "--verify", `${fixRef}^{commit}`]) : null
  if (fixSha && fixSha.status !== 0) throw new Error(`--fix ${fixRef} does not resolve`)
  const commit = fixSha ? fixSha.stdout.trim() : scored
  const edited = report.ok && commit ? fixEditedKillers(repoDir, packageDir, commit, hide, kills) : { files: [], olderTestCatch: false }
  const notStarted = Array.isArray(report.notStarted) ? report.notStarted.filter((id): id is string => typeof id === "string") : []
  const judged: Envelope = {
    ...report,
    ...judgeSealedCatch(
      {
        ok: report.ok,
        killed: typeof report.killed === "number" ? report.killed : 0,
        summary: report.summary,
        next: report.next,
        nextCall: report.nextCall,
      },
      kills,
      edited,
      notStarted,
    ),
  }
  const body = JSON.stringify(judged)
  if (labelLeaked(body, label) || sealedTreeLeaked(outDir, label)) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "mutate.sealed",
      summary: LABEL_LEAK_SUMMARY,
      next: "Keep the label in its own file. Run the same command again.",
      nextCall: null,
    }
  }
  // A finished miss has ok false, so nextCall stays null. Mutants still in notStarted are not that miss.
  if ((report.budgetHit === true || notStarted.length > 0) && judged.ok) {
    const patchDirs = texts(flags, "patches").map((dir) => path.resolve(dir))
    return {
      ...judged,
      nextCall: {
        argv: resumeArgv(resumeInput(flags, "sealed", {
          packageDir,
          repoDir,
          outDir,
          patchDirs,
          commit: scored,
          hide,
          label: labelPath,
          fix: text(flags, "fix") ?? null,
        })),
      },
    }
  }
  return judged
}

async function ledgerCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
  const packageDir = path.resolve(requireText(flags, "package"))
  const outDir = path.resolve(requireText(flags, "out"))
  const repoDir = path.resolve(text(flags, "repo") ?? gitRoot(packageDir))
  const report = buildLedger({
    packageDir,
    repoDir,
    commit: text(flags, "commit") ?? "HEAD",
    outDir,
    srcDir: text(flags, "src") ?? "src",
    testsDir: text(flags, "tests-dir") ?? "tests",
    maxLines: int(flags, "max-lines") ?? 300,
    maxCommits: int(flags, "max-commits") ?? null,
  })
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: report.ok,
    command: "ledger.build",
    summary: report.summary,
    next: report.next,
    nextCall: report.ok ? report.nextCall : null,
    clean: report.clean,
    hand: report.hand,
    handmade: report.handmade,
    skipped: report.skipped,
    handNeeded: report.handNeeded,
    rest: report.rest,
    commit: report.commit,
    full: report.full,
    historyBudgetHit: report.historyBudgetHit,
    commitsScanned: report.commitsScanned,
  }
}

function matrixCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const scoresPath = requireText(flags, "scores")
  const directPath = requireText(flags, "direct")
  const outDir = path.resolve(requireText(flags, "out"))
  const killsPath = text(flags, "kills")
  return reportMatrix({
    scores: readFileSync(scoresPath, "utf8"),
    direct: readFileSync(directPath, "utf8"),
    patchDirs: texts(flags, "patches").map((dir) => path.resolve(dir)),
    outDir,
    kills: killsPath ? readKills(killsPath) : new Map(),
  })
}

function goldenCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const recordedPath = requireText(flags, "recorded")
  const recordedText = readFileSync(recordedPath, "utf8")
  const updateRaw = text(flags, "update")
  if (updateRaw && updateRaw !== "wording" && updateRaw !== "all") throw new Error("--update must be wording or all")
  const result = checkGolden({
    recorded: asTable(recordedText),
    actual: readGolden(requireText(flags, "actual")),
    update: updateRaw === "wording" || updateRaw === "all" ? updateRaw : null,
    message: text(flags, "message") ?? "",
    roots: texts(flags, "root").map((dir) => path.resolve(dir)),
  })
  if (result.write) writeGolden(recordedPath, result.table, goldenShape(recordedText))
  return result.envelope
}

function gapFixCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const line = int(flags, "line")
  if (line === undefined || line < 1) throw new Error("--line must be a positive integer")
  const state = fixGap(loadState(dir), {
    id: requireText(flags, "id"),
    file: requireText(flags, "file"),
    line,
    guard: requireText(flags, "guard"),
    commit: requireText(flags, "commit"),
    root: path.resolve(text(flags, "root") ?? dir),
  })
  saveState(dir, state)
  const gap = state.gaps.find((item) => item.id === requireText(flags, "id"))
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "gap.fix",
    summary: `${gap?.id} is fixed, guarded by ${gap?.guard}.`,
    next: "Run status.",
    nextCall: { argv: ["status", "--state", dir] },
  }
}

function gapRevertCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const reverted = revertGap(loadState(dir), requireText(flags, "id"))
  saveState(dir, reverted.state)
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "gap.revert",
    summary: reverted.text,
    next: reverted.text,
    nextCall: null,
  }
}

function guardCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const guard = requireText(flags, "guard")
  parseGuard(guard)
  const result = guardAllows(loadState(dir), guard, text(flags, "message") ?? "", path.resolve(text(flags, "root") ?? dir))
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: result.ok,
    command: "guard.check",
    summary: result.summary,
    next: result.ok ? "The guard check passed." : "Add a Guard-Change trailer naming the guard, or restore it.",
    nextCall: null,
  }
}

function findingsCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const status = text(flags, "status")
  if (status !== "open" && status !== "fixed" && status !== "equivalent" && status !== "wont-fix") {
    throw new Error("--status must be open, fixed, equivalent, or wont-fix")
  }
  const id = requireText(flags, "id")
  const reason = requireText(flags, "reason")
  const finding: { id: string; status: FindingStatus; reason: string; file?: string; line?: number; hash?: string; agent?: string } = {
    id,
    status,
    reason,
  }
  const agent = text(flags, "agent")
  if (agent) finding.agent = agent
  const file = text(flags, "file")
  const line = int(flags, "line")
  if (file && line !== undefined) {
    const source = readSourceLine(path.resolve(text(flags, "root") ?? dir, file), line)
    if (source === null) throw new Error("decision line does not exist")
    finding.file = file
    finding.line = line
    finding.hash = lineHash(source)
    const state = loadState(dir)
    const decisions = state.decisions.filter((item) => item.id !== id)
    decisions.push({ id, file, line, hash: finding.hash, status, reason })
    saveState(dir, { ...state, decisions })
  }
  appendFinding(dir, finding)
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "findings.add",
    summary: `Recorded ${id} as ${status}.`,
    next: "Run status.",
    nextCall: null,
  }
}

function statusCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const root = path.resolve(text(flags, "root") ?? dir)
  const pagePath = path.resolve(text(flags, "page") ?? "PROBATIO.md")
  const scoresPath = text(flags, "scores")
  const score = scoresPath ? JSON.parse(readFileSync(scoresPath, "utf8")) as KillScore : null
  const view = statusEnvelope(dir, root, score, text(flags, "agent") ?? null)
  mkdirSync(path.dirname(pagePath), { recursive: true })
  writeFileSync(pagePath, view.page)
  return view.envelope
}

function sealCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const from = text(flags, "from")
  let ids = texts(flags, "id")
  if (from) {
    const parsed = JSON.parse(readFileSync(from, "utf8")) as { ids?: string[] } | string[]
    const list = Array.isArray(parsed) ? parsed : parsed.ids ?? []
    const fraction = Number(text(flags, "fraction") ?? "0.2")
    if (!Number.isFinite(fraction) || fraction < 0 || fraction > 1) throw new Error("--fraction must be between 0 and 1")
    ids = chooseSealed(list, fraction, int(flags, "seed") ?? 20261004)
  }
  sealIds(dir, ids)
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "seal",
    summary: `Sealed ${ids.length} ids.`,
    next: "status, PROBATIO.md, and the queue omit them.",
    nextCall: null,
    sealedCount: ids.length,
  }
}

function queueSeedCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const written = seedQueue(dir, texts(flags, "id"))
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "queue.seed",
    summary: `Seeded ${written.length} queue items.`,
    next: "Claim one.",
    nextCall: null,
    seeded: written,
  }
}

function queueClaimCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const now = int(flags, "now") ?? Date.now()
  const lease = int(flags, "lease-ms") ?? 600_000
  const result = claimItem(dir, requireText(flags, "id"), requireText(flags, "agent"), now, lease)
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: result.ok,
    command: "queue.claim",
    summary: result.summary,
    next: result.ok ? "Work the claim, then check-kill it." : "Pick another item.",
    nextCall: null,
  }
}

function queueReapCommand(flags: ReturnType<typeof parseArgs>["flags"]): Envelope {
  const dir = path.resolve(requireText(flags, "state"))
  const now = int(flags, "now") ?? Date.now()
  const returned = reapClaims(dir, now)
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "queue.reap",
    summary: `Returned ${returned.length} expired claims.`,
    next: returned.length === 0 ? "Nothing expired." : "The expired claims are back in the queue.",
    nextCall: null,
    returned,
  }
}

async function checkKillCommand(flags: ReturnType<typeof parseArgs>["flags"], id: string | undefined): Promise<Envelope> {
  if (!id) throw new Error("check-kill needs a mutant id")
  const packageDir = path.resolve(requireText(flags, "package"))
  const patchDirs = texts(flags, "patches").map((dir) => path.resolve(dir))
  if (patchDirs.length === 0) throw new Error("--patches is required")
  const build = buildCommand(flags)
  return checkKill(id, {
    packageDir,
    repoDir: path.resolve(text(flags, "repo") ?? gitRoot(packageDir)),
    commit: text(flags, "commit") ?? "HEAD",
    patchDirs,
    outDir: path.resolve(requireText(flags, "out")),
    direction: "forward",
    workers: 1,
    concurrency: 1,
    maxMinutes: int(flags, "max-minutes") ?? 3,
    maxMutants: null,
    build,
    testsDir: text(flags, "tests-dir") ?? "tests",
    unset: ["SOLARI_API_KEY", "AUSPEX_LIVE"],
    suiteTimeoutMs: int(flags, "suite-timeout-ms") ?? 60_000,
    testTimeoutMs: int(flags, "test-timeout-ms") ?? 30_000,
    confirm: bool(flags, "confirm", true),
    affected: false,
    agent: text(flags, "agent") ?? null,
    onProgress: (line) => process.stderr.write(`${line}\n`),
  })
}

async function ledgerCheckCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
  const packageDir = path.resolve(requireText(flags, "package"))
  return checkLedger({
    packageDir,
    repoDir: path.resolve(text(flags, "repo") ?? gitRoot(packageDir)),
    commit: text(flags, "commit") ?? "HEAD",
    ledgerDir: path.resolve(packageDir, text(flags, "ledger") ?? "ledger"),
    outDir: path.resolve(requireText(flags, "out")),
    update: bool(flags, "update", false),
    message: text(flags, "message") ?? "",
    concurrency: int(flags, "concurrency") ?? 3,
    suiteTimeoutMs: int(flags, "suite-timeout-ms") ?? 600_000,
    testTimeoutMs: int(flags, "test-timeout-ms") ?? 60_000,
    onProgress: (line) => process.stderr.write(`${line}\n`),
  })
}

async function goldenRecordCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
  const packageDir = path.resolve(requireText(flags, "package"))
  return recordGoldens({
    packageDir,
    repoDir: path.resolve(text(flags, "repo") ?? gitRoot(packageDir)),
    commit: text(flags, "commit") ?? "HEAD",
    modules: texts(flags, "module"),
    tests: texts(flags, "tests"),
    outDir: text(flags, "dir") ?? "tests/golden",
    timeoutMs: int(flags, "suite-timeout-ms") ?? 600_000,
  })
}

async function goldenCompareCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
  const packageDir = path.resolve(requireText(flags, "package"))
  return compareGoldens({
    packageDir,
    repoDir: path.resolve(text(flags, "repo") ?? gitRoot(packageDir)),
    commit: text(flags, "commit") ?? "HEAD",
    modules: texts(flags, "module"),
    tests: texts(flags, "tests"),
    golden: texts(flags, "golden"),
    outDir: path.resolve(requireText(flags, "out")),
    operators: operatorSet(flags),
    timeoutMs: int(flags, "suite-timeout-ms") ?? 600_000,
    onProgress: (line) => process.stderr.write(`${line}\n`),
  })
}

async function verifyCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
  const packageDir = path.resolve(requireText(flags, "package"))
  const operators = operatorSet(flags)
  const envelope = await verifyChange({
    packageDir,
    repoDir: path.resolve(text(flags, "repo") ?? gitRoot(packageDir)),
    outDir: path.resolve(requireText(flags, "out")),
    base: text(flags, "base") ?? "HEAD",
    commit: text(flags, "commit") ?? "HEAD",
    maxMutants: int(flags, "max-mutants") ?? 4,
    maxMinutes: int(flags, "max-minutes") ?? 3,
    maxTests: int(flags, "max-tests") ?? null,
    operators,
    onProgress: (line) => process.stderr.write(`${line}\n`),
  })
  return { ...envelope, operators }
}

/** `--operators core` (default) or `--operators wide`. Anything else is a usage error, not a silent default. */
function operatorSet(flags: ReturnType<typeof parseArgs>["flags"]): OperatorSet {
  const value = text(flags, "operators") ?? "core"
  if (value !== "core" && value !== "wide") throw new Error("--operators must be core or wide")
  return value
}

/** The published JSON Schema of one command's output. MCP-only agents read the contract this way. */
function schemaCommand(name: string | undefined): Envelope {
  const dir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "schemas")
  const commands = existsSync(dir)
    ? readdirSync(dir).filter((file) => file.endsWith(".schema.json")).map((file) => file.slice(0, -".schema.json".length)).sort()
    : []
  if (!name) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: commands.length > 0,
      command: "schema",
      summary: `${commands.length} commands have a published schema.`,
      next: "Pass one name, for example: probatio schema mutate.run",
      nextCall: null,
      commands,
    }
  }
  if (!commands.includes(name)) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "schema",
      summary: `No schema named ${name}.`,
      next: `Use one of: ${commands.join(", ")}.`,
      nextCall: null,
      commands,
    }
  }
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "schema",
    summary: `JSON Schema for ${name}, schemaVersion ${SCHEMA_VERSION}.`,
    next: "Validate a command's stdout against it. A new field does not change schemaVersion. A renamed or removed one does.",
    nextCall: null,
    commands,
    schema: JSON.parse(readFileSync(path.join(dir, `${name}.schema.json`), "utf8")) as unknown,
  }
}

function gitRoot(cwd: string): string {
  const result = git(cwd, ["rev-parse", "--show-toplevel"])
  if (result.status !== 0) throw new Error("package is not inside a git repository. Pass --repo.")
  return result.stdout.trim()
}

function finish(envelope: Envelope, human: boolean): never {
  process.stdout.write(render(envelope, human))
  process.exit(envelope.ok ? 0 : 1)
}

/** Omitted `--build` and `--no-build` both mean no build. `--build <cmd>` opts in. */
function buildText(flags: Map<string, FlagValue[]>): string | null {
  const values = flags.get("build")
  if (!values || values.length === 0) return null
  const last = values[values.length - 1]
  if (typeof last !== "string") return null
  return last.trim().length > 0 ? last : null
}

function buildCommand(flags: Map<string, FlagValue[]>): string[] | null {
  const command = buildText(flags)
  if (!command) return null
  const parts = command.split(" ").filter(Boolean)
  return parts.length > 0 ? parts : null
}

function passedInt(flags: Map<string, FlagValue[]>, key: string): number | null {
  if (!flags.has(key)) return null
  return int(flags, key) ?? null
}

function resumeInput(
  flags: Map<string, FlagValue[]>,
  verb: "run" | "sealed",
  paths: {
    packageDir: string
    repoDir: string
    outDir: string
    patchDirs: string[]
    commit: string
    hide: string[]
    label?: string | null
    fix?: string | null
  },
): BudgetResume {
  const history = text(flags, "history")
  return {
    verb,
    packageDir: paths.packageDir,
    repoDir: paths.repoDir,
    outDir: paths.outDir,
    patchDirs: paths.patchDirs,
    commit: paths.commit,
    suiteCommand: text(flags, "suite-command") ?? null,
    build: buildText(flags),
    hide: paths.hide,
    suiteTimeoutMs: int(flags, "suite-timeout-ms") ?? 600_000,
    testTimeoutMs: int(flags, "test-timeout-ms") ?? 60_000,
    timeoutMultiple: passedInt(flags, "timeout-multiple"),
    budgetMs: passedInt(flags, "budget-ms"),
    maxMinutes: passedInt(flags, "max-minutes"),
    timeoutFloorMs: passedInt(flags, "timeout-floor-ms"),
    maxMutants: passedInt(flags, "max-mutants"),
    testsDir: text(flags, "tests-dir") ?? "tests",
    workers: int(flags, "workers") ?? 1,
    confirm: bool(flags, "confirm", true),
    direction: text(flags, "direction") ?? "auto",
    onlyTests: texts(flags, "only-test"),
    affected: bool(flags, "affected", false),
    unset: texts(flags, "unset"),
    history: history ? path.resolve(history) : null,
    label: paths.label ?? null,
    fix: paths.fix ?? null,
  }
}
