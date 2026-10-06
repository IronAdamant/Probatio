// Mocha root hooks. This file is not node-coverage.mjs. Mocha does not run
// node:test afterEach/after, so that collector records no Mocha title.
// These hooks key inspector hits by the current Mocha title and write { files }.
const { mkdirSync, writeFileSync } = require("node:fs")
const path = require("node:path")

const mapPath = process.env.PROBATIO_COVERAGE_MAP
if (mapPath) {
  /** @type {Map<string, Map<number, Set<string>>>} */
  const byFile = new Map()
  /** @type {Array<{ file: string, line: number }>} */
  let prelude = []
  /** @type {{ positiveLines: () => Promise<Array<{ file: string, line: number }>> } | null} */
  let sampler = null

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

  function titleOf(test) {
    if (!test) return ""
    if (typeof test.fullTitle === "function") return test.fullTitle()
    return typeof test.title === "string" ? test.title : ""
  }

  exports.mochaHooks = {
    async beforeAll() {
      try {
        const opened = await import("./precise-lines.mjs")
        sampler = await opened.openLineSampler((rel) => rel.endsWith("mocha-coverage.cjs") || rel.endsWith("precise-lines.mjs") || rel.endsWith("node-coverage.mjs"))
        prelude = await sampler.positiveLines()
      } catch (err) {
        writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
      }
    },
    async afterEach() {
      if (!sampler) return
      const title = titleOf(this.currentTest)
      try {
        add(prelude, title)
        add(await sampler.positiveLines(), title)
      } catch (err) {
        writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
      }
    },
    afterAll() {
      if (byFile.size === 0) return
      const files = {}
      for (const [file, lines] of byFile) {
        const bucket = {}
        for (const [line, names] of lines) bucket[String(line)] = [...names].sort()
        files[file] = bucket
      }
      mkdirSync(path.dirname(mapPath), { recursive: true })
      writeFileSync(mapPath, `${JSON.stringify({ files }, null, 2)}\n`)
    },
  }
}
