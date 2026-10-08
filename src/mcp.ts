#!/usr/bin/env node
import { spawn, type ChildProcess } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { nodeTooOld } from "./node-version.js"

const tooOld = nodeTooOld(process.versions.node)
if (tooOld) {
  process.stdout.write(
    `${JSON.stringify({
      schemaVersion: 2,
      ok: false,
      command: "mcp",
      summary: tooOld,
      next: "Install Node 22.6 or newer and run the command again.",
      nextCall: null,
    })}\n`,
  )
  process.exit(1)
}

type Request = {
  jsonrpc?: string
  id?: number | string | null
  method?: string
  params?: { name?: string; arguments?: { argv?: string[] }; requestId?: number | string; protocolVersion?: string }
}

/** Protocol versions this server speaks. The client's choice wins when it is one of these. */
const PROTOCOLS = ["2024-11-05", "2025-03-26", "2025-06-18"]

const here = path.dirname(fileURLToPath(import.meta.url))
const version = readVersion()
const tool = {
  name: "probatio",
  description: "Run one probatio command. argv is the CLI words and flags.",
  inputSchema: {
    type: "object",
    properties: { argv: { type: "array", items: { type: "string" } } },
    required: ["argv"],
  },
}

let buffer = ""
let closing = false
/** Tool calls still running, by request id. A cancel kills the child and drops the reply. */
const running = new Map<string, ChildProcess>()

process.stdin.setEncoding("utf8")
process.stdin.on("data", (chunk: string) => {
  buffer += chunk
  drain()
})
process.stdin.on("end", () => {
  drain(true)
  closing = true
  // A call already running still gets its reply before the server exits.
  if (running.size === 0) process.exit(0)
})

function drain(flush = false) {
  const lines = buffer.split("\n")
  buffer = flush ? "" : lines.pop() ?? ""
  for (const line of lines) {
    const text = line.replace(/\r$/, "").trim()
    if (!text) continue
    let message: Request
    try {
      message = JSON.parse(text) as Request
    } catch {
      continue
    }
    handle(message)
  }
}

function handle(message: Request) {
  if (!message.method) return
  if (message.method === "notifications/cancelled") {
    const key = message.params?.requestId
    if (key === undefined) return
    const child = running.get(String(key))
    running.delete(String(key))
    if (child?.pid) {
      try {
        process.kill(-child.pid, "SIGTERM")
      } catch {
        child.kill("SIGTERM")
      }
    }
    return
  }
  if (message.id === undefined || message.id === null) return
  if (message.method === "initialize") {
    send({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        protocolVersion: PROTOCOLS.includes(message.params?.protocolVersion ?? "") ? message.params?.protocolVersion : PROTOCOLS[0],
        capabilities: { tools: {} },
        serverInfo: { name: "probatio", version },
        tools: [tool],
      },
    })
    return
  }
  if (message.method === "tools/list") {
    send({ jsonrpc: "2.0", id: message.id, result: { tools: [tool] } })
    return
  }
  if (message.method === "ping") {
    send({ jsonrpc: "2.0", id: message.id, result: {} })
    return
  }
  if (message.method === "tools/call") {
    const argv = message.params?.arguments?.argv
    if (!Array.isArray(argv) || argv.some((item) => typeof item !== "string")) {
      send({ jsonrpc: "2.0", id: message.id, error: { code: -32602, message: "argv must be an array of strings" } })
      return
    }
    call(message.id, argv)
    return
  }
  send({
    jsonrpc: "2.0",
    id: message.id,
    error: { code: -32601, message: `method not found: ${message.method}` },
  })
}

/** One CLI command per call. It runs beside other calls, so ping and cancel are answered meanwhile. */
function call(id: number | string, argv: string[]) {
  const key = String(id)
  const cli = cliCommand()
  // Its own process group, so a cancel also stops the suite the command started.
  const child = spawn(cli.bin, [...cli.args, ...argv], { stdio: ["ignore", "pipe", "ignore"], detached: true })
  running.set(key, child)
  let text = ""
  child.stdout?.setEncoding("utf8")
  child.stdout?.on("data", (chunk: string) => {
    text += chunk
  })
  const settle = (failure: string | null) => {
    // A cancelled call is no longer in the map. Its reply is dropped.
    const live = running.get(key) === child
    running.delete(key)
    if (live) reply(id, text, failure)
    if (closing && running.size === 0) process.exit(0)
  }
  child.on("error", (error) => settle(error.message))
  child.on("close", () => settle(null))
}

function reply(id: number | string, text: string, failure: string | null) {
  let envelope: { ok?: boolean } | null = null
  try {
    envelope = JSON.parse(text) as { ok?: boolean }
  } catch {
    envelope = null
  }
  if (!envelope || typeof envelope !== "object") {
    const message = failure ? `the command did not start: ${failure}` : "the command did not return one JSON object"
    send({ jsonrpc: "2.0", id, error: { code: -32000, message } })
    return
  }
  send({
    jsonrpc: "2.0",
    id,
    result: {
      content: [{ type: "text", text: text.trim() }],
      structuredContent: envelope,
      isError: envelope.ok !== true,
    },
  })
}

function cliCommand(): { bin: string; args: string[] } {
  const compiled = path.join(here, "cli.js")
  if (existsSync(compiled)) return { bin: process.execPath, args: [compiled] }
  const tsx = path.resolve(here, "..", "node_modules", ".bin", "tsx")
  return { bin: tsx, args: [path.join(here, "cli.ts")] }
}

function readVersion(): string {
  try {
    const raw = JSON.parse(readFileSync(path.resolve(here, "..", "package.json"), "utf8")) as { version?: string }
    return typeof raw.version === "string" && raw.version.length > 0 ? raw.version : "0.0.1"
  } catch {
    return "0.0.1"
  }
}

function send(message: unknown) {
  const line = JSON.stringify(message)
  process.stdout.write(`${line}\n`)
}
