// Kill matrix over a union run: which tests catch which mutants, scored per variant, plus a
// line-weighted greedy cover with a train/validation split.
//
//   node kill-matrix.mjs <runsDir> <unionTestsDir> [--variants ref=name,...]   (run from the repo root)
//
// A test is "<file>::<title>" (golden areas: "golden.test.ts::golden: <area>"). Lines are the
// test block's own lines; a golden area costs its case-table lines (recorded output is not counted).
import { execFileSync } from "node:child_process"
import { existsSync, readFileSync, readdirSync } from "node:fs"
import path from "node:path"

const [runsDir, testsDir] = process.argv.slice(2)
const variants = { full: "b38e85f", slimA: "b5ef60d", slimB0: "c1424d3", slimB: "f574dd0" }

function blocks(text) {
  const lines = text.split("\n")
  const out = []
  lines.forEach((l, i) => {
    const m = l.match(/^(?:test|it)\((["`])(.*?)\1/)
    if (!m) return
    let e = i + 1
    while (e < lines.length && !/^\}\)/.test(lines[e])) e++
    out.push({ title: m[2], lines: e - i + 1 })
  })
  return out
}
function testsAt(ref) {
  const tree = execFileSync("git", ["ls-tree", "-r", "--name-only", ref, "examples/auspex-ts/tests"]).toString().split("\n")
  const set = new Map()
  for (const f of tree.filter((p) => /tests\/[\w-]+\.test\.ts$/.test(p))) {
    const file = path.basename(f)
    if (file === "golden.test.ts") continue
    for (const b of blocks(execFileSync("git", ["show", `${ref}:${f}`]).toString())) set.set(`${file}::${b.title}`, b.lines)
  }
  for (const f of tree.filter((p) => /tests\/golden\/cases\/[\w-]+\.ts$/.test(p))) {
    const area = path.basename(f, ".ts")
    set.set(`golden.test.ts::golden: ${area}`, execFileSync("git", ["show", `${ref}:${f}`]).toString().split("\n").length)
  }
  return set
}

const members = Object.fromEntries(Object.entries(variants).map(([name, ref]) => [name, testsAt(ref)]))
// node's TAP for several files does not name the file of a failing test; titles are unique across
// the union (checked), so the file comes from the title.
const fileOfTitle = new Map()
for (const set of Object.values(members)) for (const k of set.keys()) fileOfTitle.set(k.slice(k.indexOf("::") + 2), k)
const runs = readdirSync(runsDir)
  .filter((f) => f.endsWith(".json"))
  .map((f) => JSON.parse(readFileSync(path.join(runsDir, f), "utf8")))
  .filter((r) => !r.error)
  .map((r) => ({ ...r, failed: r.failed.map((t) => (t.startsWith("::") ? fileOfTitle.get(t.slice(2)) ?? t : t)) }))

// A file-level failure (the file did not load) is caught by every test of that file in a variant.
function caughtBy(run, set) {
  return run.failed.filter((t) => {
    if (t.endsWith("::<file>")) {
      const file = t.split("::")[0]
      return [...set.keys()].some((k) => k.startsWith(`${file}::`))
    }
    return set.has(t)
  })
}

const kind = (id) => (id.startsWith("r-") ? "real" : "synthetic")
console.log("variant\treal caught\tsynthetic caught\ttest lines")
const killedByFull = new Set(runs.filter((r) => caughtBy(r, members.full).length).map((r) => r.id))
for (const [name, set] of Object.entries(members)) {
  const real = runs.filter((r) => kind(r.id) === "real")
  const synth = runs.filter((r) => kind(r.id) === "synthetic" && killedByFull.has(r.id))
  const c = (rs) => rs.filter((r) => caughtBy(r, set).length).length
  const lines = [...set.values()].reduce((a, b) => a + b, 0)
  console.log(`${name}\t${c(real)}/${real.length}\t${c(synth)}/${synth.length}\t${lines}`)
}
const neverKilled = runs.filter((r) => kind(r.id) === "synthetic" && !killedByFull.has(r.id))
console.log(`synthetic mutants no test catches (equivalent or untested): ${neverKilled.length}`)

if (process.env.MISSED) {
  for (const [name, set] of Object.entries(members)) {
    const missed = runs.filter((r) => killedByFull.has(r.id) && !caughtBy(r, set).length).map((r) => r.id)
    console.log(`${name} misses: ${missed.join(" ")}`)
  }
}
export { members, runs, caughtBy, killedByFull }
