import assert from "node:assert/strict"
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { claimItem, seedQueue } from "../src/swarm/queue.ts"

test("a claim name cannot leave the queue directory", () => {
  const root = mkdtempSync(path.join(tmpdir(), "probatio-queue-"))
  const outside = path.join(root, "outside.json")
  try {
    assert.deepEqual(seedQueue(root, ["ok-1", "../outside"]), ["ok-1"])
    assert.equal(existsSync(outside), false)
    assert.equal(existsSync(path.join(root, "queue", "ok-1.json")), true)
    const bad = claimItem(root, "ok-1", "../outside", 1, 1000)
    assert.equal(bad.ok, false)
    assert.match(bad.summary, /file name/)
    assert.equal(existsSync(outside), false)
    assert.equal(existsSync(path.join(root, "queue", "ok-1.json")), true)
    const won = claimItem(root, "ok-1", "agent-a", 1, 1000)
    assert.equal(won.ok, true)
    const body = JSON.parse(readFileSync(won.file as string, "utf8")) as { agent: string; id: string }
    assert.equal(body.agent, "agent-a")
    assert.equal(body.id, "ok-1")
    const again = claimItem(root, "ok-1", "agent-b", 2, 1000)
    assert.equal(again.ok, false)
    assert.match(again.summary, /not in the queue/)
    const still = JSON.parse(readFileSync(won.file as string, "utf8")) as { agent: string }
    assert.equal(still.agent, "agent-a")
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
