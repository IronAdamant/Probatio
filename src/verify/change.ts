import { mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs"
import path from "node:path"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"
import { asTable } from "../golden/check.js"
import { affectedTests, directImporters } from "../mutate/affected.js"
import { findInSource } from "../mutate/find.js"
import { mutantId } from "../mutate/ids.js"
import { forwardDiff, git } from "../mutate/patch.js"
import { runMutants, type MutantResult } from "../mutate/run.js"
import { WHOLE_PROGRAM_TEST, leadSummary } from "../mutate/suite-decision.js"
import { discoverSuite } from "../mutate/suites.js"

export type VerifyOptions = {
  packageDir: string
  repoDir: string
  outDir: string
  base: string
  commit: string
  maxMutants: number
  maxMinutes: number
  /** Caps the direct-importer names in the report. The run itself uses the line map. */
  maxTests: number | null
  onProgress?: (line: string) => void
}

type Located = { id: string; file: string; line: number; op: string }

type Other = { id: string; outcome: string; file: string; line: number }

/** Diff-scoped mutants, the tests that can see the edit, and golden contract rows that changed. */
export async function verifyChange(options: VerifyOptions): Promise<Envelope> {
  const repo = realpathSync(path.resolve(options.repoDir))
  const packageDir = realpathSync(path.resolve(options.packageDir))
  const prefix = packagePrefix(repo, packageDir)
  // The package may sit inside a large checkout. Diff that directory only.
  // A whole-repo unified diff can exceed the pipe and come back as a failed spawn.
  const scope = prefix ? ["--", prefix] : []
  const diff = git(repo, ["diff", "-U0", "--no-renames", options.base, options.commit, ...scope])
  if (diff.status !== 0) return fail(options.outDir, diff.stderr.trim() || "git diff failed")
  const ranges = parseUnified(diff.stdout)
  const names = git(repo, ["diff", "--name-only", "--no-renames", options.base, options.commit, ...scope])
  const changed = names.stdout.split(/\r?\n/).map((line) => line.trim()).filter(Boolean)
  const goldenContract = goldenDiffs(repo, options.base, options.commit, changed)
  const packageFiles: string[] = []
  const chosen: Array<{ rel: string; label: string; text: string; point: { start: number; end: number; line: number; op: string; replacement: string } }> = []
  for (const [file, lines] of ranges) {
    const rel = toPackage(prefix, file)
    if (!rel || !rel.startsWith("src/") || rel.endsWith(".d.ts")) continue
    packageFiles.push(rel)
    const text = showFile(repo, options.commit, file)
    if (!text) continue
    const found = findInSource(rel, text)
    const lineSet = new Set(lines)
    for (const point of found.points) {
      if (!lineSet.has(point.line)) continue
      chosen.push({ rel, label: file, text, point })
    }
  }
  chosen.sort((a, b) => a.rel.localeCompare(b.rel) || a.point.line - b.point.line || a.point.op.localeCompare(b.point.op) || a.point.start - b.point.start)
  const capped = chosen.slice(0, options.maxMutants)
  const mutantsDir = path.join(options.outDir, "mutants")
  mkdirSync(mutantsDir, { recursive: true })
  const written: Located[] = []
  const notRun: Located[] = chosen.slice(capped.length).map(locate)
  for (const item of capped) {
    const located = locate(item)
    const mutated = item.text.slice(0, item.point.start) + item.point.replacement + item.text.slice(item.point.end)
    const body = forwardDiff(item.label, item.text, mutated)
    if (!body) {
      notRun.push(located)
      continue
    }
    writeFileSync(path.join(mutantsDir, `${located.id}.patch`), body)
    written.push(located)
  }
  writeFileSync(path.join(mutantsDir, "mutants.json"), `${JSON.stringify({ schemaVersion: SCHEMA_VERSION, mutants: written }, null, 2)}\n`)
  const affectedMap = affectedTests(packageDir, { testsDir: "tests" })
  const affected = [...new Set(packageFiles.flatMap((file) => affectedMap[file] ?? []))].sort()
  const direct = directImporters(packageDir, packageFiles, "tests")
  const ran = options.maxTests == null ? direct : direct.slice(0, options.maxTests)
  const notRan = direct.slice(ran.length)
  let caught: Located[] = []
  let missed: Located[] = []
  let other: Other[] = []
  let suiteCommand = ""
  const suite = discoverSuite(packageDir, "tests")
  if (written.length > 0 && suite && suite.files.length > 0) {
    const report = await runMutants({
      packageDir,
      repoDir: repo,
      commit: options.commit,
      patchDirs: [mutantsDir],
      outDir: path.join(options.outDir, "run"),
      direction: "forward",
      workers: 1,
      concurrency: 1,
      maxMinutes: options.maxMinutes,
      maxMutants: null,
      build: null,
      testsDir: "tests",
      unset: ["SOLARI_API_KEY", "AUSPEX_LIVE"],
      suiteTimeoutMs: 600_000,
      testTimeoutMs: 60_000,
      confirm: false,
      affected: false,
      agent: null,
      onProgress: options.onProgress,
    })
    suiteCommand = report.suiteCommand
    if (!report.ok) return fail(options.outDir, report.summary, { affected, ran, notRan, goldenContract, notRun, next: report.next, suiteCommand })
    const byId = new Map(written.map((item) => [item.id, item]))
    const results = readOutcomes(path.join(options.outDir, "run"), written.map((item) => item.id))
    caught = results.filter((item) => item.outcome === "killed").map((item) => byId.get(item.id)).filter((item): item is Located => item !== undefined)
    missed = results.filter((item) => item.outcome === "survived").map((item) => byId.get(item.id)).filter((item): item is Located => item !== undefined)
    other = results
      .filter((item) => item.outcome !== "killed" && item.outcome !== "survived")
      .map((item) => ({ id: item.id, outcome: item.outcome, file: byId.get(item.id)?.file ?? "", line: byId.get(item.id)?.line ?? 0 }))
    const executed = executedNames(results)
    const envelope = bodyOf(options.outDir, true, affected, ran, notRan, caught, missed, other, goldenContract, notRun, suiteCommand, executed)
    writeFileSync(path.join(options.outDir, "verify.json"), `${JSON.stringify(envelope, null, 2)}\n`)
    return envelope
  }
  notRun.push(...written)
  const envelope = bodyOf(options.outDir, true, affected, ran, notRan, caught, missed, other, goldenContract, notRun, suiteCommand, [])
  writeFileSync(path.join(options.outDir, "verify.json"), `${JSON.stringify(envelope, null, 2)}\n`)
  return envelope
}

function bodyOf(
  outDir: string,
  ok: boolean,
  affected: string[],
  ran: string[],
  notRan: string[],
  caught: Located[],
  missed: Located[],
  other: Other[],
  goldenContract: Array<{ file: string; row: string; fields: string[] }>,
  notRun: Located[],
  suiteCommand: string,
  executed: string[],
): Envelope {
  const uncovered = other.filter((item) => item.outcome === "no coverage")
  const summary = ok
    ? leadSummary(uncovered.length, missed.length, "missed", `${caught.length} caught, ${notRun.length} not run, ${affected.length} affected tests, ${goldenContract.length} golden contract changes.`)
    : "verify-change did not finish."
  const first = missed[0] ?? uncovered[0] ?? notRun[0] ?? caught[0]
  return {
    schemaVersion: SCHEMA_VERSION,
    ok,
    command: "verify-change",
    summary,
    next: first ? `${first.id} at ${first.file}:${first.line}` : "No diff-scoped mutant.",
    nextCall: null,
    affected,
    ran,
    notRan,
    suiteCommand,
    executed,
    confirmed: false,
    caught,
    missed,
    other,
    notRun,
    goldenContract,
    rest: notRun.length,
    full: path.join(outDir, "verify.json"),
  }
}

function locate(item: { rel: string; point: { start: number; end: number; line: number; op: string } }): Located {
  return {
    id: mutantId(item.rel, item.point.start, item.point.end, item.point.op),
    file: item.rel,
    line: item.point.line,
    op: item.point.op,
  }
}

function fail(
  outDir: string,
  summary: string,
  extra?: {
    affected: string[]
    ran: string[]
    notRan: string[]
    goldenContract: Array<{ file: string; row: string; fields: string[] }>
    notRun: Located[]
    next?: string
    suiteCommand?: string
  },
): Envelope {
  mkdirSync(outDir, { recursive: true })
  const notRun = extra?.notRun ?? []
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: false,
    command: "verify-change",
    summary,
    next: extra?.next ?? "Fix that and run the same command again.",
    nextCall: null,
    affected: extra?.affected ?? [],
    ran: extra?.ran ?? [],
    notRan: extra?.notRan ?? [],
    suiteCommand: extra?.suiteCommand ?? "",
    executed: [],
    confirmed: false,
    caught: [],
    missed: [],
    other: [],
    notRun,
    goldenContract: extra?.goldenContract ?? [],
    rest: notRun.length,
    full: path.join(outDir, "verify.json"),
  }
}

function readOutcomes(outDir: string, ids: string[]): Array<{ id: string; outcome: string; selectedTests: string[] }> {
  const file = path.join(outDir, "results")
  const out: Array<{ id: string; outcome: string; selectedTests: string[] }> = []
  for (const id of ids) {
    try {
      const result = JSON.parse(readFileSync(path.join(file, `${id}.json`), "utf8")) as MutantResult
      out.push({ id, outcome: result.outcome, selectedTests: result.selectedTests ?? [] })
    } catch {
      out.push({ id, outcome: "missing", selectedTests: [] })
    }
  }
  return out
}

function executedNames(results: Array<{ selectedTests: string[] }>): string[] {
  return [...new Set(results.flatMap((item) => item.selectedTests))].filter((name) => name.length > 0 && name !== WHOLE_PROGRAM_TEST).sort()
}

function goldenDiffs(repo: string, base: string, commit: string, files: string[]): Array<{ file: string; row: string; fields: string[] }> {
  const found: Array<{ file: string; row: string; fields: string[] }> = []
  for (const file of files) {
    if (!file.endsWith(".golden.json") && !file.includes("/golden/")) continue
    if (!file.endsWith(".json")) continue
    const before = asTable(showFile(repo, base, file))
    const after = asTable(showFile(repo, commit, file))
    const ids = [...new Set([...Object.keys(before.rows), ...Object.keys(after.rows)])].sort()
    for (const row of ids) {
      const left = contractOf(before.rows[row])
      const right = contractOf(after.rows[row])
      if (JSON.stringify(left) === JSON.stringify(right)) continue
      const fields = [...new Set([...Object.keys(left), ...Object.keys(right)])].filter((key) => JSON.stringify(left[key]) !== JSON.stringify(right[key])).sort()
      found.push({ file, row, fields })
    }
  }
  return found
}

function contractOf(row: Record<string, unknown> | undefined): Record<string, unknown> {
  if (!row) return {}
  const out: Record<string, unknown> = {}
  for (const key of Object.keys(row).sort()) {
    if (key === "next" || key === "nextLead") continue
    out[key] = row[key]
  }
  return out
}

function parseUnified(diff: string): Map<string, number[]> {
  const lines = new Map<string, number[]>()
  let file: string | null = null
  for (const line of diff.split(/\r?\n/)) {
    const plus = line.match(/^\+\+\+ ([^\t]+)/)
    if (plus) {
      let name = plus[1]
      if (name === "/dev/null") {
        file = null
        continue
      }
      if (name.startsWith("a/") || name.startsWith("b/")) name = name.slice(2)
      file = name
      if (!lines.has(file)) lines.set(file, [])
      continue
    }
    const hunk = line.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@/)
    if (!hunk || !file) continue
    const start = Number(hunk[1])
    const count = hunk[2] === undefined ? 1 : Number(hunk[2])
    const list = lines.get(file) ?? []
    for (let offset = 0; offset < count; offset++) list.push(start + offset)
    lines.set(file, list)
  }
  return lines
}

function showFile(repo: string, commit: string, file: string): string {
  const result = git(repo, ["show", `${commit}:${file}`])
  return result.status === 0 ? result.stdout : ""
}

function packagePrefix(repo: string, packageDir: string): string {
  const rel = path.relative(repo, packageDir)
  if (rel.startsWith("..")) throw new Error("package is outside the repository")
  return rel.split(path.sep).join("/")
}

function toPackage(prefix: string, file: string): string | null {
  if (!prefix) return file
  if (!file.startsWith(`${prefix}/`)) return null
  return file.slice(prefix.length + 1)
}
