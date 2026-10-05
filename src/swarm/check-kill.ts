import { existsSync, readFileSync, realpathSync } from "node:fs"
import path from "node:path"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"
import { directImporters } from "../mutate/affected.js"
import { filesInDiff } from "../mutate/patch.js"
import { killLabel } from "../mutate/kill-label.js"
import { runMutants, type RunOptions } from "../mutate/run.js"

/** The gap is done only when this mutant is killed and the baseline suite is green. */
export async function checkKill(id: string, options: RunOptions): Promise<Envelope> {
  const importers = importersOf(id, options)
  if (importers.length === 0) {
    return {
      schemaVersion: SCHEMA_VERSION,
      ok: false,
      command: "check-kill",
      summary: `${id} is not done. no direct importer in ${options.testsDir}`,
      next: "The task is not done.",
      nextCall: null,
      done: false,
      outcome: null,
      suiteGreen: false,
    }
  }
  const report = await runMutants({ ...options, onlyTests: importers, onlyPatch: id, affected: false })
  const baselineFile = path.join(options.outDir, "baseline.json")
  let suiteGreen = false
  if (existsSync(baselineFile)) {
    const baseline = JSON.parse(readFileSync(baselineFile, "utf8")) as { ok?: boolean }
    suiteGreen = baseline.ok === true
  }
  const resultFile = path.join(options.outDir, "results", `${id}.json`)
  let outcome: string | null = null
  let killedBy: string[] = []
  if (existsSync(resultFile)) {
    const result = JSON.parse(readFileSync(resultFile, "utf8")) as { outcome?: string; killedBy?: string[] }
    outcome = result.outcome ?? null
    killedBy = Array.isArray(result.killedBy) ? result.killedBy : []
  }
  const done = suiteGreen && outcome === "killed"
  const label = done ? killLabel(killedBy) : null
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: done,
    command: "check-kill",
    summary: done ? `${id} is dead and the suite is green.` : `${id} is not done. ${report.summary}`,
    next: label?.next ?? (done ? "The gap-fixing task is done." : "The task is not done."),
    nextCall: null,
    done,
    outcome,
    cause: label?.cause ?? null,
    killedBy,
    suiteGreen,
  }
}

/** Same direct-importer set verify-change runs for a diff that touches this mutant's file. */
function importersOf(id: string, options: RunOptions): string[] {
  const patch = options.patchDirs.map((dir) => path.join(dir, `${id}.patch`)).find((file) => existsSync(file))
  if (!patch) return []
  const rels = filesInDiff(readFileSync(patch, "utf8")).map((item) => packageRelative(options.repoDir, options.packageDir, item.file))
  return directImporters(options.packageDir, rels, options.testsDir)
}

function packageRelative(repoDir: string, packageDir: string, file: string): string {
  const norm = file.split(path.sep).join("/")
  let repo = repoDir
  let pkg = packageDir
  try {
    repo = realpathSync(repoDir)
    pkg = realpathSync(packageDir)
  } catch {
    // A missing path still has a relative prefix.
  }
  const prefix = path.relative(repo, pkg).split(path.sep).join("/")
  if (prefix && prefix !== "." && !prefix.startsWith("..") && norm.startsWith(`${prefix}/`)) return norm.slice(prefix.length + 1)
  return norm
}
