import { spawnSync } from "node:child_process"
import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import type { CoverageMap } from "./coverage-map.js"

/** Blocks in a Go cover profile whose count is above zero. */
export function parseCoverProfile(text: string, modulePath: string): Array<{ file: string; line: number }> {
  const hits: Array<{ file: string; line: number }> = []
  const prefix = modulePath ? `${modulePath}/` : ""
  for (const raw of text.split("\n")) {
    const line = raw.trim()
    if (!line || line.startsWith("mode:")) continue
    const match = /^(.+):(\d+)\.\d+,(\d+)\.\d+\s+\d+\s+(\d+)$/.exec(line)
    if (!match) continue
    if (Number(match[4]) <= 0) continue
    let file = match[1]
    if (prefix && file.startsWith(prefix)) file = file.slice(prefix.length)
    file = file.split("\\").join("/")
    const start = Number(match[2])
    const end = Number(match[3])
    for (let number = start; number <= end; number++) hits.push({ file, line: number })
  }
  return hits
}

/**
 * One compiled test binary per package, then one profile per test name.
 * A stock cover profile does not name the test, so each test gets its own.
 */
export function writeGoCoverage(pkg: string, testNames: string[], dest: string): { ok: boolean; detail: string } {
  const wanted = new Set(testNames.filter((name) => name.length > 0 && !name.includes("/")))
  if (wanted.size === 0) return { ok: false, detail: "go coverage: no test names" }
  const mod = run("go", ["list", "-m"], pkg)
  if (mod.status !== 0) return { ok: false, detail: clip(mod.stderr || mod.stdout || mod.error?.message || "go list -m failed") }
  const modulePath = mod.stdout.trim().split("\n")[0]?.trim() ?? ""
  const listed = run("go", ["list", "-f", "{{.ImportPath}}\t{{.Dir}}\t{{len .TestGoFiles}}\t{{len .XTestGoFiles}}", "./..."], pkg)
  if (listed.status !== 0) return { ok: false, detail: clip(listed.stderr || listed.stdout || listed.error?.message || "go list failed") }
  const work = path.join(path.dirname(dest), "go-cover")
  mkdirSync(work, { recursive: true })
  const files: CoverageMap["files"] = {}
  const packages = listed.stdout.split("\n").map((line) => line.trim()).filter(Boolean)
  for (const row of packages) {
    const [importPath, dir, testFiles, xTestFiles] = row.split("\t")
    if (!importPath || !dir) continue
    if (Number(testFiles ?? "0") + Number(xTestFiles ?? "0") <= 0) continue
    const bin = path.join(work, `${safe(importPath)}.test`)
    const compiled = run("go", ["test", "-c", "-covermode=set", "-o", bin, importPath], pkg)
    if (compiled.status !== 0 || compiled.error) continue
    const listedTests = run(bin, ["-test.list", "."], dir)
    if (listedTests.status !== 0) continue
    for (const name of listedTests.stdout.split("\n").map((item) => item.trim()).filter((item) => wanted.has(item))) {
      const profile = path.join(work, `${safe(importPath)}-${safe(name)}.out`)
      run(bin, ["-test.run", `^${escapeRegExp(name)}$`, "-test.coverprofile", profile], dir)
      let text = ""
      try {
        text = readFileSync(profile, "utf8")
      } catch {
        continue
      }
      for (const hit of parseCoverProfile(text, modulePath)) addHit(files, hit.file, hit.line, name)
    }
  }
  if (Object.keys(files).length === 0) return { ok: false, detail: "go coverage: no covered lines" }
  writeFileSync(dest, `${JSON.stringify({ files }, null, 2)}\n`)
  return { ok: true, detail: `${Object.keys(files).length} files` }
}

function addHit(files: CoverageMap["files"], file: string, line: number, test: string): void {
  const lines = files[file] ?? {}
  const key = String(line)
  const names = lines[key] ?? []
  if (!names.includes(test)) names.push(test)
  lines[key] = names
  files[file] = lines
}

function safe(value: string): string {
  return value.replace(/[^A-Za-z0-9._-]+/g, "_")
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

function run(bin: string, args: string[], cwd: string) {
  return spawnSync(bin, args, { cwd, encoding: "utf8", timeout: 120_000 })
}

function clip(text: string): string {
  const line = text.trim().split("\n").find((item) => item.trim()) ?? "go coverage failed"
  return line.slice(0, 240)
}
