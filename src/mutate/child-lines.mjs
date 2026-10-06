// Nameless line dump for a plain script. This is not a node:test process and it
// does not write a coverage shard. The parent test's afterEach reads the dump.
import { randomBytes } from "node:crypto"
import { mkdirSync, writeFileSync } from "node:fs"
import path from "node:path"
import { openLineSampler } from "./precise-lines.mjs"

const mapPath = process.env.PROBATIO_COVERAGE_MAP
if (mapPath) {
  let sampler = null
  try {
    sampler = await openLineSampler((rel) => rel.endsWith("child-lines.mjs") || rel.endsWith("precise-lines.mjs"))
  } catch (err) {
    writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
  }
  if (sampler) {
    let wrote = false
    const dump = () => {
      if (wrote) return
      wrote = true
      sampler.positiveLines().then((hits) => {
        const dir = `${mapPath}.children`
        mkdirSync(dir, { recursive: true })
        writeFileSync(
          path.join(dir, `${process.pid}-${randomBytes(4).toString("hex")}.json`),
          JSON.stringify({ hits }),
        )
      }).catch((err) => {
        writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
      })
    }
    process.on("beforeExit", dump)
  }
}
