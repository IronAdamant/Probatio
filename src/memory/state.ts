import { createHash } from "node:crypto"
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"
import { makeRng, shuffle } from "../mutate/ids.js"

export const YARDSTICK = "these tests fit the yardstick, not the code."

export type FindingStatus = "open" | "fixed" | "equivalent" | "wont-fix"

export type GapStatus = "open" | "fixed" | "regression"

export type Gap = {
  id: string
  status: GapStatus
  file: string
  line: number
  guard: string | null
  fixedIn: string | null
  guardFile: string | null
  guardLineNo: number | null
  guardLine: string | null
  guardHash: string | null
}

export type Decision = {
  id: string
  file: string
  line: number
  hash: string
  status: FindingStatus
  reason: string
}

export type Finding = {
  id: string
  status: FindingStatus
  reason: string
  file?: string
  line?: number
  hash?: string
  agent?: string
}

export type MemoryState = {
  schemaVersion: number
  gaps: Gap[]
  decisions: Decision[]
}

const FINDING_STATUS = new Set<FindingStatus>(["open", "fixed", "equivalent", "wont-fix"])

export function lineHash(line: string): string {
  return createHash("sha256").update(line).digest("hex").slice(0, 16)
}

export function readSourceLine(file: string, line: number): string | null {
  if (!existsSync(file)) return null
  const lines = readFileSync(file, "utf8").split(/\r?\n/)
  if (line < 1 || line > lines.length) return null
  return lines[line - 1]
}

/** `Guard-Change: <guard>: <why>`. A guard such as `connect.test.ts:118` keeps its colon. */
export function guardChanges(message: string): Map<string, string> {
  const named = new Map<string, string>()
  for (const match of message.matchAll(/^Guard-Change:\s*(.+?):\s+(.+)$/gm)) named.set(match[1], match[2])
  return named
}

export function emptyState(): MemoryState {
  return { schemaVersion: SCHEMA_VERSION, gaps: [], decisions: [] }
}

export function loadState(dir: string): MemoryState {
  const file = path.join(dir, "state.json")
  if (!existsSync(file)) return emptyState()
  const parsed = JSON.parse(readFileSync(file, "utf8")) as MemoryState
  return { schemaVersion: SCHEMA_VERSION, gaps: parsed.gaps ?? [], decisions: parsed.decisions ?? [] }
}

export function saveState(dir: string, state: MemoryState) {
  mkdirSync(dir, { recursive: true })
  const ordered: MemoryState = {
    schemaVersion: SCHEMA_VERSION,
    gaps: [...state.gaps].sort((a, b) => a.id.localeCompare(b.id)),
    decisions: [...state.decisions].sort((a, b) => a.id.localeCompare(b.id)),
  }
  writeFileSync(path.join(dir, "state.json"), `${JSON.stringify(ordered, null, 2)}\n`)
}

export function loadSealed(dir: string): string[] {
  const file = path.join(dir, "sealed.json")
  if (!existsSync(file)) return []
  const parsed = JSON.parse(readFileSync(file, "utf8")) as { ids?: string[] }
  return [...new Set(parsed.ids ?? [])].sort()
}

export function sealIds(dir: string, ids: string[]) {
  const merged = [...new Set([...loadSealed(dir), ...ids])].sort()
  mkdirSync(dir, { recursive: true })
  writeFileSync(path.join(dir, "sealed.json"), `${JSON.stringify({ ids: merged }, null, 2)}\n`)
}

/** A stable share of the ids. The same seed returns the same slice. */
export function chooseSealed(ids: string[], fraction: number, seed: number): string[] {
  if (ids.length === 0 || fraction <= 0) return []
  const count = Math.min(ids.length, Math.max(1, Math.round(ids.length * fraction)))
  return shuffle([...ids], makeRng(seed)).slice(0, count).sort()
}

export function parseGuard(guard: string): { file: string; line: number } {
  const match = guard.match(/^(.*):(\d+)$/)
  if (!match) throw new Error("guard must be file:line")
  return { file: match[1], line: Number(match[2]) }
}

export function fixGap(state: MemoryState, input: { id: string; file: string; line: number; guard: string; commit: string; root: string }): MemoryState {
  const located = parseGuard(input.guard)
  const absolute = path.resolve(input.root, located.file)
  const text = readSourceLine(absolute, located.line)
  if (text === null) throw new Error("guard line does not exist")
  const hash = lineHash(text)
  const gaps = state.gaps.filter((gap) => gap.id !== input.id)
  gaps.push({
    id: input.id,
    status: "fixed",
    file: input.file,
    line: input.line,
    guard: input.guard,
    fixedIn: input.commit,
    guardFile: located.file,
    guardLineNo: located.line,
    guardLine: text,
    guardHash: hash,
  })
  const decisions = state.decisions.filter((item) => item.id !== input.id)
  decisions.push({ id: input.id, file: located.file, line: located.line, hash, status: "fixed", reason: `guarded by ${input.guard}` })
  return { ...state, gaps, decisions }
}

export function revertGap(state: MemoryState, id: string): { state: MemoryState; text: string } {
  const gap = state.gaps.find((item) => item.id === id)
  if (!gap || !gap.fixedIn || !gap.guard) throw new Error("that id is not a fixed gap")
  const text = `fixed in ${gap.fixedIn}, guarded by ${gap.guard}; the guard broke`
  const gaps = state.gaps.map((item) => (item.id === id ? { ...item, status: "regression" as const } : item))
  return { state: { ...state, gaps }, text }
}

export function guardAllows(state: MemoryState, guard: string, message: string, root: string): { ok: boolean; summary: string } {
  const owners = state.gaps.filter((gap) => gap.guard === guard && (gap.status === "fixed" || gap.status === "regression"))
  if (owners.length === 0) return { ok: true, summary: `${guard} guards nothing.` }
  const owner = owners[0]
  if (guardChanges(message).has(guard)) return { ok: true, summary: `${guard} changed with a Guard-Change trailer.` }
  const file = owner.guardFile ? path.resolve(root, owner.guardFile) : ""
  const line = owner.guardLineNo && file ? readSourceLine(file, owner.guardLineNo) : null
  const weakened = line === null || (owner.guardLine !== null && line !== owner.guardLine)
  if (!weakened) return { ok: true, summary: `${guard} is intact.` }
  return { ok: false, summary: `Refusing to change ${guard}. It is the guard of ${owner.id}.` }
}

export function appendFinding(dir: string, finding: Finding) {
  if (!FINDING_STATUS.has(finding.status)) throw new Error("status must be open, fixed, equivalent, or wont-fix")
  if (!finding.reason || !finding.reason.trim()) throw new Error("a finding needs a reason")
  mkdirSync(dir, { recursive: true })
  appendFileSync(path.join(dir, "findings.jsonl"), `${JSON.stringify(finding)}\n`)
}

export function loadFindings(dir: string): Finding[] {
  const file = path.join(dir, "findings.jsonl")
  if (!existsSync(file)) return []
  return readFileSync(file, "utf8")
    .split(/\r?\n/)
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line) as Finding)
}

export type KillScore = {
  visible: { before: number; after: number }
  sealed: { before: number; after: number }
}

export function yardstickSentence(score: KillScore | null): string | null {
  if (!score) return null
  if (score.visible.after > score.visible.before && score.sealed.after <= score.sealed.before) return YARDSTICK
  return null
}

export type StatusView = {
  gaps: Gap[]
  rest: number
  recentFixes: Array<{ id: string; fixedIn: string | null; guard: string | null }>
  protectedGuards: string[]
  decisions: Array<Decision & { review: "current" | "stale" }>
  regressions: Array<{ id: string; text: string }>
  findings: Finding[]
  sealedCount: number
}

export function statusView(dir: string, root: string): StatusView {
  const state = loadState(dir)
  const sealed = new Set(loadSealed(dir))
  const hidden = (id: string) => sealed.has(id)
  const open = state.gaps.filter((gap) => gap.status === "open" && !hidden(gap.id))
  const shown = open.slice(0, 10)
  const fixes = state.gaps
    .filter((gap) => gap.status === "fixed" && !hidden(gap.id))
    .map((gap) => ({ id: gap.id, fixedIn: gap.fixedIn, guard: gap.guard }))
  const guards = [...new Set(state.gaps.filter((gap) => gap.guard && !hidden(gap.id)).map((gap) => gap.guard as string))].sort()
  const decisions = state.decisions
    .filter((item) => !hidden(item.id))
    .map((item) => {
      const current = readSourceLine(path.resolve(root, item.file), item.line)
      const review = current !== null && lineHash(current) === item.hash ? "current" as const : "stale" as const
      return { ...item, review }
    })
  const regressions = state.gaps
    .filter((gap) => gap.status === "regression" && !hidden(gap.id) && gap.fixedIn && gap.guard)
    .map((gap) => ({ id: gap.id, text: `fixed in ${gap.fixedIn}, guarded by ${gap.guard}; the guard broke` }))
  const findings = loadFindings(dir).filter((item) => !hidden(item.id)).slice(-10)
  return {
    gaps: shown,
    rest: Math.max(open.length - shown.length, 0),
    recentFixes: fixes,
    protectedGuards: guards,
    decisions,
    regressions,
    findings,
    sealedCount: sealed.size,
  }
}

export function renderPage(view: StatusView, summary: string): string {
  const lines = ["# PROBATIO", "", summary, "", "## Gaps"]
  if (view.gaps.length === 0) lines.push("None.")
  for (const gap of view.gaps) lines.push(`- ${gap.id} at ${gap.file}:${gap.line}`)
  if (view.rest > 0) lines.push(`- ${view.rest} more`)
  lines.push("", "## Recent fixes")
  if (view.recentFixes.length === 0) lines.push("None.")
  for (const fix of view.recentFixes) lines.push(`- ${fix.id} fixed in ${fix.fixedIn}, guard ${fix.guard}`)
  lines.push("", "## Protected guards")
  if (view.protectedGuards.length === 0) lines.push("None.")
  for (const guard of view.protectedGuards) lines.push(`- ${guard}`)
  lines.push("", "## Decisions")
  if (view.decisions.length === 0) lines.push("None.")
  for (const decision of view.decisions) lines.push(`- ${decision.id} ${decision.status} ${decision.review} ${decision.reason}`)
  lines.push("", "## Regressions")
  if (view.regressions.length === 0) lines.push("None.")
  for (const item of view.regressions) lines.push(`- ${item.id}: ${item.text}`)
  lines.push("")
  return lines.join("\n")
}

export function statusEnvelope(dir: string, root: string, score: KillScore | null, agent: string | null): { envelope: Envelope; page: string } {
  const view = statusView(dir, root)
  const yard = yardstickSentence(score)
  const summary = yard ?? `${view.gaps.length} open gaps, ${view.recentFixes.length} fixes, ${view.decisions.length} decisions.`
  const first = view.regressions[0]?.text ?? (view.gaps[0] ? `${view.gaps[0].id} at ${view.gaps[0].file}:${view.gaps[0].line}` : "Nothing waiting.")
  const page = renderPage(view, summary)
  return {
    page,
    envelope: {
      schemaVersion: SCHEMA_VERSION,
      ok: true,
      command: "status",
      summary,
      next: first,
      nextCall: null,
      agent: agent ?? "unknown",
      ...view,
    },
  }
}
