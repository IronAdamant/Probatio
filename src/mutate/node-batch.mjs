// One suite process for covered mutants of one file. Jobs arrive on stdin.
// A mutant that exits this process ends it. The parent starts a clean process for the next mutant.
import { createInterface } from "node:readline"
import { realpathSync, writeFileSync } from "node:fs"
import { register } from "node:module"
import { run } from "node:test"
import path from "node:path"
import { openLineSampler } from "./precise-lines.mjs"

const loader = new URL("./node-batch-loader.mjs", import.meta.url).href
register(loader, { parentURL: import.meta.url })

let sampler = null
try {
  sampler = await openLineSampler()
} catch {
  sampler = null
}

writeFileSync(process.env.PB_READY, `${JSON.stringify({ pid: process.pid })}\n`)

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

let generation = 0

async function runJob(job) {
  generation += 1
  writeFileSync(process.env.PB_GEN, String(generation))
  if (sampler) await sampler.reset()
  const files = (job.tests || []).map((rel) => path.join(job.root, rel))
  const patterns = (job.names || []).filter((name) => name.length > 0).map((name) => new RegExp(`^${escapeRegExp(name)}$`))
  const failed = []
  const names = []
  const testTimeout = job.testTimeoutMs > 0 ? job.testTimeoutMs : job.timeoutMs
  const stream = run({
    files,
    isolation: "none",
    timeout: testTimeout,
    ...(patterns.length > 0 ? { testNamePatterns: patterns } : {}),
  })
  let lastEvent = Date.now()
  const started = Date.now()
  let ended = false
  let timer = null
  const collected = (async () => {
    for await (const event of stream) {
      lastEvent = Date.now()
      if (event.type !== "test:pass" && event.type !== "test:fail") continue
      const data = event.data || {}
      const name = typeof data.name === "string" ? data.name : ""
      if (!name || files.includes(name)) continue
      if (!names.includes(name)) names.push(name)
      if (event.type === "test:fail") {
        const error = data.details && data.details.error
        const text = error ? `${error.failureType || ""} ${error.message || ""} ${error.cause || ""}` : ""
        const fileTimeout = /testTimeoutFailure|timed out/i.test(text) && fileNameIs(name, data.file || "")
        failed.push({ name, file: data.file || "", line: data.line || 0, ...(fileTimeout ? { fileTimeout: true } : {}) })
      }
    }
    ended = true
  })()
  let dirty = false
  try {
    dirty = await Promise.race([
      collected.then(() => false),
      new Promise((resolve) => {
        timer = setInterval(() => {
          if (ended) {
            resolve(false)
            return
          }
          const idle = Date.now() - lastEvent
          const overdue = Date.now() - started >= testTimeout && idle >= 400 && names.length > 0
          if (!overdue) return
          resolve(true)
        }, 50)
        timer.unref()
      }),
    ])
  } finally {
    if (timer) clearInterval(timer)
  }
  if (dirty && failed.length === 0) {
    for (const rel of job.tests || []) {
      const posix = rel.split(path.sep).join("/")
      let file = path.join(job.root, rel)
      try {
        file = realpathSync(file)
      } catch {
        // The reporter uses the path it opened. A missing file keeps the joined path.
      }
      failed.push({ name: posix, file, line: 1, fileTimeout: true })
      if (!names.includes(posix)) names.push(posix)
    }
  }
  if (sampler && !dirty) await sampler.reset()
  const pattern = (job.names || []).join("|")
  const command = ["node", "--test", ...(pattern ? [`--test-name-pattern=${pattern}`] : []), ...(job.tests || [])].join(" ")
  return { id: job.id, pid: process.pid, failed, names, command, dirty }
}

function fileNameIs(name, file) {
  const base = String(file).split(/[/\\]/).pop()
  if (!base) return false
  return name === file || name === base || name.endsWith(`/${base}`) || name.endsWith(`\\${base}`)
}

const lines = createInterface({ input: process.stdin })
for await (const line of lines) {
  if (!line.trim()) continue
  const job = JSON.parse(line)
  // node:test with isolation "none" waits until the event loop is idle.
  // An open readline on stdin is that handle, so the job would never finish.
  lines.pause()
  process.stdin.unref()
  try {
    const reply = await runJob(job)
    writeFileSync(job.replyFile, `${JSON.stringify(reply)}\n`)
    // An open handle after the tests finished is the same file timeout node --test reports.
    // The process is dirty, so it ends and the next mutant starts clean.
    if (reply.dirty) process.exit(0)
  } catch (err) {
    writeFileSync(job.replyFile, `${JSON.stringify({ id: job.id, pid: process.pid, failed: [], names: [], error: err instanceof Error ? err.message : String(err) })}\n`)
  } finally {
    if (!process.stdin.destroyed) process.stdin.ref()
    lines.resume()
  }
}
