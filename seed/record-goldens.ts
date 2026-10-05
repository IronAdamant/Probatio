// Turn unit tests into golden rows.
//
//   npx tsx ../../experiments/slim-tests/record-goldens.ts <ref> <tests.tsv>   (run from examples/auspex-ts)
//
// Takes the listed tests as they were at <ref>, runs them once against src/ with every exported
// function wrapped in a recorder, and writes each JSON-safe call they made (arguments plus the
// clock at that instant) to tests/golden/cases/<module>.ts. The assertions are not kept: the
// golden harness records what each call returns (UPDATE_GOLDEN=1) and compares from then on.
import { execFileSync, spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { homedir, tmpdir } from "node:os"
import path from "node:path"
import { pathToFileURL } from "node:url"

const [ref, listPath] = process.argv.slice(2)
if (!ref || !listPath) throw new Error("usage: record-goldens.ts <git-ref> <tests.tsv>")
const pkg = process.cwd()
// A sibling of the package, so tests that reach ../../../docs from tests/ land in the same place.
const rec = `${pkg}-golden-record`
const recLog = path.join(rec, "calls.jsonl")
const outDir = path.join(pkg, "tests", "golden", "cases")

// 1. The tests to convert, as they were at <ref>, with every other test in their files removed.
const wanted = new Map<string, Set<string>>()
for (const row of readFileSync(listPath, "utf8").split("\n")) {
  if (!row.trim() || row.startsWith("#")) continue
  const [file, title] = row.split("\t")
  if (!wanted.has(file)) wanted.set(file, new Set())
  wanted.get(file)!.add(title)
}
rmSync(rec, { recursive: true, force: true })
mkdirSync(path.join(rec, "tests"), { recursive: true })
mkdirSync(path.join(rec, "src"), { recursive: true })
for (const entry of readdirSync(pkg)) {
  if (entry === "tests" || entry === "src") continue
  symlinkSync(path.join(pkg, entry), path.join(rec, entry))
}
const repoRel = path.relative(execFileSync("git", ["rev-parse", "--show-toplevel"]).toString().trim(), pkg)
const srcModules = new Set<string>()
for (const [file, titles] of wanted) {
  const lines = execFileSync("git", ["show", `${ref}:${repoRel}/tests/${file}`]).toString().split("\n")
  const keep: string[] = []
  let skipping = false
  for (const line of lines) {
    const m = line.match(/^(?:test|it)\((["`])(.*?)\1/)
    if (m) skipping = !titles.has(m[2])
    if (!skipping) keep.push(line)
    if (skipping && /^\}\)/.test(line)) skipping = false
  }
  const text = keep.join("\n")
  for (const m of text.matchAll(/from "\.\.\/src\/([\w-]+)\.ts"/g)) srcModules.add(m[1])
  writeFileSync(path.join(rec, "tests", file), text)
}

// 2. A recording stand-in for each src module the tests import.
writeFileSync(
  path.join(rec, "rec.ts"),
  `import { appendFileSync } from "node:fs"
type Arg = { u: true } | { url: string } | { v: unknown }
function plain(v: unknown, depth = 0): boolean {
  if (depth > 40) return false
  if (v === null || typeof v === "string" || typeof v === "boolean") return true
  if (typeof v === "number") return Number.isFinite(v)
  if (v === undefined) return depth > 0
  if (Array.isArray(v)) return v.every((x) => x !== undefined && plain(x, depth + 1))
  if (typeof v === "object") {
    const proto = Object.getPrototypeOf(v)
    if (proto !== Object.prototype && proto !== null) return false
    return Object.values(v as object).every((x) => plain(x, depth + 1))
  }
  return false
}
function encode(a: unknown): Arg | undefined {
  if (a === undefined) return { u: true }
  if (a instanceof URL) return { url: a.href }
  return plain(a) ? { v: a } : undefined
}
export function wrap(mod: string, name: string, real: unknown): unknown {
  if (typeof real !== "function" || /^class[\\s{]/.test(Function.prototype.toString.call(real))) return real
  const fn = real as (...a: unknown[]) => unknown
  return function (this: unknown, ...args: unknown[]) {
    const encoded = args.map(encode)
    const now = Date.now()
    const out = fn.apply(this, args)
    if (encoded.every(Boolean) && !(out instanceof Promise)) {
      try { appendFileSync(${JSON.stringify(recLog)}, JSON.stringify({ mod, name, args: encoded, now, file: process.argv[1] ?? "" }) + "\\n") } catch {}
    }
    return out
  }
}
`,
)
for (const mod of srcModules) {
  const real = (await import(pathToFileURL(path.join(pkg, "src", `${mod}.ts`)).href)) as Record<string, unknown>
  const lines = [`import * as real from "../../${path.basename(pkg)}/src/${mod}.ts"`, `import { wrap } from "../rec.ts"`]
  for (const key of Object.keys(real)) lines.push(`export const ${key} = wrap(${JSON.stringify(mod)}, ${JSON.stringify(key)}, real.${key}) as typeof real.${key}`)
  writeFileSync(path.join(rec, "src", `${mod}.ts`), `${lines.join("\n")}\n`)
}

// 3. Run them once. A failing test still records the calls it made before it failed.
const files = readdirSync(path.join(rec, "tests")).map((f) => path.join(rec, "tests", f))
const run = spawnSync("npx", ["tsx", "--test", "--test-reporter=tap", ...files], { cwd: rec, encoding: "utf8", env: { ...process.env, SOLARI_API_KEY: "" } })
const summary = (run.stdout.match(/^# (tests|pass|fail) \d+$/gm) ?? []).join(", ")
console.log(`recording run: ${summary}`)
for (const m of run.stdout.matchAll(/^not ok \d+ - (.*)$/gm)) console.log(`  failed while recording: ${m[1]}`)
if (process.env.KEEP_RECORD) writeFileSync(`${rec}.log`, run.stdout + run.stderr)

// 4. One case file per module: unique calls only, named by function and a short argument preview.
type Call = { mod: string; name: string; args: Array<{ u?: true; url?: string; v?: unknown }>; now: number }
const calls: Call[] = existsSync(recLog)
  ? readFileSync(recLog, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l))
  : []
const clockModules = new Set(
  readdirSync(path.join(pkg, "src")).filter((f) => f.endsWith(".ts") && /Date\.now|new Date\(/.test(readFileSync(path.join(pkg, "src", f), "utf8"))).map((f) => f.replace(/\.ts$/, "")),
)
// Recorded arguments must not carry this machine's folders: the package root and the home folder
// become PKG and HOME, which the harness resolves on whatever machine runs the table.
const machineDirs: Array<[string, string]> = [[pkg, "PKG"], [homedir(), "HOME"]]
function portable(json: string): string {
  let out = json
  for (const [dir, name] of machineDirs) out = out.split(JSON.stringify(dir).slice(1, -1)).join(`" + ${name} + "`)
  return out.replace(/^"" \+ /, "").replace(/ \+ ""$/, "")
}
const literal = (a: Call["args"][number]) => {
  if (a.u) return "undefined"
  if (a.url !== undefined) return `new URL(${portable(JSON.stringify(a.url))})`
  const json = portable(JSON.stringify(a.v))
  // Unit tests often pass a deliberately partial object; the table keeps the data, not the type.
  return a.v !== null && typeof a.v === "object" ? `(${json} as never)` : json
}
const byMod = new Map<string, Map<string, Call>>()
// A call on a per-run temp folder only makes sense in the run that made it.
const tempDirs = [tmpdir(), realpathSync(tmpdir())]
let skippedTemp = 0
for (const c of calls) {
  if (tempDirs.some((d) => JSON.stringify(c.args).includes(d))) {
    skippedTemp++
    continue
  }
  const key = `${c.name}(${c.args.map(literal).join(", ")})`
  if (!byMod.has(c.mod)) byMod.set(c.mod, new Map())
  const seen = byMod.get(c.mod)!
  if (!seen.has(key)) seen.set(key, c)
}
mkdirSync(outDir, { recursive: true })
let rows = 0
for (const [mod, seen] of [...byMod].sort()) {
  const names = new Set<string>()
  const used = new Set<string>()
  const body: string[] = []
  for (const [key, c] of seen) {
    used.add(c.name)
    const preview = c.args.map(literal).join(", ").replace(/ as never\)/g, ")").replace(/\(\{/g, "{").replace(/\}\)/g, "}").replace(/\s+/g, " ")
    let label = `${c.name}(${preview.length > 90 ? `${preview.slice(0, 87)}...` : preview})`
    for (let n = 2; names.has(label); n++) label = `${label.replace(/ #\d+$/, "")} #${n}`
    names.add(label)
    const call = `() => ${key}`
    body.push(`  ${JSON.stringify(label)}: ${clockModules.has(mod) ? `at(${c.now}, ${call})` : call},`)
    rows++
  }
  const head = [
    `// Recorded from unit tests by experiments/slim-tests/record-goldens.ts. Outputs live in ../out/${mod}.json.`,
    `import { ${clockModules.has(mod) ? "at, " : ""}${body.some((b) => b.includes("HOME + ")) ? "HOME, " : ""}${body.some((b) => b.includes("PKG + ")) ? "PKG, " : ""}type GoldenCases } from "../harness.ts"`,
    `import { ${[...used].sort().join(", ")} } from "../../../src/${mod}.ts"`,
    "",
    "export const cases: GoldenCases = {",
  ]
  writeFileSync(path.join(outDir, `${mod}.ts`), `${[...head, ...body, "}"].join("\n")}\n`)
}
console.log(`${calls.length} calls recorded, ${rows} unique rows in ${byMod.size} case files (${skippedTemp} calls on temp folders skipped)`)
rmSync(rec, { recursive: true, force: true })
