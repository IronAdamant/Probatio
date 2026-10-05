// Run mutants against a union checkout (every test from every variant) and record which tests fail.
//
//   node run-mutants.mjs <affected.json> <outDir> <worktree>... -- <patch>...
//
// One worker per worktree. Each mutant: `git apply -R` the patch, rebuild dist, run only the test
// files that can see the mutated module, record failing tests to <outDir>/<id>.json, restore src.
// A patch named "clean" runs the selected files with no mutation (flake baseline).
import { spawn, spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"

const argv = process.argv.slice(2)
const sep = argv.indexOf("--")
const [affectedPath, outDir, ...worktrees] = argv.slice(0, sep)
const patches = argv.slice(sep + 1)
const affected = JSON.parse(readFileSync(affectedPath, "utf8"))
const allTests = [...new Set(Object.values(affected).flat())].sort()
mkdirSync(outDir, { recursive: true })

// Every src module a patch touches (a real fix often spans several files).
function modulesOf(patch) {
  return [...readFileSync(patch, "utf8").matchAll(/^\+\+\+ [ab]\/examples\/auspex-ts\/src\/([^\s]+)/gm)].map((m) => m[1])
}
// Failing tests by title only. node's TAP for several files prints a file heading only when that
// file fails as a whole, so a remembered heading would stamp the wrong file on later failures.
// Titles are unique across the suite; analysis maps each title to its file.
function failures(tap) {
  const out = []
  for (const line of tap.split("\n")) {
    const m = line.match(/^(\s*)not ok \d+ - (.*)$/)
    if (!m) continue
    const title = m[2].replace(/ # .*$/, "")
    if (title.endsWith(".test.ts")) {
      // A whole file failed to load (syntax error, import error): count it as one failure.
      if (!tap.includes(`# Subtest: ${title}\n    # Subtest:`)) out.push(`${path.basename(title)}::<file>`)
      continue
    }
    out.push(`::${title}`)
  }
  return out
}
function maybeBuild(pkg) {
  const raw = process.env.PROBATIO_BUILD
  if (!raw || !raw.trim()) return Promise.resolve({ code: 0, out: "" })
  const parts = raw.trim().split(/\s+/).filter(Boolean)
  if (parts.length === 0) return Promise.resolve({ code: 0, out: "" })
  return run(parts[0], parts.slice(1), pkg, 120_000)
}

function run(cmd, args, cwd, timeoutMs) {
  return new Promise((resolve) => {
    const child = spawn(cmd, args, { cwd, env: { ...process.env, SOLARI_API_KEY: "", AUSPEX_LIVE: "" } })
    let out = ""
    child.stdout.on("data", (d) => (out += d))
    child.stderr.on("data", (d) => (out += d))
    const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs)
    child.on("close", (code) => {
      clearTimeout(timer)
      resolve({ code, out })
    })
  })
}

const queue = [...patches]
async function worker(wt) {
  const pkg = path.join(wt, "examples/auspex-ts")
  while (queue.length) {
    const patch = queue.shift()
    const id = path.basename(patch).replace(/\.patch$/, "")
    const outFile = path.join(outDir, `${id}.json`)
    if (existsSync(outFile)) continue
    const clean = id.startsWith("clean")
    const mods = clean ? [] : modulesOf(patch)
    const mod = mods.join(",")
    const files = clean || !mods.length || mods.some((m) => !affected[m]) ? allTests : [...new Set(mods.flatMap((m) => affected[m]))].sort()
    if (!clean) {
      const applied = spawnSync("git", ["-C", wt, "apply", "-R", path.resolve(patch)], { encoding: "utf8" })
      if (applied.status !== 0) {
        writeFileSync(outFile, JSON.stringify({ id, error: "apply failed", detail: applied.stderr }))
        continue
      }
    }
    const started = Date.now()
    await maybeBuild(pkg)
    const res = await run(
      "npx",
      ["tsx", "--test", "--test-reporter=tap", "--test-timeout=60000", "--test-concurrency=3", ...files.map((f) => `tests/${f}`)],
      pkg,
      600_000,
    )
    const failed = failures(res.out)
    const total = Number(res.out.match(/^# tests (\d+)$/m)?.[1] ?? 0)
    writeFileSync(outFile, JSON.stringify({ id, module: mod, files: files.length, tests: total, failed, seconds: (Date.now() - started) / 1000, exit: res.code }))
    spawnSync("git", ["-C", wt, "checkout", "--", "examples/auspex-ts/src"])
    spawnSync("git", ["-C", wt, "clean", "-fdq", "examples/auspex-ts/src"])
  }
}
await Promise.all(worktrees.map(worker))
console.log("done")
