import { readFileSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"

export type GoldenTable = { rows: Record<string, Record<string, unknown>> }
export type GoldenShape = "flat" | "rows"

const WORDING_KEYS = ["next", "nextLead"] as const

/** `Golden-Change: <row>: <why>`. The why starts after the first colon-and-space, so a row may contain colons. */
export function goldenChanges(message: string): Map<string, string> {
  const named = new Map<string, string>()
  for (const match of message.matchAll(/^Golden-Change:\s*(.+?):\s+(.+)$/gm)) named.set(match[1], match[2])
  return named
}

export function readGolden(file: string): GoldenTable {
  return asTable(readFileSync(file, "utf8"))
}

export function checkGolden(input: {
  recorded: GoldenTable
  actual: GoldenTable
  update: "wording" | "all" | null
  message: string
  roots: string[]
  home?: string
}): { envelope: Envelope; table: GoldenTable; write: boolean } {
  const home = input.home ?? homedir()
  const recorded = normalizeTable(input.recorded, input.roots)
  const actual = normalizeTable(input.actual, input.roots)
  const ids = [...new Set([...Object.keys(recorded.rows), ...Object.keys(actual.rows)])].sort()
  const wording: string[] = []
  const contract: string[] = []
  const invariant: string[] = []
  for (const id of ids) {
    const before = recorded.rows[id]
    const after = actual.rows[id]
    if (after) {
      const violation = invariantOf(id, after, home)
      if (violation) invariant.push(violation)
    }
    if (JSON.stringify(wordingOf(before)) !== JSON.stringify(wordingOf(after))) wording.push(id)
    if (JSON.stringify(contractOf(before)) !== JSON.stringify(contractOf(after))) contract.push(id)
  }
  const allowed = goldenChanges(input.message)
  const unauthorized = contract.filter((id) => !allowed.has(id))
  const canWrite = input.update !== null && invariant.length === 0 && unauthorized.length === 0
  const wordingWrite = canWrite && (input.update === "wording" || input.update === "all")
  const contractWrite = canWrite && contract.every((id) => allowed.has(id))
  const table = structuredClone(input.recorded)
  if (wordingWrite || (canWrite && contractWrite && contract.length > 0)) {
    for (const id of ids) {
      const after = input.actual.rows[id]
      const contractRow = contract.includes(id)
      const wordingRow = wording.includes(id)
      if (contractRow && contractWrite) {
        if (!after) delete table.rows[id]
        else table.rows[id] = after
      } else if (wordingRow && wordingWrite && after && table.rows[id]) {
        table.rows[id] = { ...table.rows[id], ...wordingOf(after) }
      } else if (wordingRow && wordingWrite && after && !table.rows[id] && !contractRow) {
        table.rows[id] = wordingOf(after)
      }
    }
  }
  const write = invariant.length === 0 && unauthorized.length === 0 && input.update !== null && (wording.length > 0 || contract.length > 0)
  const ok = invariant.length === 0 && unauthorized.length === 0 && (wording.length === 0 || input.update !== null)
  const summary = ok
    ? wording.length === 0 && contract.length === 0
      ? "Golden rows match."
      : `Re-recorded ${wording.length} wording row(s) and ${contract.length} contract row(s).`
    : invariant.length > 0
      ? `Invariant failed: ${invariant.join("; ")}.`
      : unauthorized.length > 0
        ? `Contract changed for ${unauthorized.join(", ")} without a Golden-Change trailer.`
        : `Wording changed for ${wording.join(", ")}. Re-record with --update wording.`
  return {
    write: write && ok,
    table,
    envelope: {
      schemaVersion: SCHEMA_VERSION,
      ok,
      command: "golden.check",
      summary,
      next: ok ? "Review the re-recorded rows." : "The table was not written.",
      nextCall: null,
      wording,
      contract,
      invariant,
      updated: write && ok,
    },
  }
}

export function goldenShape(text: string): GoldenShape {
  if (!text.trim()) return "rows"
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch {
    return "rows"
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return "flat"
  const rows = (parsed as Record<string, unknown>).rows
  if (rows && typeof rows === "object" && !Array.isArray(rows)) return "rows"
  return "flat"
}

export function writeGolden(file: string, table: GoldenTable, shape: GoldenShape = "rows") {
  const rows = shape === "flat" ? table.rows : sortRows(table.rows)
  const payload = shape === "flat" ? rows : { rows }
  writeFileSync(file, `${JSON.stringify(payload, null, 2)}\n`)
}

function invariantOf(id: string, row: Record<string, unknown>, home: string): string | null {
  if (row.ok === true && (typeof row.reason !== "string" || row.reason.trim().length === 0)) {
    return `${id} has ok without a reason`
  }
  if (home && containsHome(row, home)) return `${id} contains a home path`
  return null
}

function containsHome(value: unknown, home: string): boolean {
  if (typeof value === "string") return value.includes(home)
  if (Array.isArray(value)) return value.some((item) => containsHome(item, home))
  if (value && typeof value === "object") return Object.values(value).some((item) => containsHome(item, home))
  return false
}

function wordingOf(row: Record<string, unknown> | undefined): Record<string, unknown> {
  if (!row || typeof row !== "object" || Array.isArray(row)) return {}
  const out: Record<string, unknown> = {}
  for (const key of WORDING_KEYS) {
    if (Object.prototype.hasOwnProperty.call(row, key)) out[key] = row[key]
  }
  return out
}

function contractOf(row: Record<string, unknown> | undefined): Record<string, unknown> {
  if (!row) return {}
  const out: Record<string, unknown> = {}
  for (const key of Object.keys(row).sort()) {
    if ((WORDING_KEYS as readonly string[]).includes(key)) continue
    out[key] = row[key]
  }
  return out
}

function normalizeTable(table: GoldenTable, roots: string[]): GoldenTable {
  return { rows: normalize(table.rows, roots) as GoldenTable["rows"] }
}

function normalize(value: unknown, roots: string[]): unknown {
  if (typeof value === "string") {
    let out = value
    for (const root of roots) {
      if (root.length > 1 && out.includes(root)) out = out.split(root).join("{root}")
    }
    return out
  }
  if (Array.isArray(value)) return value.map((item) => normalize(item, roots))
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {}
    for (const [key, item] of Object.entries(value)) out[key] = normalize(item, roots)
    return out
  }
  return value
}

export function asTable(text: string): GoldenTable {
  if (!text.trim()) return { rows: {} }
  const parsed = JSON.parse(text) as unknown
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return { rows: {} }
  const record = parsed as Record<string, unknown>
  const rows = record.rows && typeof record.rows === "object" && !Array.isArray(record.rows)
    ? record.rows as Record<string, Record<string, unknown>>
    : record as Record<string, Record<string, unknown>>
  return { rows }
}

function sortRows(rows: Record<string, Record<string, unknown>>): Record<string, Record<string, unknown>> {
  const out: Record<string, Record<string, unknown>> = {}
  for (const key of Object.keys(rows).sort()) out[key] = rows[key]
  return out
}
