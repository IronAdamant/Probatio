import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, statSync, writeFileSync } from "node:fs"
import path from "node:path"
import type { CoverageMap } from "./coverage-map.js"

/**
 * One Coverlet run per test name. The collector's report does not name the test,
 * so each name gets its own results directory.
 */
export function writeCsharpCoverage(pkg: string, testNames: string[], dest: string, project?: string): { ok: boolean; detail: string } {
  const names = [...new Set(testNames.map((name) => name.trim()).filter((name) => name.length > 0))]
  if (names.length === 0) return { ok: false, detail: "csharp coverage: no test names" }
  const work = path.join(path.dirname(dest), "csharp-cover")
  mkdirSync(work, { recursive: true })
  const settings = path.join(work, "coverlet.runsettings")
  // Coverlet skips the test assembly unless asked. A one-project suite keeps the code there.
  writeFileSync(
    settings,
    [
      '<?xml version="1.0" encoding="utf-8"?>',
      "<RunSettings>",
      "  <DataCollectionRunSettings>",
      "    <DataCollectors>",
      '      <DataCollector friendlyName="XPlat Code Coverage">',
      "        <Configuration>",
      "          <Format>cobertura</Format>",
      "          <IncludeTestAssembly>true</IncludeTestAssembly>",
      "        </Configuration>",
      "      </DataCollector>",
      "    </DataCollectors>",
      "  </DataCollectionRunSettings>",
      "</RunSettings>",
      "",
    ].join("\n"),
  )
  const files: CoverageMap["files"] = {}
  const env = dotnetEnv()
  for (const name of names) {
    const results = path.join(work, safe(name))
    mkdirSync(results, { recursive: true })
    const args = ["test", "--nologo", "--verbosity", "quiet", "--settings", settings]
    if (project) args.push(project)
    args.push("--filter", `FullyQualifiedName~${name}`, "--collect", "XPlat Code Coverage", "--results-directory", results)
    const ran = spawnSync("dotnet", args, { cwd: pkg, encoding: "utf8", env, timeout: 180_000, maxBuffer: 16 * 1024 * 1024 })
    if (ran.status !== 0 || ran.error) continue
    const report = findCobertura(results)
    if (!report) continue
    let text = ""
    try {
      text = readFileSync(report, "utf8")
    } catch {
      continue
    }
    addCobertura(files, text, pkg, name)
  }
  if (Object.keys(files).length === 0) return { ok: false, detail: "csharp coverage: no covered lines" }
  writeFileSync(dest, `${JSON.stringify({ files }, null, 2)}\n`)
  return { ok: true, detail: `${Object.keys(files).length} files` }
}

function addCobertura(files: CoverageMap["files"], text: string, pkg: string, test: string): void {
  const sources = [...text.matchAll(/<source>([^<]+)<\/source>/g)].map((match) => match[1].trim())
  for (const part of text.split(/<class\b/).slice(1)) {
    const headEnd = part.indexOf(">")
    if (headEnd < 0) continue
    const filename = /filename="([^"]+)"/.exec(part.slice(0, headEnd))?.[1]
    if (!filename) continue
    const bodyEnd = part.indexOf("</class>")
    const body = bodyEnd >= 0 ? part.slice(0, bodyEnd) : part
    const rel = coberturaRel(filename, sources, pkg)
    if (!rel) continue
    for (const tag of body.match(/<line\b[^>]*\/?>/g) ?? []) {
      const number = /number="(\d+)"/.exec(tag)
      const hits = /hits="(\d+)"/.exec(tag)
      if (!number || !hits || Number(hits[1]) <= 0) continue
      addHit(files, rel, Number(number[1]), test)
    }
  }
}

function coberturaRel(filename: string, sources: string[], pkg: string): string | null {
  const norm = filename.split("\\").join("/")
  const candidates = norm.startsWith("/") || /^[A-Za-z]:\//.test(norm) ? [norm] : [norm]
  for (const source of sources) {
    const root = source.endsWith("/") || source.endsWith("\\") ? source : `${source}/`
    candidates.push(path.normalize(`${root}${norm}`))
  }
  for (const candidate of candidates) {
    const rel = underPkg(candidate, pkg)
    if (rel) return rel
  }
  return null
}

function underPkg(filename: string, pkg: string): string | null {
  if (!existsSync(filename)) return null
  let file = filename
  let root = pkg
  try {
    file = realpathSync(filename)
    root = realpathSync(pkg)
  } catch {
    return null
  }
  const rel = path.relative(root, file)
  if (!rel || rel.startsWith("..")) return null
  return rel.split(path.sep).join("/")
}

function findCobertura(dir: string): string | null {
  const found: string[] = []
  const visit = (current: string) => {
    let entries: string[]
    try {
      entries = readdirSync(current)
    } catch {
      return
    }
    for (const entry of entries) {
      const full = path.join(current, entry)
      let info
      try {
        info = statSync(full)
      } catch {
        continue
      }
      if (info.isDirectory()) {
        visit(full)
        continue
      }
      if (entry === "coverage.cobertura.xml") found.push(full)
    }
  }
  visit(dir)
  return found[0] ?? null
}

function addHit(files: CoverageMap["files"], file: string, line: number, test: string): void {
  const lines = files[file] ?? {}
  const key = String(line)
  const names = lines[key] ?? []
  if (!names.includes(test)) names.push(test)
  lines[key] = names
  files[file] = lines
}

function dotnetEnv(): NodeJS.ProcessEnv {
  if (process.env.DOTNET_ROLL_FORWARD) return process.env
  return { ...process.env, DOTNET_ROLL_FORWARD: "Major" }
}

function safe(value: string): string {
  return value.replace(/[^A-Za-z0-9._-]+/g, "_")
}
