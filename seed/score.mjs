// Score test sets against confirmed runs, split by mutant kind.
//   node score.mjs <runsDir,...> <name=set.json|ref> ...   (run from the repo root)
// A set is a git ref (its tests) or a JSON map of "<file>::<title>" → lines (a cover result).
// A mutant counts only if the full suite + golden union catches it ("catchable").
import { readFileSync } from "node:fs"
import { VARIANTS, caughtBy, kind, loadRuns, testsAt } from "./matrix-lib.mjs"

const [dirsArg, ...specs] = process.argv.slice(2)
const sets = {}
for (const spec of specs) {
  const [name, src] = spec.split("=")
  sets[name] = src.endsWith(".json") ? new Map(Object.entries(JSON.parse(readFileSync(src, "utf8")))) : testsAt(VARIANTS[src] ?? src)
}
const union = new Map([...Object.values(sets)].flatMap((s) => [...s]))
const runs = loadRuns(dirsArg.split(","), { union })
const catchable = runs.filter((r) => caughtBy(r, union).length)
const kinds = [...new Set(runs.map((r) => kind(r.id)))].sort()
console.log(["set", "lines", ...kinds.map((k) => `${k} (${catchable.filter((r) => kind(r.id) === k).length})`)].join("\t"))
for (const [name, set] of Object.entries(sets)) {
  const lines = [...set.values()].reduce((a, b) => a + b, 0)
  const cells = kinds.map((k) => {
    const rs = catchable.filter((r) => kind(r.id) === k)
    const n = rs.filter((r) => caughtBy(r, set).length).length
    return `${n} (${((100 * n) / Math.max(rs.length, 1)).toFixed(1)}%)`
  })
  console.log([name, lines, ...cells].join("\t"))
}
if (process.env.MISSED) {
  for (const [name, set] of Object.entries(sets)) {
    const missed = catchable.filter((r) => !caughtBy(r, set).length).map((r) => r.id)
    console.log(`${name} misses (${missed.length}): ${missed.join(" ")}`)
  }
}
