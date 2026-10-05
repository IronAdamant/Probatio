// Line-weighted greedy cover: the fewest test lines that still catch every training mutant the
// candidate pool catches, starting from tests that are kept by rule.
//
//   node cover.mjs <pool.json> <keep.txt> <k> <runsDir>...   (run from the repo root)
//
// pool.json: { "<file>::<title>": lines } candidate tests. keep.txt: one "<file>::<title>" per line
// (kept whatever the data says). k: how many distinct tests must catch each mutant (1 or 2).
// Training mutants: the real round-1 bugs (r-*) and synthetic batch 1 (s*). Prints the chosen set
// as JSON on stdout and a summary on stderr.
import { readFileSync } from "node:fs"
import { caughtBy, kind, loadRuns } from "./matrix-lib.mjs"

const [poolPath, keepPath, kRaw, ...dirs] = process.argv.slice(2)
const pool = new Map(Object.entries(JSON.parse(readFileSync(poolPath, "utf8"))))
const keep = readFileSync(keepPath, "utf8").split("\n").map((l) => l.trim()).filter(Boolean)
const k = Number(kRaw)
const runs = loadRuns(dirs, { pool })
const trainKinds = (process.env.TRAIN ?? "real,synthetic-1").split(",")
const train = runs.filter((r) => trainKinds.includes(kind(r.id)))

const kills = new Map([...pool.keys()].map((t) => [t, new Set()]))
for (const r of train) for (const t of caughtBy(r, pool)) kills.get(t).add(r.id)
const universe = new Set(train.filter((r) => caughtBy(r, pool).length).map((r) => r.id))

const chosen = new Set(keep.filter((t) => pool.has(t)))
const need = new Map([...universe].map((id) => [id, k]))
const credit = (t) => {
  for (const id of kills.get(t)) if (need.has(id)) need.set(id, need.get(id) - 1)
}
for (const t of chosen) credit(t)
// A mutant only one pool test catches cannot be caught twice; ask for what the pool can give.
for (const id of universe) {
  const able = [...pool.keys()].filter((t) => kills.get(t).has(id)).length
  if (need.get(id) > 0 && able < k) need.set(id, Math.max(0, need.get(id) - (k - able)))
}
const open = () => [...need.values()].some((n) => n > 0)
while (open()) {
  let best
  let bestScore = 0
  for (const [t, lines] of pool) {
    if (chosen.has(t)) continue
    let gain = 0
    for (const id of kills.get(t)) if ((need.get(id) ?? 0) > 0) gain++
    const score = gain / Math.max(lines, 1)
    if (score > bestScore) {
      bestScore = score
      best = t
    }
  }
  if (!best) break
  chosen.add(best)
  credit(best)
}

const lines = [...chosen].reduce((a, t) => a + pool.get(t), 0)
const poolLines = [...pool.values()].reduce((a, b) => a + b, 0)
console.error(`training mutants caught by pool: ${universe.size}; k=${k}`)
console.error(`chosen ${chosen.size} of ${pool.size} tests, ${lines} of ${poolLines} lines (kept by rule: ${keep.filter((t) => pool.has(t)).length})`)
console.log(JSON.stringify(Object.fromEntries([...chosen].sort().map((t) => [t, pool.get(t)])), null, 1))
