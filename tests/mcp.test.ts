import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { homedir, tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("the mcp tool returns the same json object as the cli", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-mcp-"))
  const recorded = path.join(dir, "recorded.json")
  const table = { rows: { gate: { ok: true, reason: "opened", next: "Run the next check.", nextCall: { tool: "probatio" } } } }
  writeFileSync(recorded, `${JSON.stringify(table, null, 2)}\n`)
  const argv = ["golden", "check", "--recorded", recorded, "--actual", recorded]
  try {
    const cli = spawnSync(tsx, ["src/cli.ts", ...argv], { cwd: root, encoding: "utf8", timeout: 300_000 })
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

test("mcp answers ping and rejects an unknown method", async () => {
  const child = spawn(tsx, ["src/mcp.ts"], { cwd: root, stdio: ["pipe", "pipe", "pipe"] })
  const frames: Array<{ id?: number; result?: unknown; error?: { code?: number; message?: string } }> = []
  const pending = { text: "" }
  const done = new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill("SIGKILL")
      reject(new Error(`mcp timed out with ${frames.length} frames`))
    }, 10_000)
    child.stdout.on("data", (chunk: Buffer) => {
      pending.text += chunk.toString("utf8")
      for (;;) {
        const nl = pending.text.indexOf("\n")
        if (nl === -1) return
        const line = pending.text.slice(0, nl).trim()
        pending.text = pending.text.slice(nl + 1)
        if (!line) continue
        frames.push(JSON.parse(line) as { id?: number; result?: unknown; error?: { code?: number; message?: string } })
        if (frames.length < 3) continue
        clearTimeout(timer)
        child.stdin.end()
        resolve()
      }
    })
    child.on("error", (error) => {
      clearTimeout(timer)
      reject(error)
    })
  })
  child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} })}\n`)
  child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 2, method: "ping" })}\n`)
  child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 7, method: "nope" })}\n`)
  await done
  assert.equal(frames[1]?.id, 2)
  assert.deepEqual(frames[1]?.result, {})
  assert.equal(frames[2]?.id, 7)
  assert.equal(frames[2]?.error?.code, -32601)
  assert.match(frames[2]?.error?.message ?? "", /nope/)
})

test("mcp answers ping while a tool call runs, and a cancelled call gets no reply", { timeout: 120_000 }, async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-mcp-slow-"))
  const before = "export function gate(n: number): boolean {\n  return n > 0\n}\n"
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  mkdirSync(path.join(dir, "patches"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "src", "gate.ts"), before)
  writeFileSync(
    path.join(dir, "tests", "gate.test.ts"),
    'import assert from "node:assert/strict"\nimport test from "node:test"\nimport { gate } from "../src/gate.ts"\ntest("slow shut", async () => { await new Promise((r) => setTimeout(r, 2500)); assert.equal(gate(0), false) })\n',
  )
  writeFileSync(path.join(dir, "patches", "m-ge.patch"), forwardDiff("src/gate.ts", before, before.replace(">", ">=")))
  for (const args of [["init", "-q"], ["add", "."], ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"]]) {
    spawnSync("git", ["-C", dir, ...args])
  }
  const runArgv = (out: string) => ["mutate", "run", "--package", dir, "--patches", path.join(dir, "patches"), "--out", path.join(dir, out), "--no-build", "--no-confirm"]
  const child = spawn(tsx, ["src/mcp.ts"], { cwd: root, stdio: ["pipe", "pipe", "pipe"] })
  const started = Date.now()
  const seen = new Map<number, number>()
  const bodies = new Map<number, { result?: { isError?: boolean } }>()
  try {
    const finished = new Promise<void>((resolve, reject) => {
      let pending = ""
      const timer = setTimeout(() => reject(new Error(`no reply to the tool call: ${JSON.stringify([...seen])}`)), 100_000)
      child.stdout.on("data", (chunk: Buffer) => {
        pending += chunk.toString("utf8")
        for (;;) {
          const nl = pending.indexOf("\n")
          if (nl === -1) return
          const line = pending.slice(0, nl).trim()
          pending = pending.slice(nl + 1)
          if (!line) continue
          const frame = JSON.parse(line) as { id?: number; result?: { isError?: boolean } }
          if (typeof frame.id === "number") {
            seen.set(frame.id, Date.now() - started)
            bodies.set(frame.id, frame)
          }
          if (frame.id === 2) {
            clearTimeout(timer)
            resolve()
          }
        }
      })
    })
    const send = (message: unknown) => child.stdin.write(`${JSON.stringify(message)}\n`)
    send({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} })
    send({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "probatio", arguments: { argv: runArgv("kept") } } })
    send({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "probatio", arguments: { argv: runArgv("cancelled") } } })
    await new Promise((resolve) => setTimeout(resolve, 300))
    send({ jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: 3, reason: "test" } })
    send({ jsonrpc: "2.0", id: 4, method: "ping" })
    await finished
    // Give a late reply to the cancelled call a moment to show up.
    await new Promise((resolve) => setTimeout(resolve, 500))
    assert.ok(seen.has(4), "ping was answered")
    assert.ok((seen.get(4) ?? Infinity) < (seen.get(2) ?? 0), `ping waited for the tool call: ${JSON.stringify([...seen])}`)
    assert.equal(bodies.get(2)?.result?.isError, false, JSON.stringify(bodies.get(2)))
    assert.equal(seen.has(3), false, "a cancelled request gets no reply")
  } finally {
    child.stdin.end()
    child.kill("SIGKILL")
    rmSync(dir, { recursive: true, force: true })
  }
})
