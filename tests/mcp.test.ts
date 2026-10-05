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
    assert.deepEqual(first, JSON.parse(cli.stdout))
    assert.deepEqual(second, first)
    assert.equal(JSON.stringify(first).includes(homedir()), false)
    assert.equal(typeof first.schemaVersion, "number")
    assert.equal(first.ok, true)
    assert.equal(typeof first.next, "string")
    assert.ok("nextCall" in first)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function callMcp(argv: string[]): Promise<{ schemaVersion: number; ok: boolean; next: string; nextCall: unknown }> {
  return new Promise((resolve, reject) => {
    const child = spawn(tsx, ["src/mcp.ts"], { cwd: root, stdio: ["pipe", "pipe", "pipe"] })
    let buf = Buffer.alloc(0)
    const frames: string[] = []
    const timer = setTimeout(() => {
      child.kill("SIGKILL")
      reject(new Error(`mcp timed out: ${frames.length} frames`))
    }, 15_000)
    child.stdout.on("data", (chunk: Buffer) => {
      buf = Buffer.concat([buf, chunk])
      for (;;) {
        const headerEnd = buf.indexOf("\r\n\r\n")
        if (headerEnd === -1) return
        const header = buf.subarray(0, headerEnd).toString("utf8")
        const match = header.match(/Content-Length:\s*(\d+)/i)
        if (!match) return
        const length = Number(match[1])
        const start = headerEnd + 4
        if (buf.length < start + length) return
        frames.push(buf.subarray(start, start + length).toString("utf8"))
        buf = buf.subarray(start + length)
        if (frames.length < 2) continue
        clearTimeout(timer)
        child.stdin.end()
        const call = JSON.parse(frames[1]) as { result?: { content?: Array<{ text?: string }> } }
        const text = call.result?.content?.[0]?.text
        if (!text) {
          reject(new Error("mcp result had no text"))
          return
        }
        resolve(JSON.parse(text) as { schemaVersion: number; ok: boolean; next: string; nextCall: unknown })
        return
      }
    })
    child.on("error", (error) => {
      clearTimeout(timer)
      reject(error)
    })
    child.stdin.write(frame({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05", capabilities: {} } }))
    child.stdin.write(frame({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "probatio", arguments: { argv } } }))
  })
}

function frame(message: unknown): Buffer {
  const payload = Buffer.from(JSON.stringify(message), "utf8")
  return Buffer.concat([Buffer.from(`Content-Length: ${payload.length}\r\n\r\n`, "utf8"), payload])
}
