import { spawnSync } from "node:child_process"
import { randomBytes } from "node:crypto"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import ts from "typescript"
import { SCHEMA_VERSION, type Envelope } from "../contract.js"
import { generateMutants } from "../mutate/generate.js"
import type { OperatorSet } from "../mutate/operators.js"
import { createWorktrees, packageIn, removeWorktrees, runMutants, type RunOptions } from "../mutate/run.js"

/**
 * Golden tables recorded from the unit tests a project already has.
 *
 * `golden record` runs those tests once with every exported function of a module wrapped. Each call
 * whose arguments and result are plain data becomes a row: the function, the arguments, and what it
 * returned or threw. A replay test runs every row against the code. `golden compare` then mutates
 * the module and reports whether the table kills every mutant those unit tests kill.
 *
 * Nothing is deleted. A table that kills what the unit tests kill is evidence for replacing them,
 * and the decision stays with whoever writes the commit. A test with hidden inputs (callbacks,
 * stubbed globals, a clock the function does not take) is not recordable and stays as code.
 */

const RECORDER = fileURLToPath(new URL("./recorder.mjs", import.meta.url))
const MODULE_EXT = /\.(ts|tsx|mts|js|mjs)$/

export type RecordOptions = {
  packageDir: string
  repoDir: string
  commit: string
  modules: string[]
  tests: string[]
  /** Package-relative directory for the tables and their replay tests. */
  outDir: string
  timeoutMs: number
}

type Encoded = { u?: true; url?: string; v?: unknown }

type Call = {
  module: string
  fn: string
  at: number
  args?: Encoded[]
  result?: Encoded
  error?: { name: string; message: string }
  async?: boolean
  hidden?: "argument" | "result"
}

type Row = {
  fn: string
  args: Encoded[]
  result?: Encoded
  error?: { name: string; message: string }
  async?: true
  now?: number
}

export async function recordGoldens(options: RecordOptions): Promise<Envelope> {
  const repo = realpathSync(path.resolve(options.repoDir))
  const packageDir = realpathSync(path.resolve(options.packageDir))
  const fail = (summary: string, next = "Fix the command and run it again."): Envelope => ({
    schemaVersion: SCHEMA_VERSION,
    ok: false,
    command: "golden.record",
    summary,
    next,
    nextCall: null,
  })
  if (options.modules.length === 0) return fail("Pass --module with the source file to record.")
  if (options.tests.length === 0) return fail("Pass --tests with the unit test files that call it.")
  for (const module of options.modules) {
    if (!MODULE_EXT.test(module)) return fail(`${module} is not a JavaScript or TypeScript module.`)
    if (!existsSync(path.join(packageDir, module))) return fail(`${module} is not in the package.`)
  }
  for (const file of options.tests) if (!existsSync(path.join(packageDir, file))) return fail(`${file} is not in the package.`)
  const sha = spawnSync("git", ["-C", repo, "rev-parse", "--verify", `${options.commit}^{commit}`], { encoding: "utf8" })
  if (sha.status !== 0) return fail("that commit does not resolve")
  const commit = sha.stdout.trim()
  const scratch = mkdtempSync(path.join(tmpdir(), "probatio-golden-"))
  const log = path.join(scratch, "calls.jsonl")
  const runId = randomBytes(4).toString("hex")
  const worktrees = await createWorktrees(repo, packageDir, runId, commit, 1, scratch)
  if ("error" in worktrees) {
    rmSync(scratch, { recursive: true, force: true })
    return fail(worktrees.error)
  }
  let testRun: { code: number; pass: number; fail: number }
  const exported = new Map<string, string[]>()
  try {
    const pkg = packageIn(worktrees.dirs[0], repo, packageDir)
    for (const module of options.modules) {
      const names = exportedFunctions(module, readFileSync(path.join(pkg, module), "utf8"))
      exported.set(module, names)
      wrapModule(pkg, module, names)
    }
    testRun = runTests(pkg, options.tests, log, options.timeoutMs)
  } finally {
    removeWorktrees(repo, worktrees.dirs, scratch)
  }
  const calls = readCalls(log)
  rmSync(scratch, { recursive: true, force: true })
  if (testRun.fail > 0 || testRun.code !== 0) {
    return fail(
      `The unit tests did not pass while recording (${testRun.pass} passed, ${testRun.fail} failed). A table recorded from a red run is not a contract.`,
      "Run those tests green first.",
    )
  }
  const written: string[] = []
  const modules: Array<{ module: string; rows: number; hidden: number; unstable: number; functions: string[] }> = []
  for (const module of options.modules) {
    const mine = calls.filter((call) => call.module === module)
    const clock = /Date\.now|new Date\(/.test(readFileSync(path.join(packageDir, module), "utf8"))
    const built = buildRows(mine, clock)
    const base = path.basename(module).replace(MODULE_EXT, "")
    const tableRel = path.posix.join(options.outDir, `${base}.golden.json`)
    const testRel = path.posix.join(options.outDir, `${base}.golden.test${module.endsWith(".ts") || module.endsWith(".tsx") || module.endsWith(".mts") ? ".ts" : ".mjs"}`)
    mkdirSync(path.join(packageDir, options.outDir), { recursive: true })
    writeFileSync(path.join(packageDir, tableRel), `${JSON.stringify({ module, rows: built.rows }, null, 2)}\n`)
    writeFileSync(path.join(packageDir, testRel), replayTest(module, tableRel, testRel))
    written.push(tableRel, testRel)
    modules.push({
      module,
      rows: Object.keys(built.rows).length,
      hidden: mine.filter((call) => call.hidden).length,
      unstable: built.unstable,
      functions: exported.get(module) ?? [],
    })
  }
  const rows = modules.reduce((sum, item) => sum + item.rows, 0)
  const hidden = modules.reduce((sum, item) => sum + item.hidden, 0)
  const unstable = modules.reduce((sum, item) => sum + item.unstable, 0)
  const empty = modules.filter((item) => item.rows === 0).map((item) => item.module)
  const notes = [`${rows} rows from ${testRun.pass} passing tests.`]
  if (hidden > 0) notes.push(`${hidden} calls had a callback, an instance, or another hidden input. Tests built on those stay as code.`)
  if (unstable > 0) notes.push(`${unstable} calls gave two answers for the same arguments, so they were left out.`)
  if (empty.length > 0) notes.push(`No row for ${empty.join(", ")}: those tests never called an export with plain data.`)
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: rows > 0,
    command: "golden.record",
    summary: notes.join(" "),
    next:
      rows > 0
        ? "Commit the tables and replay tests, then run nextCall to see whether the table kills what the unit tests kill. Nothing was deleted."
        : "Nothing to replay. Pick tests that call the module's exports with plain data.",
    nextCall:
      rows > 0
        ? {
            argv: [
              "golden",
              "compare",
              "--package",
              packageDir,
              ...options.modules.flatMap((module) => ["--module", module]),
              ...options.tests.flatMap((file) => ["--tests", file]),
              ...written.filter((file) => /\.golden\.test\./.test(file)).flatMap((file) => ["--golden", file]),
              "--out",
              path.join(packageDir, ".probatio", "golden-compare"),
            ],
          }
        : null,
    rows,
    hidden,
    unstable,
    modules,
    written,
    commit,
  }
}

/** Exported names that may be functions. Classes and default exports stay as code. */
export function exportedFunctions(file: string, text: string): string[] {
  const kind = file.endsWith(".tsx") ? ts.ScriptKind.TSX : /\.m?js$/.test(file) ? ts.ScriptKind.JS : ts.ScriptKind.TS
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, kind)
  const names = new Set<string>()
  const exported = (node: ts.Node) =>
    ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some((mod) => mod.kind === ts.SyntaxKind.ExportKeyword) &&
    !(ts.getModifiers(node) ?? []).some((mod) => mod.kind === ts.SyntaxKind.DefaultKeyword)
  for (const statement of source.statements) {
    if (ts.isFunctionDeclaration(statement) && statement.name && exported(statement)) names.add(statement.name.text)
    if (ts.isVariableStatement(statement) && exported(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        if (ts.isIdentifier(declaration.name)) names.add(declaration.name.text)
      }
    }
    if (ts.isExportDeclaration(statement) && !statement.moduleSpecifier && !statement.isTypeOnly && statement.exportClause && ts.isNamedExports(statement.exportClause)) {
      for (const element of statement.exportClause.elements) if (!element.isTypeOnly) names.add(element.name.text)
    }
  }
  return [...names].sort()
}

/** The module becomes a wrapper that records each call and forwards it to the real file beside it. */
function wrapModule(pkg: string, module: string, names: string[]): void {
  const abs = path.join(pkg, module)
  const ext = path.extname(module)
  const realBase = `${path.basename(module, ext)}.probatio-real${ext}`
  renameSync(abs, path.join(path.dirname(abs), realBase))
  const lines = [
    `export * from "./${realBase}"`,
    `import * as real from "./${realBase}"`,
    `import { record } from ${JSON.stringify(pathToFileURL(RECORDER).href)}`,
    ...names.map((name) => `export const ${name} = record(${JSON.stringify(module)}, ${JSON.stringify(name)}, real.${name})`),
    "",
  ]
  writeFileSync(abs, lines.join("\n"))
}

function runTests(pkg: string, tests: string[], log: string, timeoutMs: number): { code: number; pass: number; fail: number } {
  const tsx = path.join(pkg, "node_modules", ".bin", "tsx")
  const bin = existsSync(tsx) ? tsx : process.execPath
  const args = [...(existsSync(tsx) ? [] : ["--experimental-strip-types"]), "--test", "--test-reporter=tap", ...tests]
  const env: NodeJS.ProcessEnv = { ...process.env, PROBATIO_GOLDEN_LOG: log }
  for (const key of Object.keys(env)) if (key === "NODE_TEST_CONTEXT" || key.startsWith("NODE_TEST_")) delete env[key]
  const run = spawnSync(bin, args, { cwd: pkg, env, encoding: "utf8", timeout: timeoutMs, maxBuffer: 64 * 1024 * 1024 })
  const count = (name: string) => Number(new RegExp(`^# ${name} (\\d+)$`, "m").exec(run.stdout ?? "")?.[1] ?? 0)
  return { code: run.status ?? 1, pass: count("pass"), fail: count("fail") }
}

function readCalls(log: string): Call[] {
  if (!existsSync(log)) return []
  return readFileSync(log, "utf8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .flatMap((line) => {
      try {
        return [JSON.parse(line) as Call]
      } catch {
        return []
      }
    })
}

/** One row per distinct call. A call that answered two ways for the same arguments is left out. */
function buildRows(calls: Call[], clock: boolean): { rows: Record<string, Row>; unstable: number } {
  const byKey = new Map<string, { row: Row; outcomes: Set<string> }>()
  for (const call of calls) {
    if (call.hidden || !call.args) continue
    const key = `${call.fn}${JSON.stringify(call.args)}${clock ? `@${call.at}` : ""}`
    const outcome = JSON.stringify(call.error ? { error: call.error } : { result: call.result })
    const known = byKey.get(key)
    if (known) {
      known.outcomes.add(outcome)
      continue
    }
    const row: Row = { fn: call.fn, args: call.args, ...(call.error ? { error: call.error } : { result: call.result }) }
    if (call.async) row.async = true
    if (clock) row.now = call.at
    byKey.set(key, { row, outcomes: new Set([outcome]) })
  }
  const rows: Record<string, Row> = {}
  let unstable = 0
  const ids = new Set<string>()
  for (const { row, outcomes } of byKey.values()) {
    if (outcomes.size > 1) {
      unstable += 1
      continue
    }
    const preview = row.args.map((arg) => (arg.u ? "undefined" : arg.url !== undefined ? `URL(${arg.url})` : JSON.stringify(arg.v))).join(", ")
    let id = `${row.fn}(${preview.length > 80 ? `${preview.slice(0, 77)}...` : preview})`
    for (let n = 2; ids.has(id); n += 1) id = `${id.replace(/ #\d+$/, "")} #${n}`
    ids.add(id)
    rows[id] = row
  }
  return { rows, unstable }
}

/** A self-contained node:test file. It does not import Probatio, so the project keeps it without the tool. */
function replayTest(module: string, tableRel: string, testRel: string): string {
  const fromTest = (target: string) => {
    const rel = path.posix.relative(path.posix.dirname(testRel), target)
    return rel.startsWith(".") ? rel : `./${rel}`
  }
  return `// @ts-nocheck
// Generated by \`probatio golden record\` from the unit tests that called ${module}.
// The table is ${path.posix.basename(tableRel)}: one row per call, with what it returned or threw.
// A failing row is a behaviour change. Fix the code, or re-record and review the JSON diff.
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test, { mock } from "node:test"
import * as subject from ${JSON.stringify(fromTest(module))}

const table = JSON.parse(readFileSync(new URL(${JSON.stringify(fromTest(tableRel))}, import.meta.url), "utf8"))

function decode(arg) {
  if (arg.u) return undefined
  if (arg.url !== undefined) return new URL(arg.url)
  return structuredClone(arg.v)
}

function encode(value) {
  if (value === undefined) return { u: true }
  if (value instanceof URL) return { url: value.href }
  return { v: JSON.parse(JSON.stringify(value)) }
}

for (const [id, row] of Object.entries(table.rows)) {
  test(\`golden ${module} \${id}\`, async () => {
    if (typeof row.now === "number") mock.timers.enable({ apis: ["Date"], now: row.now })
    try {
      const fn = subject[row.fn]
      assert.equal(typeof fn, "function", \`\${row.fn} is no longer exported\`)
      let actual
      try {
        const out = fn(...row.args.map(decode))
        actual = { result: encode(row.async ? await out : out) }
      } catch (error) {
        actual = { error: { name: typeof error?.name === "string" ? error.name : "Error", message: String(error?.message ?? error) } }
      }
      assert.deepStrictEqual(actual, row.error ? { error: row.error } : { result: row.result })
    } finally {
      mock.timers.reset()
    }
  })
}
`
}

export type CompareOptions = {
  packageDir: string
  repoDir: string
  commit: string
  modules: string[]
  tests: string[]
  golden: string[]
  outDir: string
  operators: OperatorSet
  timeoutMs: number
  onProgress?: (line: string) => void
}

/**
 * Mutate the module, then score the same mutants twice: once with only the unit tests, once with
 * only the replay tests. The question is whether the table kills every mutant the unit tests kill.
 */
export async function compareGoldens(options: CompareOptions): Promise<Envelope> {
  const fail = (summary: string, next = "Fix the command and run it again."): Envelope => ({
    schemaVersion: SCHEMA_VERSION,
    ok: false,
    command: "golden.compare",
    summary,
    next,
    nextCall: null,
  })
  if (options.modules.length === 0 || options.tests.length === 0 || options.golden.length === 0) {
    return fail("Pass --module, --tests, and --golden.")
  }
  const outDir = path.resolve(options.outDir)
  rmSync(outDir, { recursive: true, force: true })
  const patchDirs: string[] = []
  for (const [index, module] of options.modules.entries()) {
    const out = path.join(outDir, `generate-${index}`)
    const generated = generateMutants({
      packageDir: options.packageDir,
      srcDir: module,
      outDir: out,
      perFile: null,
      skip: 0,
      seed: 20261003,
      maxMutants: null,
      skipFiles: [],
      commit: options.commit,
      operators: options.operators,
    })
    if (generated.error) return fail(generated.error, "Commit the module, the table, and the replay test, then run the same command.")
    if (generated.violations.length > 0) return fail(`Refusing to write mutants: ${generated.violations[0].file}:${generated.violations[0].line} sits inside a string or a comment.`)
    patchDirs.push(path.join(out, "mutants"))
  }
  const score = (label: string, tests: string[]) =>
    runMutants(baseRun({ ...options, outDir: path.join(outDir, label), patchDirs, onlyTests: tests }))
  const unit = await score("unit", options.tests)
  if (!unit.ok) return fail(`unit tests: ${unit.summary}`, unit.next)
  const golden = await score("golden", options.golden)
  if (!golden.ok) return fail(`golden replay: ${golden.summary}`, golden.next)
  const killedBy = (report: { kills: Array<{ id: string }> }) => new Set(report.kills.map((kill) => kill.id))
  const unitKills = killedBy(unit)
  const goldenKills = killedBy(golden)
  const unitOnly = [...unitKills].filter((id) => !goldenKills.has(id)).sort()
  const goldenOnly = [...goldenKills].filter((id) => !unitKills.has(id)).sort()
  const both = [...unitKills].filter((id) => goldenKills.has(id)).length
  const covers = unitOnly.length === 0 && unitKills.size > 0
  return {
    schemaVersion: SCHEMA_VERSION,
    ok: true,
    command: "golden.compare",
    summary: `The table kills ${both} of the ${unitKills.size} mutants the unit tests kill. ${unitOnly.length} only the unit tests kill. ${goldenOnly.length} only the table kills. Operators: ${options.operators}.`,
    next: covers
      ? "On these mutants the table catches everything the unit tests catch. Replacing those tests is still a decision: keep any test with hidden inputs, run a sealed or ledger check, and give the reason in the commit. Nothing was deleted."
      : unitKills.size === 0
        ? "The unit tests killed nothing here, so there is nothing to compare. Try --operators wide."
        : `Keep the unit tests that kill ${unitOnly.slice(0, 5).join(", ")}${unitOnly.length > 5 ? ", and more" : ""}, or record rows that do. Nothing was deleted.`,
    nextCall: null,
    operators: options.operators,
    unitKills: unitKills.size,
    goldenKills: goldenKills.size,
    both,
    unitOnly: unitOnly.slice(0, 10),
    goldenOnly: goldenOnly.slice(0, 10),
    rest: Math.max(unitOnly.length - 10, 0),
    unit: { summary: unit.summary, full: unit.full },
    golden: { summary: golden.summary, full: golden.full },
    pruning: { mode: "advisory", deletedTests: 0 },
  }
}

function baseRun(input: CompareOptions & { outDir: string; patchDirs: string[]; onlyTests: string[] }): RunOptions {
  return {
    packageDir: input.packageDir,
    repoDir: input.repoDir,
    commit: input.commit,
    patchDirs: input.patchDirs,
    outDir: input.outDir,
    direction: "forward",
    workers: 1,
    concurrency: 1,
    maxMinutes: null,
    maxMutants: null,
    build: null,
    testsDir: "tests",
    unset: [],
    suiteTimeoutMs: input.timeoutMs,
    testTimeoutMs: 60_000,
    confirm: true,
    affected: false,
    agent: null,
    onlyTests: input.onlyTests,
    onProgress: input.onProgress,
  }
}
