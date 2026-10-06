import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs"
import path from "node:path"
import { loadSealed } from "../memory/state.js"

export type Claim = {
  id: string
  agent?: string
  leasedUntil?: number
}

/** One path segment. Rejects slashes and `..` so a claim cannot leave the queue directory. */
export function safeFilePart(value: string): string | null {
  if (!/^[A-Za-z0-9._-]+$/.test(value) || value === "." || value === "..") return null
  return value
}

export function seedQueue(root: string, ids: string[]): string[] {
  const sealed = new Set(loadSealed(root))
  const queue = path.join(root, "queue")
  mkdirSync(queue, { recursive: true })
  const written: string[] = []
  for (const id of [...ids].sort()) {
    if (sealed.has(id) || !safeFilePart(id)) continue
    const dest = path.join(queue, `${id}.json`)
    if (existsSync(dest)) continue
    writeFileSync(dest, `${JSON.stringify({ id })}\n`)
    written.push(id)
  }
  return written
}

/** Claim by atomic rename. A second claim fails because the queue file is already gone. */
export function claimItem(root: string, id: string, agent: string, now: number, leaseMs: number): { ok: boolean; summary: string; file: string | null } {
  if (!safeFilePart(id) || !safeFilePart(agent)) return { ok: false, summary: "agent and id must be a single file name.", file: null }
  const source = path.join(root, "queue", `${id}.json`)
  const claimed = path.join(root, "claimed")
  const dest = path.join(claimed, `${agent}-${id}.json`)
  if (!existsSync(source)) return { ok: false, summary: `${id} is not in the queue.`, file: null }
  mkdirSync(claimed, { recursive: true })
  // Rename first. Writing the body before the rename lets the other claim overwrite the winner.
  try {
    renameSync(source, dest)
  } catch {
    return { ok: false, summary: `${id} was claimed by someone else.`, file: null }
  }
  const body: Claim = { id, agent, leasedUntil: now + leaseMs }
  writeFileSync(dest, `${JSON.stringify(body)}\n`)
  return { ok: true, summary: `${agent} claimed ${id} until ${body.leasedUntil}.`, file: dest }
}

/** Move claims whose lease has expired back to the queue. */
export function reapClaims(root: string, now: number): string[] {
  const claimed = path.join(root, "claimed")
  const queue = path.join(root, "queue")
  if (!existsSync(claimed)) return []
  mkdirSync(queue, { recursive: true })
  const returned: string[] = []
  for (const name of readdirSync(claimed).sort()) {
    if (!name.endsWith(".json")) continue
    const file = path.join(claimed, name)
    const body = JSON.parse(readFileSync(file, "utf8")) as Claim
    if (typeof body.leasedUntil !== "number" || body.leasedUntil > now) continue
    if (!body.id || !safeFilePart(body.id)) continue
    renameSync(file, path.join(queue, `${body.id}.json`))
    returned.push(body.id)
  }
  return returned.sort()
}
