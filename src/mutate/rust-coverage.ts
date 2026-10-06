import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs"
import path from "node:path"
import { exportLlvmProfile, llvmToolsPresent, type CoverageMap } from "./coverage-map.js"

/**
 * One instrumented test binary, then one profile per test name.
 * A single cargo test profile does not say which test hit the line.
 */
export function writeRustCoverage(pkg: string, testNames: string[], dest: string): { ok: boolean; detail: string } {
  const names = [...new Set(testNames.map((name) => name.trim()).filter((name) => name.length > 0 && !/\s/.test(name)))]
  if (names.length === 0) return { ok: false, detail: "rust coverage: no test names" }
  if (!llvmToolsPresent()) return { ok: false, detail: "rust coverage: llvm tool missing" }
  const env: NodeJS.ProcessEnv = { ...process.env, RUSTFLAGS: "-C instrument-coverage", CARGO_INCREMENTAL: "0" }
  const built = spawnSync("cargo", ["test", "--no-run", "--message-format=json"], {
    cwd: pkg,
    encoding: "utf8",
    env,
    timeout: 180_000,
    maxBuffer: 32 * 1024 * 1024,
  })
  if (built.status !== 0 || built.error) return { ok: false, detail: clip(built.stderr || built.stdout || built.error?.message || "cargo test --no-run failed") }
  const exes = testExecutables(built.stdout)
  if (exes.length === 0) return { ok: false, detail: "rust coverage: no test executable" }
  const work = path.join(path.dirname(dest), "rust-cover")
  mkdirSync(work, { recursive: true })
  const files: CoverageMap["files"] = {}
  for (const name of names) {
    for (const exe of exes) {
      const raw = path.join(work, `${safe(name)}-${safe(path.basename(exe))}.profraw`)
      const ran = spawnSync(exe, [name, "--exact", "--test-threads=1"], {
        cwd: pkg,
        encoding: "utf8",
        env: { ...env, LLVM_PROFILE_FILE: raw },
        timeout: 60_000,
      })
      if (!existsSync(raw)) continue
      if (!new RegExp(`^test ${escapeRegExp(name)} \\.\\.\\. ok`, "m").test(`${ran.stdout}\n${ran.stderr}`)) continue
      const merged = path.join(work, `${safe(name)}-${safe(path.basename(exe))}.profdata`)
      const exported = exportLlvmProfile(exe, [raw], merged)
      if (!exported.ok) continue
      for (const hit of llvmHits(exported.stdout, pkg)) addHit(files, hit.file, hit.line, name)
    }
  }
  if (Object.keys(files).length === 0) return { ok: false, detail: "rust coverage: no covered lines" }
  writeFileSync(dest, `${JSON.stringify({ files }, null, 2)}\n`)
  return { ok: true, detail: `${Object.keys(files).length} files` }
}

function testExecutables(stdout: string): string[] {
  const found: string[] = []
  for (const line of stdout.split("\n")) {
    if (!line.startsWith("{")) continue
    try {
      const msg = JSON.parse(line) as { reason?: string; executable?: string | null; profile?: { test?: boolean } }
      if (msg.reason !== "compiler-artifact" || !msg.executable || !msg.profile?.test) continue
      if (!found.includes(msg.executable)) found.push(msg.executable)
    } catch {
      continue
    }
  }
  return found
}

function llvmHits(stdout: string, pkg: string): Array<{ file: string; line: number }> {
  const start = stdout.indexOf("{")
  if (start < 0) return []
  let parsed: { data?: Array<{ files?: Array<{ filename?: string; segments?: number[][] }> }> }
  try {
    parsed = JSON.parse(stdout.slice(start)) as typeof parsed
  } catch {
    return []
  }
  const hits: Array<{ file: string; line: number }> = []
  for (const group of parsed.data ?? []) {
    for (const file of group.files ?? []) {
      if (!file.filename) continue
      const rel = underPkg(file.filename, pkg)
      if (!rel) continue
      for (const segment of file.segments ?? []) {
        const line = segment[0]
        const count = segment[2] ?? 0
        if (!line || count <= 0) continue
        hits.push({ file: rel, line })
      }
    }
  }
  return hits
}

function underPkg(filename: string, pkg: string): string | null {
  let file = filename
  let root = pkg
  try {
    if (existsSync(filename)) file = realpathSync(filename)
  } catch {
    // keep the path llvm reported
  }
  try {
    root = realpathSync(pkg)
  } catch {
    // keep the package path
  }
  const rel = path.relative(root, file)
  if (!rel || rel.startsWith("..")) return null
  return rel.split(path.sep).join("/")
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

function clip(text: string): string {
  const line = text.trim().split("\n").find((item) => item.trim()) ?? "rust coverage failed"
  return line.slice(0, 240)
}
