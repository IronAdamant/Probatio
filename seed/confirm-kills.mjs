// Confirm kills: re-run each failing test of each mutant on its own (exact title), twice, with the
// mutant applied. A kill counts only when the test fails both times; a test that passes on a rerun
// was a flake under load and is dropped from that mutant's kills.
//
//   node confirm-kills.mjs <runsDir> <patchDir>... -- <worktree>...
// Writes <runsDir>/<id>.json in place with `failed` confirmed and `flaky` listing the dropped ones.
import { spawn, spawnSync } from "node:child_process"
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs"
import path from "node:path"

const argv = process.argv.slice(2)
const sep = argv.indexOf("--")
const [runsDir, ...patchDirs] = argv.slice(0, sep)
const worktrees = argv.slice(sep + 1)
const patchOf = (id) => patchDirs.map((d) => path.join(d, `${id}.patch`)).find((p) => existsSync(p))
const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")

function maybeBuild(pkg) {
  const raw = process.env.PROBATIO_BUILD
  if (!raw || !raw.trim()) return
  const parts = raw.trim().split(/\s+/).filter(Boolean)
  if (parts.length === 0) return
  spawnSync(parts[0], parts.slice(1), { cwd: pkg })
}

function run(args, cwd) {
  return new Promise((resolve) => {
    const child = spawn("npx", args, { cwd, env: { ...process.env, SOLARI_API_KEY: "", AUSPEX_LIVE: "" } })
    let out = ""
    child.stdout.on("data", (d) => (out += d))
    child.stderr.on("data", (d) => (out += d))
    const timer = setTimeout(() => child.kill("SIGKILL"), 300_000)
    child.on("close", () => {
      clearTimeout(timer)
      resolve(out)
    })
  })
}

// Runs record failing tests by title; titles are unique, so the file comes from the test sources.
const testsDir = path.join(worktrees[0], "examples/auspex-ts/tests")
const fileOfTitle = new Map()
for (const f of readdirSync(testsDir).filter((f) => f.endsWith(".test.ts"))) {
  for (const l of readFileSync(path.join(testsDir, f), "utf8").split("\n")) {
    const m = l.match(/^(?:test|it)\((["`])(.*?)\1/)
    if (m) fileOfTitle.set(m[2], f)
  }
}
for (const f of readdirSync(path.join(testsDir, "golden", "cases"))) fileOfTitle.set(`golden: ${f.replace(/\.ts$/, "")}`, "golden.test.ts")
const attribute = (t) => (t.startsWith("::") ? `${fileOfTitle.get(t.slice(2)) ?? ""}::${t.slice(2)}` : t)

const todo = readdirSync(runsDir)
  .filter((f) => f.endsWith(".json"))
  .map((f) => JSON.parse(readFileSync(path.join(runsDir, f), "utf8")))
  .filter((r) => !r.error && r.failed.length && !r.confirmed)
  .map((r) => ({ ...r, failed: r.failed.map(attribute) }))

async function worker(wt) {
  const pkg = path.join(wt, "examples/auspex-ts")
  while (todo.length) {
    const r = todo.shift()
    const patch = patchOf(r.id)
    if (!patch) continue
    spawnSync("git", ["-C", wt, "apply", "-R", path.resolve(patch)])
    maybeBuild(pkg)
    const keep = []
    const flaky = []
    // One run per file per round: every failing title of that file, matched exactly.
    const byFile = new Map()
    for (const t of r.failed) {
      const [file, title] = t.split("::")
      if (title === "<file>") {
        keep.push(t) // a file that fails to load fails the same way every time
        continue
      }
      if (!byFile.has(file)) byFile.set(file, [])
      byFile.get(file).push(title)
    }
    const failCount = new Map()
    for (const [file, titles] of byFile) {
      const files = file ? [`tests/${file}`] : readdirSync(path.join(pkg, "tests")).filter((f) => f.endsWith(".test.ts")).map((f) => `tests/${f}`)
      const pattern = `^(${titles.map(escape).join("|")})$`
      for (let i = 0; i < 2; i++) {
        const out = await run(["tsx", "--test", "--test-reporter=tap", "--test-concurrency=3", `--test-name-pattern=${pattern}`, ...files], pkg)
        for (const title of titles) {
          if (new RegExp(`^\\s*not ok \\d+ - ${escape(title)}( #.*)?$`, "m").test(out)) failCount.set(`${file}::${title}`, (failCount.get(`${file}::${title}`) ?? 0) + 1)
        }
      }
    }
    for (const t of r.failed) {
      if (t.endsWith("::<file>")) continue
      ;(failCount.get(t) === 2 ? keep : flaky).push(t)
    }
    writeFileSync(path.join(runsDir, `${r.id}.json`), JSON.stringify({ ...r, failed: keep, flaky, confirmed: true }))
    spawnSync("git", ["-C", wt, "checkout", "--", "examples/auspex-ts/src"])
    spawnSync("git", ["-C", wt, "clean", "-fdq", "examples/auspex-ts/src"])
  }
}
await Promise.all(worktrees.map(worker))
console.log("confirmed")
