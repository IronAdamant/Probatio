// Nameless line dump for a node child of a test. This is not a node:test process and it
// does not write a coverage shard. The parent test's afterEach reads the dump.
// The test-process collector puts this file in NODE_OPTIONS, so the child needs no import.
// The dump is written on `exit`, which also runs after process.exit(), so it is synchronous.
import { randomBytes } from "node:crypto"
import { mkdirSync, writeFileSync } from "node:fs"
import path from "node:path"
import { openSyncLineSampler } from "./precise-lines.mjs"

const mapPath = process.env.PROBATIO_COVERAGE_MAP
if (mapPath && !globalThis.__probatioChildLines) {
  globalThis.__probatioChildLines = true
  let sampler = null
  try {
    sampler = openSyncLineSampler(
      (rel) => rel.endsWith("child-lines.mjs") || rel.endsWith("precise-lines.mjs"),
      process.env.PROBATIO_PACKAGE_ROOT || process.cwd(),
    )
  } catch (err) {
    writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
  }
  if (sampler) {
    process.on("exit", () => {
      // A node:test process that loaded this through an inherited NODE_OPTIONS has its own
      // hooks. Its hits are already named. A nameless dump would land on the wrong test.
      if (globalThis.__probatioTestHooks) return
      try {
        const hits = sampler.positiveLines()
        // A child started by a mapped test writes where that test's process reads.
        // A child that was not (a script that imports this file itself) uses the shared dir.
        const dir = process.env.PROBATIO_CHILD_DIR || `${mapPath}.children`
        mkdirSync(dir, { recursive: true })
        writeFileSync(path.join(dir, `${process.pid}-${randomBytes(4).toString("hex")}.json`), JSON.stringify({ hits }))
      } catch (err) {
        writeFileSync(`${mapPath}.error`, `${err instanceof Error ? err.message : String(err)}\n`)
      }
    })
  }
}
