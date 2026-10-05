import { homedir } from "node:os"

export const SCHEMA_VERSION = 1

export type NextCall = { argv: string[] } | null

export type Envelope = {
  schemaVersion: number
  ok: boolean
  command: string
  summary: string
  next: string
  nextCall: NextCall
  [key: string]: unknown
}

/** Replace the home directory so a result never carries a user's name or home path. */
export function scrub(value: unknown, home = homedir()): unknown {
  if (typeof value === "string") {
    if (!home || !value.includes(home)) return value
    return value.split(home).join("~")
  }
  if (Array.isArray(value)) return value.map((item) => scrub(item, home))
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {}
    for (const [key, item] of Object.entries(value)) out[key] = scrub(item, home)
    return out
  }
  return value
}

export function render(envelope: Envelope, human: boolean, home = homedir()): string {
  const clean = scrub(envelope, home) as Envelope
  if (!human) return `${JSON.stringify(clean, null, 2)}\n`
  const lines = [clean.summary, clean.next]
  const call = clean.nextCall
  if (call && Array.isArray(call.argv)) lines.push(`next call: probatio ${call.argv.join(" ")}`)
  return `${lines.filter((line) => line.length > 0).join("\n")}\n`
}
