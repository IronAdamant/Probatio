import { existsSync, readFileSync, readdirSync, statSync } from "node:fs"
import path from "node:path"

const IMPORT_SPEC = /(?:from\s+|import\s*\(\s*|import\s+)["'](\.{1,2}\/[^"']+)["']/g

export type AffectedOptions = {
  srcDir?: string
  testsDir?: string
  /** A test whose source matches this reaches every module (process, built output). */
  alwaysPattern?: RegExp
}

/** Package-relative source file to the test files that can see a change in it. */
export function affectedTests(packageDir: string, options: AffectedOptions = {}): Record<string, string[]> {
  const srcRel = options.srcDir ?? "src"
  const testsRel = options.testsDir ?? "tests"
  const srcDir = path.join(packageDir, srcRel)
  const testsDir = path.join(packageDir, testsRel)
  const alwaysPattern = options.alwaysPattern ?? /\b(?:spawn|execFile|execSync)\b|dist\//
  const sources = walk(srcDir, (name) => /\.(ts|tsx|py)$/.test(name) && !name.endsWith(".d.ts"))
  const tests = existsSync(testsDir)
    ? readdirSync(testsDir)
        .filter((name) => /\.test\.(ts|tsx|mts|js|mjs)$/.test(name))
        .sort()
    : []
  const imports = new Map<string, Set<string>>()
  const read = (file: string) => {
    const found = imports.get(file)
    if (found) return found
    const out = new Set<string>()
    imports.set(file, out)
    if (!existsSync(file)) return out
    const text = readFileSync(file, "utf8")
    for (const match of text.matchAll(IMPORT_SPEC)) {
      const resolved = resolveImport(path.dirname(file), match[1])
      if (resolved) out.add(resolved)
    }
    return out
  }
  const closureOf = (file: string) => {
    const seen = new Set<string>()
    const stack = [file]
    while (stack.length) {
      const current = stack.pop()
      if (!current || seen.has(current)) continue
      seen.add(current)
      for (const next of read(current)) stack.push(next)
    }
    return seen
  }
  const rel = (file: string) => path.relative(packageDir, file).split(path.sep).join("/")
  const byModule = new Map<string, Set<string>>()
  for (const source of sources) byModule.set(rel(source), new Set())
  const testRel = (name: string) => `${testsRel}/${name}`.replace(/\\/g, "/")
  const always: string[] = []
  for (const test of tests) {
    const text = readFileSync(path.join(testsDir, test), "utf8")
    if (alwaysPattern.test(text)) always.push(testRel(test))
  }
  for (const names of byModule.values()) for (const test of always) names.add(test)
  for (const test of tests) {
    const seen = closureOf(path.join(testsDir, test))
    for (const file of seen) {
      const key = rel(file)
      const testsForFile = byModule.get(key)
      if (testsForFile) testsForFile.add(testRel(test))
    }
    const blob = [path.join(testsDir, test), ...seen].map((file) => (existsSync(file) ? readFileSync(file, "utf8") : "")).join("\n")
    for (const source of sources) {
      if (!source.endsWith(".py")) continue
      if (blob.includes(path.basename(source))) byModule.get(rel(source))?.add(testRel(test))
    }
  }
  for (const testFile of walk(testsDir, (name) => /^test_.*\.py$/.test(name) || /_test\.py$/.test(name))) {
    const text = readFileSync(testFile, "utf8")
    const relTest = rel(testFile)
    for (const source of sources) {
      if (!source.endsWith(".py")) continue
      const moduleName = pythonModuleName(rel(source))
      if (!moduleName) continue
      const pattern = new RegExp(`(?<![\\w.])${escapeRegExp(moduleName)}(?![\\w.])`)
      if (pattern.test(text)) byModule.get(rel(source))?.add(relTest)
    }
  }
  return Object.fromEntries([...byModule.entries()].sort().map(([file, names]) => [file, [...names].sort()]))
}

/** Test files that import one of `changed` themselves, not through another module. */
export function directImporters(packageDir: string, changed: string[], testsDir = "tests"): string[] {
  const testsRoot = path.join(packageDir, testsDir)
  if (!existsSync(testsRoot)) return []
  const wanted = new Set(changed.map((file) => path.resolve(packageDir, file)))
  const out: string[] = []
  const seen = new Set<string>()
  const add = (rel: string) => {
    if (seen.has(rel)) return
    seen.add(rel)
    out.push(rel)
  }
  for (const name of readdirSync(testsRoot).filter((item) => /\.test\.(ts|tsx|mts|js|mjs)$/.test(item)).sort()) {
    const file = path.join(testsRoot, name)
    const text = readFileSync(file, "utf8")
    for (const match of text.matchAll(IMPORT_SPEC)) {
      const resolved = resolveImport(path.dirname(file), match[1])
      if (resolved && wanted.has(resolved)) {
        add(`${testsDir}/${name}`.replace(/\\/g, "/"))
        break
      }
    }
  }
  const patterns = changed
    .map((file) => pythonModuleName(file))
    .filter((name): name is string => name !== null)
    .map((name) => new RegExp(`(?<![\\w.])${escapeRegExp(name)}(?![\\w.])`))
  if (patterns.length > 0) {
    for (const file of walk(testsRoot, (name) => /^test_.*\.py$/.test(name) || /_test\.py$/.test(name))) {
      const text = readFileSync(file, "utf8")
      if (!patterns.some((pattern) => pattern.test(text))) continue
      add(path.relative(packageDir, file).split(path.sep).join("/"))
    }
  }
  return out
}

/** `src/pkg/mod.py` is the import name `pkg.mod`. */
function pythonModuleName(file: string): string | null {
  const norm = file.split(path.sep).join("/")
  if (!norm.endsWith(".py")) return null
  let body = norm.slice(0, -3)
  if (body.startsWith("src/")) body = body.slice(4)
  if (body.endsWith("/__init__")) body = body.slice(0, -"/__init__".length)
  if (!body || body.startsWith("../")) return null
  return body.split("/").join(".")
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

function resolveImport(fromDir: string, spec: string): string | null {
  const target = path.resolve(fromDir, spec)
  const candidates = [target, `${target}.ts`, `${target}.tsx`, `${target}.mts`, `${target}.js`, `${target}.mjs`, `${target}.py`]
  for (const candidate of candidates) {
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate
  }
  return null
}

function walk(dir: string, keep: (name: string) => boolean): string[] {
  if (!existsSync(dir)) return []
  const out: string[] = []
  const visit = (current: string) => {
    for (const entry of readdirSync(current).sort()) {
      const full = path.join(current, entry)
      if (statSync(full).isDirectory()) visit(full)
      else if (keep(entry)) out.push(full)
    }
  }
  visit(dir)
  return out
}
