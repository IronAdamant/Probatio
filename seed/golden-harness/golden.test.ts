// Golden tables: each file in golden/cases exports `cases`, a map of name → call. The harness
// runs every call, records what came back (or what was thrown), and compares the whole table
// with golden/out/<area>.json. A changed row fails with its name, the recorded value and the
// new one. When the change is intended, re-record with UPDATE_GOLDEN=1 and review the JSON diff.
import assert from "node:assert/strict"
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import test, { mock } from "node:test"
import { GOLDEN_NOW, type GoldenCases, type GoldenRow } from "./golden/harness.ts"

const here = path.dirname(fileURLToPath(import.meta.url))
const casesDir = path.join(here, "golden", "cases")
const outDir = path.join(here, "golden", "out")
const pkgRoot = path.resolve(here, "..")
const update = process.env.UPDATE_GOLDEN === "1"


// Only this run's own folders are rewritten: the package, home, and the per-run test state folder
// (a temp folder with a random name). A literal path such as /tmp/x.png in a row stays as it is.
function machineFree(s: string): string {
  return s
    .replace(/[^\s"']*auspex-test-state-\w+/g, "<test-state>")
    .split(pkgRoot).join("<pkg>")
    .split(homedir()).join("<home>")
}

function record(value: unknown): unknown {
  if (value === undefined) return "<undefined>"
  const text = JSON.stringify(value, (_k, v) => {
    if (v === undefined) return "<undefined>"
    if (typeof v === "function") return "<function>"
    if (v instanceof Map) return { "<Map>": [...v.entries()] }
    if (v instanceof Set) return { "<Set>": [...v.values()] }
    if (v instanceof Error) return { "<Error>": v.name, message: v.message }
    if (typeof v === "string") return machineFree(v)
    return v
  })
  return JSON.parse(text)
}

async function run(row: GoldenRow): Promise<unknown> {
  const call = typeof row === "function" ? row : row.run
  mock.timers.setTime(typeof row === "function" ? GOLDEN_NOW : row.now)
  try {
    return record(await call())
  } catch (err) {
    const e = err as Error
    return { "<throws>": `${e?.name ?? "Error"}: ${machineFree(String(e?.message ?? err))}` }
  }
}

const areas = readdirSync(casesDir)
  .filter((f) => f.endsWith(".ts"))
  .sort()

for (const file of areas) {
  const area = file.replace(/\.ts$/, "")
  test(`golden: ${area}`, async () => {
    const { cases } = (await import(pathToFileURL(path.join(casesDir, file)).href)) as { cases: GoldenCases }
    mock.timers.enable({ apis: ["Date"], now: GOLDEN_NOW })
    const actual: Record<string, unknown> = {}
    try {
      for (const name of Object.keys(cases)) actual[name] = await run(cases[name])
    } finally {
      mock.timers.reset()
    }
    const outPath = path.join(outDir, `${area}.json`)
    if (update) {
      mkdirSync(outDir, { recursive: true })
      writeFileSync(outPath, `${JSON.stringify(actual, null, 2)}\n`)
      return
    }
    let expected: Record<string, unknown>
    try {
      expected = JSON.parse(readFileSync(outPath, "utf8"))
    } catch {
      assert.fail(`no recording for ${area}: run UPDATE_GOLDEN=1 npm test and review golden/out/${area}.json`)
    }
    const changed: string[] = []
    for (const name of new Set([...Object.keys(expected), ...Object.keys(actual)])) {
      if (!(name in actual)) changed.push(`- ${name}: recorded but no longer a case`)
      else if (!(name in expected)) changed.push(`- ${name}: new case, not recorded yet`)
      else if (JSON.stringify(actual[name]) !== JSON.stringify(expected[name])) {
        changed.push(
          `- ${name}\n    recorded: ${JSON.stringify(expected[name])}\n    now:      ${JSON.stringify(actual[name])}`,
        )
      }
    }
    assert.equal(
      changed.length,
      0,
      `${area}: ${changed.length} row(s) changed. If intended: UPDATE_GOLDEN=1 npm test, then review the JSON diff.\n${changed.join("\n")}`,
    )
  })
}
