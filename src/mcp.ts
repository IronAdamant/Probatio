#!/usr/bin/env node
import { spawnSync } from "node:child_process"
import { existsSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

type Request = {
  jsonrpc?: string
  id?: number | string | null
  method?: string
  params?: { name?: string; arguments?: { argv?: string[] } }
}

const here = path.dirname(fileURLToPath(import.meta.url))
let buffer = Buffer.alloc(0)

process.stdin.on("data", (chunk: Buffer) => {
  buffer = Buffer.concat([buffer, chunk])
  drain()
})
process.stdin.on("end", () => process.exit(0))

function drain() {
  for (;;) {
    const headerEnd = buffer.indexOf("\r\n\r\n")
    if (headerEnd === -1) return
    const header = buffer.subarray(0, headerEnd).toString("utf8")
    const match = header.match(/Content-Length:\s*(\d+)/i)
    const start = headerEnd + 4
    if (!match) {
      buffer = buffer.subarray(start)
      continue
    }
    const length = Number(match[1])
    if (buffer.length < start + length) return
    const body = buffer.subarray(start, start + length).toString("utf8")
    buffer = buffer.subarray(start + length)
    let message: Request
    try {
      message = JSON.parse(body) as Request
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
        serverInfo: { name: "probatio", version: "0.0.1" },
      },
    })
    return
  }
  if (message.method === "tools/list") {
    send({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        tools: [
          {
            name: "probatio",
            description: "Run one probatio command. argv is the CLI words and flags.",
            inputSchema: {
              type: "object",
              properties: { argv: { type: "array", items: { type: "string" } } },
              required: ["argv"],
            },
          },
        ],
      },
    })
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

function send(message: unknown) {
  const payload = Buffer.from(JSON.stringify(message), "utf8")
  process.stdout.write(`Content-Length: ${payload.length}\r\n\r\n`)
  process.stdout.write(payload)
}
