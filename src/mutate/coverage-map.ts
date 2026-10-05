import { spawnSync } from "node:child_process"
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import { WHOLE_PROGRAM_TEST } from "./suite-decision.js"

export type CoverageMap = {
  files: Record<string, Record<string, string[]>>
}

export type CoverageSelection =
  | { state: "none" }
  | { state: "uncovered"; file: string; line: number }
  | { state: "covered"; file: string; line: number; tests: string[] }

export function readCoverageMap(file: string): CoverageMap | null {
  if (!existsSync(file)) return null
  try {
    const parsed = JSON.parse(readFileSync(file, "utf8")) as CoverageMap
    if (!parsed || typeof parsed.files !== "object" || parsed.files === null) return null
    return parsed
  } catch {
    return null
  }
}

/** Lines the baseline executed, and the tests that hit them. No map means no decision. */
export function selectionFor(map: CoverageMap | null, files: Array<{ file: string; line: number }>): CoverageSelection {
  if (!map || Object.keys(map.files).length === 0 || files.length === 0) return { state: "none" }
  const tests = new Set<string>()
  let sawUncovered = false
  let uncovered = files[0]
  for (const item of files) {
    const hit = testsOnLine(map, item.file, item.line)
    if (hit === null) {
      sawUncovered = true
      uncovered = item
      continue
    }
    for (const name of hit) tests.add(name)
  }
  if (tests.size === 0 && sawUncovered) return { state: "uncovered", file: uncovered.file, line: uncovered.line }
  if (tests.size === 0) return { state: "none" }
  const [file, line] = coveredAnchor(map, files)
  return { state: "covered", file, line, tests: [...tests] }
}

function coveredAnchor(map: CoverageMap, files: Array<{ file: string; line: number }>): [string, number] {
  for (const item of files) {
    if (testsOnLine(map, item.file, item.line)) return [item.file, item.line]
  }
  return [files[0].file, files[0].line]
}

function testsOnLine(map: CoverageMap, file: string, line: number): string[] | null {
  const lines = linesFor(map, file)
  if (!lines) return null
  const tests = lines[String(line)]
  if (!tests || tests.length === 0) return null
  return tests
}

function linesFor(map: CoverageMap, file: string): Record<string, string[]> | null {
  const norm = file.split(path.sep).join("/")
  if (map.files[norm]) return map.files[norm]
  if (map.files[file]) return map.files[file]
  // A repo-relative patch can be longer than the package-relative map key.
  // The longest suffix wins, so span/simple.py does not steal span_handlers/simple.py.
  let best: Record<string, string[]> | null = null
  let bestLen = -1
  const baseHits: Record<string, string[]>[] = []
  const base = norm.split("/").pop() ?? norm
  for (const [name, lines] of Object.entries(map.files)) {
    const key = name.split(path.sep).join("/")
    if (key === norm || norm.endsWith(`/${key}`) || key.endsWith(`/${norm}`)) {
      if (key.length > bestLen) {
        best = lines
        bestLen = key.length
      }
    }
    if (key === base || key.endsWith(`/${base}`)) baseHits.push(lines)
  }
  if (best) return best
  if (baseHits.length === 1) return baseHits[0]
  return null
}

export function binaryFromCommand(command: string, pkg: string): string | null {
  const match = command.match(/(?:^|\s)-o\s+["']?([^\s"']+)["']?/)
  if (!match) return null
  const rel = match[1]
  const full = path.isAbsolute(rel) ? rel : path.join(pkg, rel)
  return existsSync(full) ? full : null
}

/**
 * Build a line map from LLVM profraw left by an instrumented baseline.
 * Writes nothing when llvm-cov cannot start or no profile was produced.
 */
export function writeLlvmCoverage(pkg: string, profileDir: string, binary: string | null, dest: string): { ok: boolean; detail: string } {
  if (!existsSync(profileDir)) return { ok: false, detail: "no profile directory" }
  const raws = readdirSync(profileDir).filter((name) => name.endsWith(".profraw")).map((name) => path.join(profileDir, name))
  if (raws.length === 0) return { ok: false, detail: "no profraw" }
  if (!binary) return { ok: false, detail: "no instrumented binary" }
  const llvmCov = findTool("llvm-cov")
  const llvmProf = findTool("llvm-profdata")
  if (!llvmCov || !llvmProf) return { ok: false, detail: `llvm tool missing (${llvmCov ? "llvm-profdata" : "llvm-cov"})` }
  const merged = path.join(profileDir, "baseline.profdata")
  const merge = spawnSync(llvmProf, ["merge", "-sparse", "-o", merged, ...raws], { encoding: "utf8" })
  if (merge.status !== 0) return { ok: false, detail: (merge.stderr || merge.stdout || "llvm-profdata failed").trim() }
  const exported = spawnSync(llvmCov, ["export", binary, `-instr-profile=${merged}`], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 })
  if (exported.status !== 0) return { ok: false, detail: (exported.stderr || exported.stdout || "llvm-cov failed").trim() }
  const map = llvmExportToMap(exported.stdout, pkg)
  if (!map) return { ok: false, detail: "llvm-cov export was not a coverage map" }
  writeFileSync(dest, `${JSON.stringify(map, null, 2)}\n`)
  return { ok: true, detail: `${Object.keys(map.files).length} files` }
}

function llvmExportToMap(stdout: string, pkg: string): CoverageMap | null {
  const start = stdout.indexOf("{")
  if (start < 0) return null
  let parsed: { data?: Array<{ files?: Array<{ filename?: string; segments?: number[][] }> }> }
  try {
    parsed = JSON.parse(stdout.slice(start)) as typeof parsed
  } catch {
    return null
  }
  const files: CoverageMap["files"] = {}
  const root = pkg.endsWith(path.sep) ? pkg : `${pkg}${path.sep}`
  for (const group of parsed.data ?? []) {
    for (const file of group.files ?? []) {
      if (!file.filename) continue
      const lines: Record<string, string[]> = {}
      for (const segment of file.segments ?? []) {
        const line = segment[0]
        const count = segment[2] ?? 0
        if (!line || count <= 0) continue
        lines[String(line)] = [WHOLE_PROGRAM_TEST]
      }
      if (Object.keys(lines).length === 0) continue
      let rel = file.filename
      if (rel.startsWith(root)) rel = rel.slice(root.length)
      rel = rel.split(path.sep).join("/")
      files[rel] = { ...(files[rel] ?? {}), ...lines }
    }
  }
  if (Object.keys(files).length === 0) return null
  return { files }
}

function findTool(name: string): string | null {
  const fromXcrun = spawnSync("xcrun", ["--find", name], { encoding: "utf8" })
  const xcrunPath = fromXcrun.stdout?.trim() ?? ""
  if (fromXcrun.status === 0 && xcrunPath && existsSync(xcrunPath)) return xcrunPath
  const which = spawnSync("sh", ["-c", `command -v ${name}`], { encoding: "utf8" })
  const found = which.stdout?.trim() ?? ""
  if (which.status === 0 && found) return found
  return null
}
