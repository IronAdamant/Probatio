import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs"
import path from "node:path"
import { loadSealed } from "../memory/state.js"

export type Claim = {
  id: string
  agent?: string
  leasedUntil?: number
}

export function seedQueue(root: string, ids: string[]): string[] {
  const sealed = new Set(loadSealed(root))
  const queue = path.join(root, "queue")
  mkdirSync(queue, { recursive: true })
  const written: string[] = []
  for (const id of [...ids].sort()) {
    if (sealed.has(id)) continue
    const dest = path.join(queue, `${id}.json`)
    if (existsSync(dest)) continue
    writeFileSync(dest, `${JSON.stringify({ id })}\n`)
    written.push(id)
  }
  return written
}

/** Claim by atomic rename. A second claim fails because the queue file is already gone. */
export function claimItem(root: string, id: string, agent: string, now: number, leaseMs: number): { ok: boolean; summary: string; file: string | null } {
  const source = path.join(root, "queue", `${id}.json`)
  const claimed = path.join(root, "claimed")
  const dest = path.join(claimed, `${agent}-${id}.json`)
  if (!existsSync(source)) return { ok: false, summary: `${id} is not in the queue.`, file: null }
  mkdirSync(claimed, { recursive: true })
  const body = JSON.parse(readFileSync(source, "utf8")) as Claim
  body.id = id
  body.agent = agent
  body.leasedUntil = now + leaseMs
  writeFileSync(source, `${JSON.stringify(body)}\n`)
  try {
    renameSync(source, dest)
  } catch {
    return { ok: false, summary: `${id} was claimed by someone else.`, file: null }
  }
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
    if (!body.id) continue
    renameSync(file, path.join(queue, `${body.id}.json`))
    returned.push(body.id)
  }
  return returned.sort()
}
