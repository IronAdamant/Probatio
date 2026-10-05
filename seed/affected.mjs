// Which test files can see a change to each src module.
//   node affected.mjs <pkgDir>   → prints JSON { [srcModule]: [testFile...] }
// A test file sees a module when its static or dynamic imports reach it (through src, scripts or
// test helpers). Test files that start a process, run the built dist, or are the golden harness
// can reach anything, so they are listed for every module.
import { existsSync, readFileSync, readdirSync } from "node:fs"
import path from "node:path"

const pkg = path.resolve(process.argv[2] ?? ".")
const IMPORT = /(?:from\s+|import\s*\(\s*|import\s+)["'](\.{1,2}\/[^"']+)["']/g

const deps = new Map()
function importsOf(file) {
  if (deps.has(file)) return deps.get(file)
  const out = new Set()
  deps.set(file, out)
  if (!existsSync(file)) return out
  const text = readFileSync(file, "utf8")
  for (const m of text.matchAll(IMPORT)) {
    let target = path.resolve(path.dirname(file), m[1])
    if (!/\.(ts|mjs|js|py)$/.test(target)) target += ".ts"
    out.add(target)
  }
  // The Python sandbox assert is read as text by sandbox.ts / receipt.ts, not imported.
  if (/assert_receipt\.py/.test(text)) out.add(path.join(pkg, "src", "assert_receipt.py"))
  return out
}
function closure(file) {
  const seen = new Set()
  const stack = [file]
  while (stack.length) {
    const f = stack.pop()
    if (seen.has(f)) continue
    seen.add(f)
    for (const d of importsOf(f)) stack.push(d)
  }
  return seen
}

const testsDir = path.join(pkg, "tests")
const tests = readdirSync(testsDir).filter((f) => f.endsWith(".test.ts"))
const always = new Set(["golden.test.ts"])
for (const t of tests) {
  const text = readFileSync(path.join(testsDir, t), "utf8")
  if (/spawn|execFile|execSync|dist\/|bin\/auspex/.test(text)) always.add(t)
}
const srcModules = readdirSync(path.join(pkg, "src")).filter((f) => /\.(ts|py)$/.test(f))
const result = {}
for (const mod of srcModules) result[mod] = new Set(always)
for (const t of tests) {
  for (const f of closure(path.join(testsDir, t))) {
    const rel = path.relative(path.join(pkg, "src"), f)
    if (!rel.startsWith("..") && result[rel]) result[rel].add(t)
  }
}
console.log(JSON.stringify(Object.fromEntries(Object.entries(result).map(([k, v]) => [k, [...v].filter((t) => tests.includes(t)).sort()])), null, 1))
