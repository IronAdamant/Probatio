import { spawnSync } from "node:child_process"
import { mkdirSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs"
import path from "node:path"
import { mutantId, makeRng, shuffle } from "./ids.js"
import { findInSource, isGeneratedSource } from "./find.js"
import type { MutantPoint } from "./operators.js"
import { forwardDiff } from "./patch.js"

export type GenerateOptions = {
  packageDir: string
  srcDir: string
  outDir: string
  /** Mutants kept per file after the shuffle. Null keeps every point. */
  perFile: number | null
  /** How many shuffled points to skip in each file before taking `perFile`. */
  skip: number
  seed: number
  maxMutants: number | null
  /** Stop the walk when this many minutes have passed. Null walks the whole tree. */
  maxMinutes?: number | null
  /** Basenames excluded before mutation, for prose modules that are not behaviour. */
  skipFiles: string[]
}

export type GeneratedMutant = {
  id: string
  file: string
  line: number
  op: string
  patch: string
}

export type GenerateResult = {
  mutants: GeneratedMutant[]
  violations: MutantPoint[]
  filesVisited: number
  budgetHit: boolean
}

export function generateMutants(options: GenerateOptions): GenerateResult {
  const started = Date.now()
  const limit = options.maxMinutes
  const over = () => limit != null && Date.now() - started >= limit * 60_000
  if (over()) return { mutants: [], violations: [], filesVisited: 0, budgetHit: true }
  const srcRoot = path.resolve(options.packageDir, options.srcDir)
  const walked = walkSources(srcRoot, over)
  const files = walked.files.filter((file) => !options.skipFiles.includes(path.basename(file)))
  const rng = makeRng(options.seed)
  const violations: MutantPoint[] = []
  const chosen: Array<{ rel: string; text: string; point: MutantPoint }> = []
  let filesVisited = 0
  let budgetHit = walked.stopped
  for (const file of files) {
    if (over()) {
      budgetHit = true
      break
    }
    filesVisited += 1
    const text = readFileSync(file, "utf8")
    const rel = posix(path.relative(options.packageDir, file))
    const found = findInSource(rel, text)
    violations.push(...found.violations)
    const ordered = shuffle(found.points, rng)
    const slice = options.perFile === null ? ordered.slice(options.skip) : ordered.slice(options.skip, options.skip + options.perFile)
    for (const point of slice) chosen.push({ rel, text, point })
  }
  if (violations.length > 0) return { mutants: [], violations, filesVisited, budgetHit }
  const capped = options.maxMutants === null ? chosen : chosen.slice(0, options.maxMutants)
  const packageReal = real(options.packageDir)
  const patchRoot = gitRoot(packageReal)
  const prefix = patchRoot ? path.relative(patchRoot, packageReal) : ""
  const mutantsDir = path.join(options.outDir, "mutants")
  rmSync(mutantsDir, { recursive: true, force: true })
  mkdirSync(mutantsDir, { recursive: true })
  const mutants: GeneratedMutant[] = []
  for (const item of capped) {
    const id = mutantId(item.rel, item.point.start, item.point.end, item.point.op)
    const mutated = item.text.slice(0, item.point.start) + item.point.replacement + item.text.slice(item.point.end)
    const label = prefix && !prefix.startsWith("..") ? posix(path.join(prefix, item.rel)) : item.rel
    const diff = forwardDiff(label, item.text, mutated)
    if (!diff) continue
    const patch = `${id}.patch`
    writeFileSync(path.join(mutantsDir, patch), diff)
    mutants.push({ id, file: item.rel, line: item.point.line, op: item.point.op, patch })
  }
  writeFileSync(
    path.join(mutantsDir, "mutants.json"),
    `${JSON.stringify({ schemaVersion: 1, seed: options.seed, mutants }, null, 2)}\n`,
  )
  return { mutants, violations, filesVisited, budgetHit }
}

function walkSources(dir: string, over: () => boolean): { files: string[]; stopped: boolean } {
  if (!statExists(dir)) return { files: [], stopped: false }
  const out: string[] = []
  let stopped = false
  const walk = (current: string) => {
    if (over()) {
      stopped = true
      return
    }
    let entries: string[]
    try {
      entries = readdirSync(current).sort()
    } catch {
      return
    }
    for (const entry of entries) {
      if (over()) {
        stopped = true
        return
      }
      if (entry === "node_modules" || entry === ".git") continue
      const full = path.join(current, entry)
      let info
      try {
        info = statSync(full)
      } catch {
        continue
      }
      if (info.isDirectory()) {
        walk(full)
        if (stopped) return
      } else if (isSource(entry)) out.push(full)
    }
  }
  walk(dir)
  return { files: out, stopped }
}

function isSource(name: string): boolean {
  return isGeneratedSource(name)
}

function statExists(file: string): boolean {
  try {
    statSync(file)
    return true
  } catch {
    return false
  }
}

function gitRoot(cwd: string): string | null {
  const result = spawnSync("git", ["-C", cwd, "rev-parse", "--show-toplevel"], { encoding: "utf8" })
  if (result.status !== 0) return null
  return real(result.stdout.trim())
}

function real(file: string): string {
  try {
    return realpathSync(file)
  } catch {
    return path.resolve(file)
  }
}

function posix(file: string): string {
  return file.split(path.sep).join("/")
}
