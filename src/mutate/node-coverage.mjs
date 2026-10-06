// Baseline-only per-test line map for node --test. Loaded with --import in each
// test-file process. Writes one shard; the runner merges shards after the suite.
// takePreciseCoverage resets counters, so each sample is only what ran since the last take.
// A plain-script child does not run these hooks. It may leave a nameless hit list in
// ${map}.children. afterEach stores that list under the parent test before the shard is written.
import { randomBytes } from "node:crypto"
import { existsSync, mkdirSync, readdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs"
import { after, afterEach, before } from "node:test"
import path from "node:path"
import { openLineSampler } from "./precise-lines.mjs"

const mapPath = process.env.PROBATIO_COVERAGE_MAP
if (mapPath) {
  let sampler = null
  try {
    sampler = await openLineSampler((rel) => rel.endsWith("node-coverage.mjs") || rel.endsWith("precise-lines.mjs") || rel.endsWith("child-lines.mjs"))
  } catch (err) {
    writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
  }

  if (sampler) {
    /** @type {Map<string, Map<number, Set<string>>>} */
    const byFile = new Map()
    /** @type {Array<{ file: string, line: number }>} */
    let prelude = []

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

    function takeChildHits() {
      const dir = `${mapPath}.children`
      if (!existsSync(dir)) return []
      /** @type {Array<{ file: string, line: number }>} */
      const hits = []
      for (const name of readdirSync(dir)) {
        if (!name.endsWith(".json")) continue
        const full = path.join(dir, name)
        try {
          const parsed = JSON.parse(readFileSync(full, "utf8"))
          const list = parsed && Array.isArray(parsed.hits) ? parsed.hits : []
          for (const hit of list) {
            if (!hit || typeof hit.file !== "string" || !Number.isInteger(hit.line)) continue
            hits.push({ file: hit.file, line: hit.line })
          }
        } catch {
          // A torn dump is ignored. The parent does not invent a line.
        }
        try {
          unlinkSync(full)
        } catch {
          // The next test must not inherit this dump.
        }
      }
      return hits
    }

    before(async () => {
      try {
        prelude = await sampler.positiveLines()
      } catch (err) {
        writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
      }
    })

    afterEach(async (t) => {
      try {
        add(prelude, t.name)
        add(await sampler.positiveLines(), t.name)
        add(takeChildHits(), t.name)
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
