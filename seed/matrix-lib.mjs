// Shared by kill-matrix.mjs and cover.mjs: tests per variant (with line costs) and confirmed runs.
import { execFileSync } from "node:child_process"
import { existsSync, readFileSync, readdirSync } from "node:fs"
import path from "node:path"

export const VARIANTS = { full: "b38e85f", slimA: "b5ef60d", slimB0: "c1424d3", slimB: "f574dd0" }

export function blocks(text) {
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

/** Map of "<file>::<title>" → lines for one git ref. A golden area costs its case-table lines. */
export function testsAt(ref) {
  const tree = execFileSync("git", ["ls-tree", "-r", "--name-only", ref, "examples/auspex-ts/tests"]).toString().split("\n")
  const set = new Map()
  for (const f of tree.filter((p) => /tests\/[\w-]+\.test\.ts$/.test(p))) {
    const file = path.basename(f)
    if (file === "golden.test.ts") continue
    for (const b of blocks(execFileSync("git", ["show", `${ref}:${f}`]).toString())) set.set(`${file}::${b.title}`, b.lines)
  }
  for (const f of tree.filter((p) => /tests\/golden\/cases\/[\w-]+\.ts$/.test(p))) {
    set.set(`golden.test.ts::golden: ${path.basename(f, ".ts")}`, execFileSync("git", ["show", `${ref}:${f}`]).toString().split("\n").length)
  }
  return set
}

/** Every run in the given dirs; failures as "<file>::<title>" (titles are unique across the union). */
export function loadRuns(dirs, members) {
  const fileOfTitle = new Map()
  for (const set of Object.values(members)) for (const k of set.keys()) fileOfTitle.set(k.slice(k.indexOf("::") + 2), k)
  const runs = []
  for (const dir of dirs) {
    if (!existsSync(dir)) continue
    for (const f of readdirSync(dir).filter((f) => f.endsWith(".json"))) {
      const r = JSON.parse(readFileSync(path.join(dir, f), "utf8"))
      if (r.error) continue
      r.failed = r.failed.map((t) => (t.startsWith("::") ? fileOfTitle.get(t.slice(2)) ?? t : t))
      runs.push(r)
    }
  }
  return runs
}

/** Tests of `set` that catch `run` (a file that failed to load counts for every test of that file). */
export function caughtBy(run, set) {
  const out = new Set()
  for (const t of run.failed) {
    if (t.endsWith("::<file>")) {
      const file = t.split("::")[0]
      for (const k of set.keys()) if (k.startsWith(`${file}::`)) out.add(k)
    } else if (set.has(t)) out.add(t)
  }
  return [...out]
}

export const kind = (id) => (id.startsWith("r-") ? "real" : id.startsWith("v") ? "synthetic-2" : id.startsWith("ho-") ? "holdout" : "synthetic-1")
