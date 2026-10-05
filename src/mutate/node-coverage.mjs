// Baseline-only per-test line map for node --test. Loaded with --import in each
// test-file process. Writes one shard; the runner merges shards after the suite.
// takePreciseCoverage resets counters, so each sample is only what ran since the last take.
import { randomBytes } from "node:crypto"
import { mkdirSync, writeFileSync } from "node:fs"
import { Session } from "node:inspector/promises"
import { findSourceMap } from "node:module"
import { after, afterEach, before } from "node:test"
import path from "node:path"
import { fileURLToPath } from "node:url"

const B64 = new Int32Array(128)
for (let i = 0; i < 64; i++) B64["ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".charCodeAt(i)] = i

function decodeSegments(row) {
  const segments = []
  let i = 0
  while (i < row.length) {
    const fields = []
    while (i < row.length && row.charCodeAt(i) !== 44) {
      let value = 0
      let shift = 0
      let digit = 0
      do {
        digit = B64[row.charCodeAt(i++)]
        value |= (digit & 31) << shift
        shift += 5
      } while (digit & 32)
      fields.push(value & 1 ? -(value >> 1) : value >> 1)
    }
    if (row.charCodeAt(i) === 44) i++
    if (fields.length > 0) segments.push(fields)
  }
  return segments
}

const mapPath = process.env.PROBATIO_COVERAGE_MAP
if (mapPath) {
  const session = new Session()
  let started = false
  try {
    session.connect()
    await session.post("Profiler.enable")
    await session.post("Debugger.enable")
    await session.post("Profiler.startPreciseCoverage", { callCount: true, detailed: true })
    started = true
  } catch (err) {
    writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
  }

  if (started) {
    const textCache = new Map()
    const indexCache = new Map()
    /** @type {Map<string, Map<number, Set<string>>>} */
    const byFile = new Map()
    /** @type {Array<{ file: string, line: number }>} */
    let prelude = []

    async function scriptText(script) {
      if (textCache.has(script.scriptId)) return textCache.get(script.scriptId)
      let text = ""
      try {
        const got = await session.post("Debugger.getScriptSource", { scriptId: script.scriptId })
        text = got.scriptSource || ""
      } catch {
        text = ""
      }
      textCache.set(script.scriptId, text)
      return text
    }

    function relPath(url) {
      if (!url || url.startsWith("node:")) return null
      let abs
      try {
        abs = fileURLToPath(url.split("?")[0].split("#")[0])
      } catch {
        return null
      }
      const rel = path.relative(process.cwd(), abs).split(path.sep).join("/")
      if (!rel || rel.startsWith("..") || path.isAbsolute(rel)) return null
      if (rel.split("/").includes("node_modules")) return null
      if (rel.endsWith("node-coverage.mjs")) return null
      return rel
    }

    function indexFor(scriptId, text, sm) {
      if (indexCache.has(scriptId)) return indexCache.get(scriptId)
      const mapped = sm && sm.payload && typeof sm.payload.mappings === "string" ? linesFromMappings(text, sm.payload.mappings) : null
      const lines = mapped && hasLine(mapped) ? mapped : linesFromNewlines(text, sm)
      indexCache.set(scriptId, lines)
      return lines
    }

    function hasLine(lines) {
      for (let i = 0; i < lines.length; i++) if (lines[i] > 0) return true
      return false
    }

    function linesFromNewlines(text, sm) {
      const lines = new Int32Array(text.length + 1)
      if (!sm) {
        let line = 1
        for (let i = 0; i < text.length; i++) {
          lines[i] = line
          if (text.charCodeAt(i) === 10) line++
        }
        return lines
      }
      // Node 22 reports SourceMap.findEntry originalLine as 0-based.
      let genLine = 0
      let col = 0
      for (let i = 0; i < text.length; i++) {
        const entry = sm.findEntry(genLine, col)
        if (entry && Number.isInteger(entry.originalLine) && entry.originalLine >= 0) lines[i] = entry.originalLine + 1
        if (text.charCodeAt(i) === 10) {
          genLine++
          col = 0
        } else {
          col++
        }
      }
      return lines
    }

    function linesFromMappings(text, mappings) {
      const lines = new Int32Array(text.length + 1)
      const starts = [0]
      for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) starts.push(i + 1)
      let carryLine = 0
      const rows = mappings.split(";")
      for (let genLine = 0; genLine < rows.length && genLine < starts.length; genLine++) {
        const start = starts[genLine]
        const next = genLine + 1 < starts.length ? starts[genLine + 1] : text.length
        let col = 0
        let cursor = start
        let active = -1
        for (const fields of decodeSegments(rows[genLine])) {
          col += fields[0]
          const at = Math.min(next, start + Math.max(0, col))
          if (active >= 0) {
            for (let i = cursor; i < at; i++) lines[i] = active + 1
          }
          cursor = at
          if (fields.length >= 4) {
            carryLine += fields[2]
            active = carryLine
          } else {
            active = -1
          }
        }
        if (active >= 0) {
          for (let i = cursor; i < next; i++) lines[i] = active + 1
        }
      }
      return lines
    }

    function applyRanges(text, script, lines) {
      const ranges = []
      for (const fn of script.functions || []) {
        for (const range of fn.ranges || []) ranges.push(range)
      }
      // Outer ranges first. A nested block then overwrites its own offsets, so a dead
      // branch stays at 0 while a condition earlier on that same line can stay covered.
      ranges.sort((a, b) => a.startOffset - b.startOffset || b.endOffset - a.endOffset)
      const offsets = new Int32Array(text.length)
      for (const range of ranges) {
        const end = Math.min(text.length, range.endOffset)
        for (let i = Math.max(0, range.startOffset); i < end; i++) offsets[i] = range.count
      }
      const counts = new Map()
      for (let i = 0; i < text.length; i++) {
        const line = lines[i]
        if (line <= 0 || offsets[i] <= 0) continue
        if (offsets[i] > (counts.get(line) || 0)) counts.set(line, offsets[i])
      }
      return counts
    }

    async function positiveLines() {
      const cov = await session.post("Profiler.takePreciseCoverage")
      /** @type {Array<{ file: string, line: number }>} */
      const found = []
      for (const script of cov.result || []) {
        const file = relPath(script.url)
        if (!file) continue
        const text = await scriptText(script)
        if (!text) continue
        const counts = applyRanges(text, script, indexFor(script.scriptId, text, findSourceMap(script.url)))
        for (const [line, count] of counts) {
          if (count > 0) found.push({ file, line })
        }
      }
      return found
    }

    function add(hits, name) {
      if (!name) return
      for (const hit of hits) {
        let lines = byFile.get(hit.file)
        if (!lines) {
          lines = new Map()
          byFile.set(hit.file, lines)
        }
        let names = lines.get(hit.line)
        if (!names) {
          names = new Set()
          lines.set(hit.line, names)
        }
        names.add(name)
      }
    }

    before(async () => {
      try {
        prelude = await positiveLines()
      } catch (err) {
        writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
      }
    })

    afterEach(async (t) => {
      try {
        add(prelude, t.name)
        add(await positiveLines(), t.name)
      } catch (err) {
        writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
      }
    })

    after(() => {
      if (byFile.size === 0) return
      const files = {}
      for (const [file, lines] of byFile) {
        const bucket = {}
        for (const [line, names] of lines) bucket[String(line)] = [...names].sort()
        files[file] = bucket
      }
      const dir = `${mapPath}.parts`
      mkdirSync(dir, { recursive: true })
      writeFileSync(path.join(dir, `${process.pid}-${randomBytes(4).toString("hex")}.json`), JSON.stringify({ files }))
    })
  }
}
