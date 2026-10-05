#!/usr/bin/env node
import { spawnSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

type Request = {
  jsonrpc?: string
  id?: number | string | null
  method?: string
  params?: { name?: string; arguments?: { argv?: string[] } }
}

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

process.stdin.setEncoding("utf8")
process.stdin.on("data", (chunk: string) => {
  buffer += chunk
  drain()
})
process.stdin.on("end", () => {
  drain(true)
  process.exit(0)
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
  if (message.id === undefined || message.id === null) return
  if (message.method === "initialize") {
    send({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        protocolVersion: "2024-11-05",
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
  if (message.method === "tools/call") {
    const argv = message.params?.arguments?.argv
    if (!Array.isArray(argv) || argv.some((item) => typeof item !== "string")) {
      send({ jsonrpc: "2.0", id: message.id, error: { code: -32602, message: "argv must be an array of strings" } })
      return
    }
    const cli = cliCommand()
    const child = spawnSync(cli.bin, [...cli.args, ...argv], { encoding: "utf8" })
    const text = child.stdout ?? ""
    let envelope: { ok?: boolean } | null = null
    try {
      envelope = JSON.parse(text) as { ok?: boolean }
    } catch {
      envelope = null
    }
    if (!envelope || typeof envelope !== "object") {
      send({ jsonrpc: "2.0", id: message.id, error: { code: -32000, message: "the command did not return one JSON object" } })
      return
    }
    send({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        content: [{ type: "text", text: text.trim() }],
        structuredContent: envelope,
        isError: envelope.ok !== true,
      },
    })
  }
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
