#!/usr/bin/env node
import path from "node:path"
import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { SCHEMA_VERSION, render, type Envelope } from "./contract.js"
import { bool, int, parseArgs, requireText, text, texts, type FlagValue } from "./flags.js"
import { asTable, checkGolden, goldenShape, readGolden, writeGolden } from "./golden/check.js"
import { buildLedger } from "./ledger/build.js"
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
import { runMutants } from "./mutate/run.js"
import { generateNext } from "./mutate/suite-decision.js"
import { discoverSuite } from "./mutate/suites.js"
import { tallyRun } from "./mutate/tally.js"
import { checkKill } from "./swarm/check-kill.js"
import { claimItem, reapClaims, seedQueue } from "./swarm/queue.js"
import { verifyChange } from "./verify/change.js"

const parsed = parseArgs(process.argv.slice(2))
const [group, action] = parsed.command

try {
  if (group === "mutate" && action === "generate") finish(await generateCommand(parsed.flags), parsed.human)
  if (group === "mutate" && action === "run") finish(await runCommand(parsed.flags), parsed.human)
  if (group === "mutate" && action === "tally") finish(tallyCommand(parsed.flags), parsed.human)
  if (group === "ledger" && action === "build") finish(await ledgerCommand(parsed.flags), parsed.human)
  if (group === "matrix" && action === "report") finish(matrixCommand(parsed.flags), parsed.human)
  if (group === "golden" && action === "check") finish(goldenCommand(parsed.flags), parsed.human)
  if (group === "gap" && action === "fix") finish(gapFixCommand(parsed.flags), parsed.human)
  if (group === "gap" && action === "revert") finish(gapRevertCommand(parsed.flags), parsed.human)
  if (group === "guard" && action === "check") finish(guardCommand(parsed.flags), parsed.human)
  if (group === "findings" && action === "add") finish(findingsCommand(parsed.flags), parsed.human)
  if (group === "status") finish(statusCommand(parsed.flags), parsed.human)
  if (group === "seal") finish(sealCommand(parsed.flags), parsed.human)
  if (group === "queue" && action === "seed") finish(queueSeedCommand(parsed.flags), parsed.human)
  if (group === "queue" && action === "claim") finish(queueClaimCommand(parsed.flags), parsed.human)
  if (group === "queue" && action === "reap") finish(queueReapCommand(parsed.flags), parsed.human)
  if (group === "check-kill") finish(await checkKillCommand(parsed.flags, action), parsed.human)
  if (group === "verify-change") finish(await verifyCommand(parsed.flags), parsed.human)
  finish(usage(false, "Use mutate, ledger, matrix, golden, gap, guard, findings, status, queue, check-kill, or verify-change."), parsed.human)
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
    next: "probatio mutate generate writes operator mutants. probatio mutate run confirms each kill twice. probatio mutate tally reads a finished run and does not delete a test. probatio ledger build reverts fix commits. probatio matrix report reads a recorded score table.",
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
  })
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
  const suite = discoverSuite(packageDir, text(flags, "tests-dir") ?? "tests")
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "mutate.generate",
    summary: stopped
      ? `Stopped by the time budget after ${result.filesVisited} files. ${result.mutants.length} mutants written.`
      : `${result.mutants.length} mutants in ${result.filesVisited} files. No string-literal mutant.`,
    next: stopped
      ? "Narrow --src or raise --max-minutes, then run the same command."
      : generateNext(result.mutants.length, suite ? { kind: suite.kind, command: suite.command } : null),
    nextCall:
      result.mutants.length === 0
        ? null
        : { argv: ["mutate", "run", "--package", packageDir, "--patches", patches, "--out", path.join(outDir, "runs")] },
    mutantCount: result.mutants.length,
    stringLiteralMutants: 0,
    filesVisited: result.filesVisited,
    budgetHit: result.budgetHit,
    mutants: shown,
    rest: Math.max(result.mutants.length - shown.length, 0),
    full: path.join(patches, "mutants.json"),
  }
}

async function runCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
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
    historyPath: text(flags, "history") ? path.resolve(text(flags, "history") as string) : null,
    timeoutMultiple: int(flags, "timeout-multiple") ?? 5,
    timeoutFloorMs: int(flags, "timeout-floor-ms") ?? 20_000,
    budgetMs: int(flags, "budget-ms") ?? null,
    onProgress: (line) => process.stderr.write(`${line}\n`),
  })
  const resume: Envelope["nextCall"] = report.budgetHit
    ? { argv: ["mutate", "run", "--package", packageDir, "--repo", repoDir, "--out", outDir, ...patchDirs.flatMap((dir) => ["--patches", dir])] }
    : null
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: report.ok,
    command: "mutate.run",
    summary: report.summary,
    next: report.next,
    nextCall: report.ok ? resume : null,
    killed: report.killed,
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

async function verifyCommand(flags: ReturnType<typeof parseArgs>["flags"]): Promise<Envelope> {
  const packageDir = path.resolve(requireText(flags, "package"))
  return verifyChange({
    packageDir,
    repoDir: path.resolve(text(flags, "repo") ?? gitRoot(packageDir)),
    outDir: path.resolve(requireText(flags, "out")),
    base: text(flags, "base") ?? "HEAD",
    commit: text(flags, "commit") ?? "HEAD",
    maxMutants: int(flags, "max-mutants") ?? 4,
    maxMinutes: int(flags, "max-minutes") ?? 3,
    maxTests: int(flags, "max-tests") ?? null,
    onProgress: (line) => process.stderr.write(`${line}\n`),
  })
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
function buildCommand(flags: Map<string, FlagValue[]>): string[] | null {
  const values = flags.get("build")
  if (!values || values.length === 0) return null
  const last = values[values.length - 1]
  if (typeof last !== "string") return null
  const parts = last.split(" ").filter(Boolean)
  return parts.length > 0 ? parts : null
}
