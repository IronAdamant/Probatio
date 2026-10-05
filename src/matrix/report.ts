import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"
import { filesInDiff } from "../mutate/patch.js"

export type Count = { caught: number; total: number }

export type SuiteRow = {
  set: string
  name: string
  source: "union" | "direct"
  lines: number | null
  holdout: Count | null
  real: Count | null
  synthetic1: Count | null
  synthetic2: Count | null
  missed: string[]
}

export type Gap = {
  id: string
  file: string | null
  line: number | null
  locations: Array<{ file: string; line: number }>
}

export type PruneAdvice = {
  mode: "advisory"
  deletedTests: number
  advice: string[]
}

const METRIC = /^(holdout|real \(train\)|synthetic-1|synthetic-2):\s+(\d+)\/(\d+)(?:\s+\([^)]*\))?(?:\s+missed\s+(.+))?$/
const PREFIX = /^(?:ho|r|h|v)-/

/** Names of tests that killed nothing. This never deletes a file. */
export function advisePrune(kills: ReadonlyMap<string, number>): PruneAdvice {
  const advice = [...kills.entries()]
    .filter(([, killed]) => killed === 0)
    .map(([name]) => name)
    .sort()
  return { mode: "advisory", deletedTests: 0, advice }
}

export function parseUnionTsv(text: string): SuiteRow[] {
  const lines = text.split(/\r?\n/).filter((line) => line.trim().length > 0)
  if (lines.length === 0) return []
  const columns = lines[0].split("\t").map(parseHeader)
  return lines.slice(1).map((line) => {
    const cells = line.split("\t")
    const row = emptyRow("union")
    for (let i = 0; i < columns.length; i++) {
      const column = columns[i]
      const cell = (cells[i] ?? "").trim()
      if (column.kind === "set") {
        row.set = cell
        row.name = cell
      } else if (column.kind === "lines") {
        const lines = Number(cell)
        if (!Number.isInteger(lines)) throw new Error(`lines is not an integer: ${cell}`)
        row.lines = lines
      } else if (column.total === null) {
        throw new Error(`column ${column.kind} has no total`)
      } else {
        assign(row, column.kind, parseCaught(cell, column.total))
      }
    }
    return row
  })
}

export function parseDirectScores(text: string): SuiteRow[] {
  const rows: SuiteRow[] = []
  let current: SuiteRow | null = null
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    if (!line) continue
    const metric = line.match(METRIC)
    if (!metric) {
      current = emptyRow("direct")
      current.name = line
      current.set = line.split("(")[0].trim().split(/\s+/).join("")
      rows.push(current)
      continue
    }
    if (!current) throw new Error(`metric before a suite: ${line}`)
    assign(current, metricKind(metric[1]), { caught: Number(metric[2]), total: Number(metric[3]) })
    if (metric[4]) current.missed.push(...metric[4].trim().split(/\s+/).filter(Boolean))
  }
  return rows
}

export function reportMatrix(input: {
  scores: string
  direct: string
  patchDirs: string[]
  outDir: string
  kills: ReadonlyMap<string, number>
}): Envelope {
  const rows = [...parseUnionTsv(input.scores), ...parseDirectScores(input.direct)]
  if (rows.length === 0) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "matrix.report",
      summary: "The score files have no suite rows.",
      next: "Pass --scores and --direct that contain the recorded table.",
      nextCall: null,
    }
  }
  const reference = rows.find((row) => row.source === "union" && row.holdout) ?? null
  const gapRow = weakestHoldout(rows.filter((row) => row.source === "direct"))
  const gaps = resolveGaps(gapRow?.missed ?? [], input.patchDirs)
  const shown = gaps.slice(0, 10)
  const collapse = {
    batch2: rows
      .filter((row) => row.source === "union" && lowerRate(row.synthetic2, row.synthetic1))
      .map((row) => ({ set: row.set, synthetic1: row.synthetic1, synthetic2: row.synthetic2 })),
    holdoutBelowReference: reference
      ? rows
          .filter((row) => row !== reference && lowerRate(row.holdout, reference.holdout))
          .map((row) => ({ set: row.set, holdout: row.holdout, reference: reference.holdout }))
      : [],
  }
  const pruning = advisePrune(input.kills)
  const fullPath = path.join(input.outDir, "matrix.json")
  const complete: Envelope = {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "matrix.report",
    summary: summarize(gapRow, collapse.batch2, gaps.length, pruning.deletedTests),
    next: nextSentence(gaps),
    nextCall: null,
    rows,
    collapse,
    gaps,
    rest: 0,
    pruning,
    full: fullPath,
  }
  mkdirSync(input.outDir, { recursive: true })
  writeFileSync(fullPath, `${JSON.stringify(complete, null, 2)}\n`)
  return {
    ...complete,
    gaps: shown,
    rest: gaps.length - shown.length,
  }
}

export function readKills(file: string): Map<string, number> {
  const parsed = JSON.parse(readFileSync(file, "utf8")) as unknown
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("--kills must be a JSON object of test name to kill count")
  const kills = new Map<string, number>()
  for (const [name, value] of Object.entries(parsed)) {
    if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
      throw new Error(`kill count for ${name} must be a non-negative integer`)
    }
    kills.set(name, value)
  }
  return kills
}

function summarize(
  gapRow: SuiteRow | null,
  batch2: Array<{ set: string; synthetic1: Count | null; synthetic2: Count | null }>,
  gapCount: number,
  deletedTests: number,
): string {
  const parts: string[] = []
  if (gapRow?.holdout) {
    const train = gapRow.real ? ` Train real is ${gapRow.real.caught}/${gapRow.real.total}.` : ""
    parts.push(`${gapRow.set} holdout is ${gapRow.holdout.caught}/${gapRow.holdout.total}.${train}`)
  }
  if (batch2.length > 0) {
    const bits = batch2.map((item) => {
      const left = item.synthetic1
      const right = item.synthetic2
      return `${item.set} synthetic-1 ${left?.caught}/${left?.total} against synthetic-2 ${right?.caught}/${right?.total}`
    })
    parts.push(`Cover collapse on batch 2: ${bits.join("; ")}.`)
  }
  parts.push(`${gapCount} gaps. Pruning is advisory and deleted ${deletedTests} tests.`)
  return parts.join(" ")
}

function nextSentence(gaps: Gap[]): string {
  const first = gaps.find((gap) => gap.file !== null && gap.line !== null) ?? gaps[0]
  if (!first) return "No gaps."
  if (first.file === null || first.line === null) return `${first.id} has no patch, so there is no file:line.`
  return `${first.id} at ${first.file}:${first.line}`
}

function weakestHoldout(rows: SuiteRow[]): SuiteRow | null {
  let best: SuiteRow | null = null
  let bestRate = Number.POSITIVE_INFINITY
  for (const row of rows) {
    const value = rate(row.holdout)
    if (value === null || value >= bestRate) continue
    best = row
    bestRate = value
  }
  return best
}

function lowerRate(left: Count | null, right: Count | null): boolean {
  const a = rate(left)
  const b = rate(right)
  return a !== null && b !== null && a < b
}

function rate(count: Count | null): number | null {
  if (!count || count.total === 0) return null
  return count.caught / count.total
}

function resolveGaps(ids: string[], patchDirs: string[]): Gap[] {
  const unique: string[] = []
  const seen = new Set<string>()
  for (const id of ids) {
    if (seen.has(id)) continue
    seen.add(id)
    unique.push(id)
  }
  unique.sort()
  return unique.map((id) => locate(id, patchDirs))
}

function locate(id: string, patchDirs: string[]): Gap {
  const bare = id.replace(PREFIX, "")
  const names = [...new Set([`${id}.patch`, `r-${bare}.patch`, `h-${bare}.patch`, `${bare}.patch`, `ho-${bare}.patch`])]
  for (const dir of patchDirs) {
    for (const name of names) {
      const candidate = path.join(dir, name)
      let raw: string
      try {
        raw = readFileSync(candidate, "utf8")
      } catch {
        continue
      }
      const locations = filesInDiff(raw)
      const first = locations[0]
      return { id, file: first?.file ?? null, line: first?.line ?? null, locations }
    }
  }
  return { id, file: null, line: null, locations: [] }
}

function parseHeader(cell: string): { kind: string; total: number | null } {
  const match = cell.trim().match(/^(.*?)\s*\((\d+)\)$/)
  if (!match) return { kind: cell.trim(), total: null }
  return { kind: metricKind(match[1].trim()), total: Number(match[2]) }
}

function metricKind(name: string): string {
  if (name === "synthetic-1") return "synthetic1"
  if (name === "synthetic-2") return "synthetic2"
  if (name === "real (train)" || name === "real") return "real"
  return name
}

function parseCaught(cell: string, total: number): Count {
  const match = cell.match(/^(\d+)/)
  if (!match) throw new Error(`cannot read a count from "${cell}"`)
  return { caught: Number(match[1]), total }
}

function assign(row: SuiteRow, kind: string, count: Count) {
  if (kind === "holdout") row.holdout = count
  else if (kind === "real") row.real = count
  else if (kind === "synthetic1") row.synthetic1 = count
  else if (kind === "synthetic2") row.synthetic2 = count
  else throw new Error(`unknown score column ${kind}`)
}

function emptyRow(source: SuiteRow["source"]): SuiteRow {
  return {
    set: "",
    name: "",
    source,
    lines: null,
    holdout: null,
    real: null,
    synthetic1: null,
    synthetic2: null,
    missed: [],
  }
}
