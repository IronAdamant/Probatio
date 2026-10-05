import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { homedir, tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("the mcp tool returns the same json object as the cli", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-mcp-"))
  const recorded = path.join(dir, "recorded.json")
  const table = { rows: { gate: { ok: true, reason: "opened", next: "Run the next check.", nextCall: { tool: "probatio" } } } }
  writeFileSync(recorded, `${JSON.stringify(table, null, 2)}\n`)
  const argv = ["golden", "check", "--recorded", recorded, "--actual", recorded]
  try {
    const cli = spawnSync(tsx, ["src/cli.ts", ...argv], { cwd: root, encoding: "utf8" })
    assert.equal(cli.status, 0, cli.stderr)
    const first = await callMcp(argv)
    const second = await callMcp(argv)
    assert.deepEqual(first.envelope, JSON.parse(cli.stdout))
    assert.deepEqual(second.envelope, first.envelope)
    assert.equal(JSON.stringify(first.envelope).includes(homedir()), false)
    assert.equal(typeof first.envelope.schemaVersion, "number")
    assert.equal(first.envelope.ok, true)
    assert.equal(typeof first.envelope.summary, "string")
    assert.equal(typeof first.envelope.next, "string")
    assert.ok("nextCall" in first.envelope)
    assert.equal(first.handshake.protocolVersion, "2024-11-05")
    assert.ok(first.handshake.capabilities && "tools" in first.handshake.capabilities)
    assert.deepEqual(first.handshake.tools?.map((item) => item.name), ["probatio"])
    assert.equal(first.raw.includes("Content-Length"), false)
    assert.equal(second.raw.includes("Content-Length"), false)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

type Handshake = {
  protocolVersion?: string
  capabilities?: { tools?: unknown }
  tools?: Array<{ name?: string }>
}

function callMcp(argv: string[]): Promise<{ envelope: { schemaVersion: number; ok: boolean; summary: string; next: string; nextCall: unknown }; handshake: Handshake; raw: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(tsx, ["src/mcp.ts"], { cwd: root, stdio: ["pipe", "pipe", "pipe"] })
    let pending = ""
    let raw = ""
    const frames: string[] = []
    const timer = setTimeout(() => {
      child.kill("SIGKILL")
      reject(new Error(`mcp timed out: ${frames.length} frames`))
    }, 15_000)
    child.stdout.on("data", (chunk: Buffer) => {
      const text = chunk.toString("utf8")
      raw += text
      pending += text
      for (;;) {
        const nl = pending.indexOf("\n")
        if (nl === -1) return
        const line = pending.slice(0, nl).replace(/\r$/, "").trim()
        pending = pending.slice(nl + 1)
        if (!line) continue
        frames.push(line)
        if (frames.length < 2) continue
        clearTimeout(timer)
        child.stdin.end()
        const hello = JSON.parse(frames[0]) as { result?: Handshake }
        const call = JSON.parse(frames[1]) as { result?: { content?: Array<{ text?: string }> } }
        const body = call.result?.content?.[0]?.text
        if (!hello.result || !body) {
          reject(new Error("mcp result had no handshake or text"))
          return
        }
        resolve({
          handshake: hello.result,
          envelope: JSON.parse(body) as { schemaVersion: number; ok: boolean; summary: string; next: string; nextCall: unknown },
          raw,
        })
        return
      }
    })
    child.on("error", (error) => {
      clearTimeout(timer)
      reject(error)
    })
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05", capabilities: {} } })}\n`)
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "probatio", arguments: { argv } } })}\n`)
  })
}
