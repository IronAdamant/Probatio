import { spawn, spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import type { TestReport } from "./run.js"

export type SuiteSpec = {
  kind: string
  files: string[]
  /** Tool names only. This is the line written to progress. */
  command: string
  /** Project file the runner passes to the tool, when the suite is not the package root. */
  project?: string
}

const SKIP_DIR = new Set(["node_modules", ".git", "target", "dist", "bin", "obj", ".build", "vendor"])

export function discoverSuite(pkg: string, testsDir: string): SuiteSpec | null {
  return (
    scriptSuite(pkg) ||
    cargoSuite(pkg) ||
    goSuite(pkg) ||
    swiftPackageSuite(pkg) ||
    mavenSuite(pkg) ||
    dotnetSuite(pkg) ||
    pytestSuite(pkg, testsDir) ||
    unittestSuite(pkg, testsDir) ||
    nodeSuite(pkg) ||
    mochaSuite(pkg) ||
    makeTestSuite(pkg)
  )
}

export type SuiteOutcome = {
  report: TestReport | null
  timedOut: boolean
  detail: string
  command: string
  compileToken: string | null
  /** True when a test-name subset was requested and this command cannot take one. */
  wholeSuite: boolean
  selectedTests: string[]
}

export function commandSuite(command: string, files: string[]): SuiteSpec {
  return { kind: "command", files: files.length > 0 ? files : ["suite-command"], command }
}

export async function runDiscoveredSuite(
  pkg: string,
  files: string[],
  testsDir: string,
  pattern: string | null,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
  hooks: { reporterPath: string; testTimeoutMs: number; concurrency: number },
  extra?: { spec?: SuiteSpec | null; names?: string[] | null; coverageMap?: string | null; profileFile?: string | null },
): Promise<SuiteOutcome> {
  const spec = extra && "spec" in extra ? extra.spec ?? null : discoverSuite(pkg, testsDir)
  const empty: SuiteOutcome = {
    report: null,
    timedOut: false,
    detail: "no suite",
    command: "",
    compileToken: null,
    wholeSuite: false,
    selectedTests: [],
  }
  if (!spec) return empty
  const selected = files.length > 0 ? files : spec.files
  const child: NodeJS.ProcessEnv = { ...env }
  if (extra?.coverageMap) child.PROBATIO_COVERAGE_MAP = extra.coverageMap
  if (extra?.profileFile) child.LLVM_PROFILE_FILE = extra.profileFile
  const result = await launch(pkg, spec, selected, pattern, extra?.names ?? null, timeoutMs, child, hooks)
  const compileToken = result.timedOut ? null : detectCompile(spec.kind, result.stdout, result.stderr, result.code)
  // Ignore a report read from disk when this run failed to compile. The previous
  // suite's files are still in the worktree and are not this mutant's result.
  const report = result.timedOut || compileToken ? null : adapt(spec.kind, pkg, result.stdout, result.stderr, result.code)
  return {
    report,
    timedOut: result.timedOut,
    detail: firstLine(result.stderr || result.stdout),
    command: result.command,
    compileToken,
    wholeSuite: result.wholeSuite,
    selectedTests: result.selectedTests,
  }
}

function scriptSuite(pkg: string): SuiteSpec | null {
  if (!existsSync(path.join(pkg, "run_tests.sh"))) return null
  return { kind: "script", files: ["run_tests.sh"], command: "bash run_tests.sh" }
}

function cargoSuite(pkg: string): SuiteSpec | null {
  if (!existsSync(path.join(pkg, "Cargo.toml"))) return null
  const files = walk(pkg, (rel) => rel.endsWith(".rs") && !rel.startsWith("target/")).filter((rel) => {
    if (rel.split("/").includes("tests") || rel.endsWith("_test.rs")) return true
    return readFileSync(path.join(pkg, rel), "utf8").includes("#[test]")
  })
  if (files.length === 0) return null
  return { kind: "cargo", files, command: "cargo test" }
}

function goSuite(pkg: string): SuiteSpec | null {
  const files = walk(pkg, (rel) => rel.endsWith("_test.go"))
  if (files.length === 0) return null
  return { kind: "go", files, command: "go test -json ./..." }
}

function swiftPackageSuite(pkg: string): SuiteSpec | null {
  if (!existsSync(path.join(pkg, "Package.swift"))) return null
  const files = walk(pkg, (rel) => rel.endsWith(".swift") && rel.split("/").includes("Tests"))
  if (files.length === 0) return null
  return { kind: "swift", files, command: "swift test" }
}

function mavenSuite(pkg: string): SuiteSpec | null {
  if (!existsSync(path.join(pkg, "pom.xml"))) return null
  const files = walk(pkg, (rel) => rel.endsWith(".java") && rel.split("/").includes("test"))
  if (files.length === 0) return null
  return { kind: "maven", files, command: "mvn -B test" }
}

function dotnetSuite(pkg: string): SuiteSpec | null {
  const inTestDir = (rel: string) => hasSegment(rel, "tests") || hasSegment(rel, "test")
  const files = walk(pkg, (rel) => rel.endsWith(".cs") && inTestDir(rel))
  if (files.length === 0) return null
  const projects = walk(pkg, (rel) => rel.endsWith(".csproj") && inTestDir(rel))
  if (projects.length === 0) return null
  const project = [...projects].sort((a, b) => Number(isDotnetTestProject(pkg, b)) - Number(isDotnetTestProject(pkg, a)))[0]
  if (!isDotnetTestProject(pkg, project) && readFileSync(path.join(pkg, project), "utf8").includes("Exe")) {
    return { kind: "dotnet-exe", files, command: "dotnet run", project }
  }
  return { kind: "dotnet", files, command: "dotnet test", project }
}

function isDotnetTestProject(pkg: string, rel: string): boolean {
  return /Test\.Sdk|xunit|nunit|mstest/i.test(readFileSync(path.join(pkg, rel), "utf8"))
}

function pytestSuite(pkg: string, testsDir: string): SuiteSpec | null {
  const files = pythonTests(pkg, testsDir)
  if (files.length === 0 || !isPytest(pkg, testsDir, files)) return null
  return { kind: "pytest", files, command: "python3 pytest" }
}

function unittestSuite(pkg: string, testsDir: string): SuiteSpec | null {
  const files = pythonTests(pkg, testsDir)
  if (files.length === 0) return null
  return { kind: "unittest", files, command: "python3 unittest" }
}

function nodeSuite(pkg: string): SuiteSpec | null {
  const files = walk(pkg, isNodeTest)
  if (files.length === 0) return null
  if (usesMocha(pkg) && !files.some((rel) => readFileSync(path.join(pkg, rel), "utf8").includes("node:test"))) return null
  return { kind: "node", files, command: "node --test" }
}

function mochaSuite(pkg: string): SuiteSpec | null {
  if (!usesMocha(pkg)) return null
  const files = mochaFiles(pkg)
  if (files.length === 0) return null
  return { kind: "mocha", files, command: shown("mocha", buildMochaArgs(pkg, files)) }
}

/** A Makefile `test` target that links the project's own COBOL tests, instead of one file. */
function makeTestSuite(pkg: string): SuiteSpec | null {
  const makefile = path.join(pkg, "Makefile")
  if (!existsSync(makefile)) return null
  const text = readFileSync(makefile, "utf8")
  if (!/^test:/m.test(text)) return null
  const files = walk(pkg, (rel) => /\.(cob|cbl|cobol)$/i.test(rel) && hasSegment(rel, "tests"))
  if (files.length === 0) return null
  return { kind: "make-test", files, command: "make test" }
}

function compiledSuite(pkg: string): SuiteSpec | null {
  const asm = underSuite(pkg, (rel) => rel.endsWith(".asm") || rel.endsWith(".nasm") || rel.endsWith(".yasm"))
  if (asm.length > 0) return { kind: "asm-x86_64", files: asm, command: "nasm" }
  const arm = underSuite(pkg, (rel) => rel.endsWith(".s"))
  if (arm.length > 0) return { kind: "asm-aarch64", files: arm, command: "clang" }
  const riscv = underSuite(pkg, (rel) => rel.endsWith(".S"))
  if (riscv.length > 0) return { kind: "asm-riscv", files: riscv, command: "riscv64-elf-as" }
  const cobol = underSuite(pkg, (rel) => /\.(cob|cbl|cobol)$/i.test(rel))
  if (cobol.length > 0) return { kind: "cobol", files: cobol, command: "cobc" }
  const cpp = underSuite(pkg, (rel) => /\.(cpp|cc|cxx)$/.test(rel))
  if (cpp.length > 0) return { kind: "cpp", files: cpp, command: "clang++" }
  const c = underSuite(pkg, (rel) => rel.endsWith(".c"))
  if (c.length > 0) return { kind: "c", files: c, command: "clang" }
  const java = underSuite(pkg, (rel) => rel.endsWith(".java"))
  if (java.length > 0) return { kind: "java", files: java, command: "javac" }
  const swift = underSuite(pkg, (rel) => rel.endsWith(".swift"))
  if (swift.length > 0) return { kind: "swiftc", files: swift, command: "swiftc" }
  return null
}

function underSuite(pkg: string, accept: (rel: string) => boolean): string[] {
  return walk(pkg, (rel) => accept(rel) && (hasSegment(rel, "tests") || hasSegment(rel, "test")))
}

const SOURCE_EXT = /\.(?:cobol|cbl|cob|cpp|cxx|cc|java|asm|nasm|yasm|swift|py|go|rs|js|ts|cs|c|s|S)$/

/** Source files under tests/ or test/ that are not, by themselves, a proved runner. */
export function sourceCandidates(pkg: string): string[] {
  return walk(pkg, (rel) => SOURCE_EXT.test(rel) && (hasSegment(rel, "tests") || hasSegment(rel, "test")))
}

function pythonTests(pkg: string, testsDir: string): string[] {
  const configured = pytestTestPaths(pkg)
  const roots = new Set(configured ?? [testsDir, "tests", "test"])
  return walk(pkg, (rel) => {
    const base = path.posix.basename(rel)
    if (!/^test_.*\.py$/.test(base) && !/_test\.py$/.test(base) && !/^tests_.*\.py$/.test(base)) return false
    if (configured) return configured.some((root) => rel === root || rel.startsWith(`${root}/`))
    return rel.split("/").some((part) => roots.has(part))
  })
}

/** Directories pytest itself would collect. A missing setting keeps the broader walk. */
function pytestTestPaths(pkg: string): string[] | null {
  const fromPyproject = readPyprojectTestPaths(path.join(pkg, "pyproject.toml"))
  if (fromPyproject) return fromPyproject
  for (const file of ["pytest.ini", "tox.ini", "setup.cfg"]) {
    const found = readIniTestPaths(path.join(pkg, file))
    if (found) return found
  }
  return null
}

function readPyprojectTestPaths(file: string): string[] | null {
  if (!existsSync(file)) return null
  const text = readFileSync(file, "utf8")
  const section = /\[tool\.pytest\.ini_options\][^\n]*\n([\s\S]*?)(?:\n\[|\s*$)/.exec(text)?.[1]
  if (!section) return null
  const key = /testpaths\s*=\s*([\s\S]*?)(?:\n[A-Za-z]|\s*$)/.exec(section)?.[1]
  if (!key) return null
  const quoted = [...key.matchAll(/"([^"]+)"|'([^']+)'/g)].map((match) => (match[1] ?? match[2]).trim()).filter(Boolean)
  return quoted.length > 0 ? quoted : null
}

function readIniTestPaths(file: string): string[] | null {
  if (!existsSync(file)) return null
  const text = readFileSync(file, "utf8")
  const section = /\[(?:pytest|tool:pytest)\][^\n]*\n([\s\S]*?)(?:\n\[|\s*$)/.exec(text)?.[1]
  if (!section) return null
  const key = /testpaths\s*=\s*([\s\S]*?)(?:\n[A-Za-z]|\s*$)/.exec(section)?.[1]
  if (!key) return null
  const paths = key
    .split(/\s+/)
    .map((item) => item.trim())
    .filter((item) => item.length > 0 && !item.startsWith("#"))
  return paths.length > 0 ? paths : null
}

const MOCHA_SKIP_DIR = new Set(["support", "fixtures", "helpers", "helper"])

/** Mocha files the project would run: node-style names, TypeScript under test/, or describe/it scripts. */
function mochaFiles(pkg: string): string[] {
  return walk(pkg, (rel) => {
    const parts = rel.split("/")
    if (parts.some((part) => MOCHA_SKIP_DIR.has(part.toLowerCase()))) return false
    const inTest = parts.some((part) => part === "test" || part === "tests")
    if (!inTest) return false
    if (isNodeTest(rel)) return true
    if (parts.includes("test") && /\.tsx?$/.test(rel)) return true
    if (!/\.(?:js|mjs|cjs)$/.test(rel)) return false
    let text = ""
    try {
      text = readFileSync(path.join(pkg, rel), "utf8")
    } catch {
      return false
    }
    return /\bdescribe\s*\(/.test(text) || /\bit\s*\(/.test(text)
  })
}

/** Args after the mocha binary: reporter, the project's own --require, and files when package.json has no mocha.spec. */
export function buildMochaArgs(pkg: string, files: string[]): string[] {
  const config = readMochaConfig(pkg)
  const args = ["--reporter", "json", "--timeout", "20000"]
  const fromConfig = new Set((config?.require ?? []).filter((name) => name !== "esm"))
  for (const name of scriptRequires(pkg)) {
    if (name === "esm" || fromConfig.has(name)) continue
    args.push("--require", name)
  }
  const hasSpec = Boolean(config?.spec && config.spec.length > 0)
  if (!hasSpec) args.push(...files)
  return args
}

function scriptRequires(pkg: string): string[] {
  const file = path.join(pkg, "package.json")
  if (!existsSync(file)) return []
  let script = ""
  try {
    const body = JSON.parse(readFileSync(file, "utf8")) as { scripts?: { test?: string } }
    script = body.scripts?.test ?? ""
  } catch {
    return []
  }
  const found: string[] = []
  for (const match of script.matchAll(/(?:^|\s)--require(?:=|\s+)(\S+)/g)) {
    const name = match[1].replace(/^['"]|['"]$/g, "")
    if (name && name !== "esm" && !found.includes(name)) found.push(name)
  }
  return found
}

function isPytest(pkg: string, testsDir: string, files: string[]): boolean {
  if (existsSync(path.join(pkg, "pytest.ini")) || existsSync(path.join(pkg, "conftest.py"))) return true
  if (existsSync(path.join(pkg, testsDir, "conftest.py")) || existsSync(path.join(pkg, "tests", "conftest.py"))) return true
  const project = path.join(pkg, "pyproject.toml")
  if (existsSync(project) && readFileSync(project, "utf8").includes("pytest")) return true
  return files.some((rel) => readFileSync(path.join(pkg, rel), "utf8").includes("pytest"))
}

function isNodeTest(rel: string): boolean {
  const base = path.posix.basename(rel)
  if (!/\.(test|spec)\.(ts|tsx|mts|js|mjs|cjs)$/.test(base) && !/_test\.(ts|tsx|js|mjs)$/.test(base)) return false
  return hasSegment(rel, "test") || hasSegment(rel, "tests")
}

function usesMocha(pkg: string): boolean {
  const file = path.join(pkg, "package.json")
  if (!existsSync(file)) return false
  return readFileSync(file, "utf8").includes("mocha")
}

function hasSegment(rel: string, name: string): boolean {
  return rel.split("/").some((part) => part.toLowerCase() === name.toLowerCase())
}

type LaunchResult = {
  code: number
  stdout: string
  stderr: string
  timedOut: boolean
  command: string
  wholeSuite: boolean
  selectedTests: string[]
}

function tag(
  result: Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> | { code: number; stdout: string; stderr: string; timedOut: boolean },
  command: string,
  wholeSuite: boolean,
  selectedTests: string[],
): Promise<LaunchResult> {
  return Promise.resolve(result).then((item) => ({ ...item, command, wholeSuite, selectedTests }))
}

function shown(bin: string, args: string[]): string {
  return [bin, ...args.map((arg) => (/\s/.test(arg) ? JSON.stringify(arg) : arg))].join(" ")
}

function shellQuote(value: string): string {
  if (/^[A-Za-z0-9_:.]+$/.test(value)) return value
  return `'${value.replace(/'/g, `'\\''`)}'`
}

async function launch(
  pkg: string,
  spec: SuiteSpec,
  files: string[],
  pattern: string | null,
  names: string[] | null,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
  hooks: { reporterPath: string; testTimeoutMs: number; concurrency: number },
): Promise<LaunchResult> {
  const selection = names && names.length > 0 ? names : namesFromPattern(pattern)
  const wantsNames = selection.length > 0
  if (spec.kind === "node") {
    const nodePattern = pattern ?? (selection.length > 0 ? `^(${selection.map(escapeRegExp).join("|")})$` : null)
    const args = [
      ...(existsSync(path.join(pkg, "node_modules", ".bin", "tsx")) ? [] : ["--experimental-strip-types"]),
      "--test",
      ...(nodePattern ? [`--test-name-pattern=${nodePattern}`] : []),
      ...files,
    ]
    return tag(runNode(pkg, files, nodePattern, timeoutMs, env, hooks), shown(existsSync(path.join(pkg, "node_modules", ".bin", "tsx")) ? "tsx" : "node", args), false, selection)
  }
  if (spec.kind === "unittest") {
    const k = pattern ?? (selection.length > 0 ? selection.map((name) => name.split("::").pop() || name).join(" or ") : null)
    const shownK = k && /\s/.test(k) ? JSON.stringify(k) : k
    const command = shownK ? `${spec.command} -k ${shownK}` : spec.command
    return tag(runPython(pkg, files, k, timeoutMs, env, "py-reporter.py", null), command, false, selection)
  }
  if (spec.kind === "pytest") {
    // -k is an expression. test_simple_reject[<lambda>1] makes pytest exit 4, and that usage error is not a kill.
    const plan = pytestPlan(selection)
    const k = plan.nodeIds ? null : plan.pattern ?? (pattern && pytestKeywordExpr(pattern) ? pattern : null)
    const shownK = k && /\s/.test(k) ? JSON.stringify(k) : k
    const command = plan.nodeIds
      ? ["python3", "pytest", ...plan.nodeIds.map(quotePytestArg)].join(" ")
      : shownK
        ? `${spec.command} -k ${shownK}`
        : spec.command
    return tag(runPython(pkg, files, k, timeoutMs, env, "py-pytest-reporter.py", plan.nodeIds), command, false, selection)
  }
  if (spec.kind === "cargo") {
    if (selection.length === 0) {
      return tag(spawnCollected("cargo", ["test", "--", "--test-threads=1"], pkg, env, timeoutMs), "cargo test -- --test-threads=1", false, selection)
    }
    if (selection.length === 1) {
      const args = ["test", selection[0], "--", "--exact", "--test-threads=1"]
      return tag(spawnCollected("cargo", args, pkg, env, timeoutMs), shown("cargo", args), false, selection)
    }
    const script = selection.map((name) => `cargo test ${shellQuote(name)} -- --exact --test-threads=1`).join(" && ")
    return tag(spawnCollected("sh", ["-c", script], pkg, env, timeoutMs), script, false, selection)
  }
  if (spec.kind === "go") {
    const filter = goRunFilter(names, pattern)
    const args = ["test", "-json", "./...", ...(filter ? ["-run", filter] : [])]
    return tag(spawnCollected("go", args, pkg, env, timeoutMs), shown("go", args), false, selection)
  }
  if (spec.kind === "swift") return tag(runSwift(pkg, timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "maven") {
    const tests = uniqueSurefire(selection)
    const args = ["-B", "test", ...(tests.length > 0 ? [`-Dtest=${tests.join(",")}`] : [])]
    const whole = wantsNames && tests.length === 0
    // Surefire leaves the previous run's XML in place. A confirm that did not
    // re-execute a class would otherwise read that class as failed again.
    rmSync(path.join(pkg, "target", "surefire-reports"), { recursive: true, force: true })
    return tag(spawnCollected("mvn", args, pkg, javaEnv(env), timeoutMs), shown("mvn", args), whole, selection)
  }
  if (spec.kind === "dotnet") {
    const command = wantsNames ? `${spec.command} --filter ${selection.join("|")}` : spec.command
    return tag(runDotnetTest(pkg, spec, timeoutMs, env, selection), command, false, selection)
  }
  if (spec.kind === "dotnet-exe") return tag(runDotnetExe(pkg, spec, files, timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "mocha") {
    const grep = selection.length > 0 ? selection.map(escapeRegExp).join("|") : ""
    const command = grep ? `${spec.command} --grep ${JSON.stringify(grep)}` : spec.command
    return tag(runMocha(pkg, files, timeoutMs, env, selection), command, false, selection)
  }
  // A tty is only for a suite that runs `docker run -it` (tini). Everything else is bash.
  if (spec.kind === "script") {
    const launched = scriptInvocation(pkg)
    return tag(runScript(pkg, timeoutMs, env, launched), launched.command, wantsNames, selection)
  }
  if (spec.kind === "c" || spec.kind === "cpp") return tag(compileAndRun(pkg, files, spec.kind === "cpp", timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "java") return tag(compileJava(pkg, files, timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "swiftc") return tag(compileSwift(pkg, files, timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "cobol") return tag(compileCobol(pkg, files, timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "make-test") return tag(spawnCollected("make", ["test"], pkg, env, timeoutMs), "make test", wantsNames, selection)
  if (spec.kind === "asm-x86_64") return tag(assembleX86(pkg, files, timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "asm-aarch64") return tag(assembleArm(pkg, files, timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "asm-riscv") return tag(assembleRiscv(pkg, files, timeoutMs, env), spec.command, wantsNames, selection)
  if (spec.kind === "command") return tag(runShell(pkg, spec.command, timeoutMs, env), spec.command, wantsNames, selection)
  return tag({ code: 1, stdout: "", stderr: `no runner for ${spec.kind}`, timedOut: false }, spec.command, wantsNames, selection)
}

const PYTEST_KEYWORD = /^[A-Za-z_][A-Za-z0-9_]*$/

function pytestLeaf(name: string): string {
  return name.split("::").pop() || name
}

/** Safe leaves stay on -k. Anything else is a node id, passed as an argument. */
function pytestPlan(selection: string[]): { pattern: string | null; nodeIds: string[] | null } {
  if (selection.length === 0) return { pattern: null, nodeIds: null }
  // A doubled keep id (file.py::tests/file.py::test_low) has more than one ::.
  // Reducing it to the leaf would run -k test_low and still kill the mutant.
  if (selection.some((name) => name.split("::").length > 2)) return { pattern: null, nodeIds: selection }
  const leaves = selection.map(pytestLeaf)
  if (leaves.every((leaf) => PYTEST_KEYWORD.test(leaf))) return { pattern: leaves.join(" or "), nodeIds: null }
  return { pattern: null, nodeIds: selection }
}

function pytestKeywordExpr(pattern: string): boolean {
  return pattern.split(" or ").every((part) => PYTEST_KEYWORD.test(part))
}

function quotePytestArg(arg: string): string {
  return /[\s<>|&;()[\]"'\\]/.test(arg) ? JSON.stringify(arg) : arg
}

function namesFromPattern(pattern: string | null): string[] {
  if (!pattern) return []
  const wrapped = /^\^\((.*)\)\$$/.exec(pattern)
  if (!wrapped) return [pattern]
  return wrapped[1].split("|").filter((part) => part.length > 0)
}

function goRunFilter(names: string[] | null, pattern: string | null): string | null {
  if (pattern) return pattern
  if (!names || names.length === 0) return null
  return `^(${names.map(escapeRegExp).join("|")})$`
}

function uniqueSurefire(names: string[]): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  for (const name of names) {
    const selector = surefireTest(name)
    if (!selector || seen.has(selector)) continue
    seen.add(selector)
    out.push(selector)
  }
  return out
}

function surefireTest(name: string): string {
  const leaf = name.split("::").pop() ?? name
  const dot = leaf.lastIndexOf(".")
  const body = dot <= 0 ? leaf : `${leaf.slice(0, dot)}#${leaf.slice(dot + 1)}`
  // testShut[0] collects nothing. method(Parser)[1] keeps the parameter types.
  return body.replace(/\[[^\]]*\]$/, "")
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

function runShell(
  pkg: string,
  command: string,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  // A command can be pytest. Baseline writes a .pyc, and a same-size patch in that same second leaves it valid.
  clearBytecode(pkg)
  const child = /\b(java|javac|mvn)\b/.test(command) ? javaEnv(env) : env
  return spawnCollected("sh", ["-c", command], pkg, { ...child, PYTHONDONTWRITEBYTECODE: "1" }, timeoutMs)
}

/**
 * Node's `--test-timeout` caps the file, not only a test that left the option unset.
 * A test that sets a longer `timeout` is cancelled, and the failure name is the file path.
 * The flag stays at least that long, and it stays inside the suite cap.
 */
export function nodeTestTimeoutMs(pkg: string, files: string[], testTimeoutMs: number, suiteTimeoutMs: number): number {
  let declared = 0
  for (const rel of files) {
    let text = ""
    try {
      text = readFileSync(path.join(pkg, rel), "utf8")
    } catch {
      continue
    }
    for (const match of text.matchAll(/timeout\s*:\s*(\d[\d_]*)/g)) {
      const value = Number(match[1].replaceAll("_", ""))
      if (Number.isFinite(value) && value > declared) declared = value
    }
  }
  const floor = Number.isFinite(testTimeoutMs) && testTimeoutMs > 0 ? testTimeoutMs : 0
  let chosen = Math.max(floor, declared)
  if (Number.isFinite(suiteTimeoutMs) && suiteTimeoutMs > 0 && chosen > suiteTimeoutMs) chosen = suiteTimeoutMs
  return chosen > 0 ? chosen : floor
}

async function runNode(
  pkg: string,
  files: string[],
  pattern: string | null,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
  hooks: { reporterPath: string; testTimeoutMs: number; concurrency: number },
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const tsx = path.join(pkg, "node_modules", ".bin", "tsx")
  const bin = existsSync(tsx) ? tsx : process.execPath
  const prefix = existsSync(tsx) ? [] : ["--experimental-strip-types"]
  const hook = env.PROBATIO_COVERAGE_MAP ? fileURLToPath(new URL("./node-coverage.mjs", import.meta.url)) : null
  const fileTimeout = nodeTestTimeoutMs(pkg, files, hooks.testTimeoutMs, timeoutMs)
  const result = await spawnCollected(
    bin,
    [
      ...prefix,
      ...(hook ? ["--import", hook] : []),
      "--test",
      `--test-reporter=${hooks.reporterPath}`,
      `--test-concurrency=${hooks.concurrency}`,
      `--test-timeout=${fileTimeout}`,
      ...(pattern ? [`--test-name-pattern=${pattern}`] : []),
      ...files,
    ],
    pkg,
    env,
    timeoutMs,
  )
  if (env.PROBATIO_COVERAGE_MAP) mergeNodeCoverage(env.PROBATIO_COVERAGE_MAP)
  return result
}

function mergeNodeCoverage(mapPath: string): void {
  const dir = `${mapPath}.parts`
  const files: Record<string, Record<string, string[]>> = {}
  if (existsSync(dir)) {
    for (const name of readdirSync(dir)) {
      if (!name.endsWith(".json")) continue
      try {
        const parsed = JSON.parse(readFileSync(path.join(dir, name), "utf8")) as { files?: Record<string, Record<string, string[]>> }
        for (const [file, lines] of Object.entries(parsed.files ?? {})) {
          const bucket = files[file] ?? {}
          for (const [line, tests] of Object.entries(lines ?? {})) {
            bucket[line] = [...new Set([...(bucket[line] ?? []), ...(tests ?? [])])].sort()
          }
          files[file] = bucket
        }
      } catch {
        // A torn shard is ignored. An empty merge does not invent a map.
      }
    }
    rmSync(dir, { recursive: true, force: true })
  }
  if (Object.keys(files).length === 0) {
    const error = `${mapPath}.error`
    if (existsSync(error)) writeFileSync(path.join(path.dirname(mapPath), "coverage-error.txt"), readFileSync(error))
    return
  }
  writeFileSync(mapPath, `${JSON.stringify({ files }, null, 2)}\n`)
}

function runPython(
  pkg: string,
  files: string[],
  pattern: string | null,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
  script: string,
  nodeIds: string[] | null,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const reporter = fileURLToPath(new URL(`./${script}`, import.meta.url))
  const src = path.join(pkg, "src")
  clearBytecode(pkg)
  const next: NodeJS.ProcessEnv = {
    ...(existsSync(src)
      ? { ...env, PYTHONPATH: [src, env.PYTHONPATH].filter((item): item is string => Boolean(item)).join(path.delimiter) }
      : env),
    PYTHONDONTWRITEBYTECODE: "1",
    ...(env.PROBATIO_COVERAGE_MAP ? { COVERAGE_CORE: env.COVERAGE_CORE || "ctrace" } : {}),
  }
  const targets = nodeIds && nodeIds.length > 0 ? nodeIds : files
  return spawnCollected("python3", [reporter, ...targets, ...(pattern ? ["--pattern", pattern] : [])], pkg, next, timeoutMs)
}

async function runMocha(
  pkg: string,
  files: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
  names: string[] | null = null,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const mocha = ["mocha.js", "mocha"]
    .map((name) => path.join(pkg, "node_modules", "mocha", "bin", name))
    .find((file) => existsSync(file))
  if (!mocha) return Promise.resolve({ code: 1, stdout: "", stderr: "mocha is not installed", timedOut: false })
  const prepared = await preparePublished(pkg, env, timeoutMs)
  if (prepared && (prepared.code !== 0 || prepared.timedOut)) return prepared
  const config = readMochaConfig(pkg)
  const args = [mocha, ...buildMochaArgs(pkg, files)]
  if (names && names.length > 0) args.push("--grep", names.map(escapeRegExp).join("|"))
  if (env.PROBATIO_COVERAGE_MAP) args.push("--require", fileURLToPath(new URL("./mocha-coverage.cjs", import.meta.url)))
  if (config && config.require.includes("esm")) {
    // The `esm` loader throws on current Node before any test runs. Drop that one require and keep the rest of the project's mocha config.
    const requires = config.require.filter((name) => name !== "esm")
    const dir = path.join(pkg, ".probatio-suite")
    mkdirSync(dir, { recursive: true })
    const override = path.join(dir, "mocha-package.json")
    writeFileSync(
      override,
      `${JSON.stringify({ mocha: { extension: config.extension, require: requires, spec: config.spec } }, null, 2)}\n`,
    )
    args.push("--package", override)
  }
  return spawnCollected(process.execPath, args, pkg, env, timeoutMs)
}

/** Rebuild published JS when tests import the package entry and `src` is newer than that entry. */
async function preparePublished(
  pkg: string,
  env: NodeJS.ProcessEnv,
  timeoutMs: number,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean } | null> {
  const file = path.join(pkg, "package.json")
  if (!existsSync(file)) return null
  let body: { scripts?: Record<string, string>; types?: string; typings?: string; main?: string; module?: string; exports?: unknown }
  try {
    body = JSON.parse(readFileSync(file, "utf8")) as typeof body
  } catch {
    return null
  }
  const entries = publishedEntries(body).map((rel) => path.join(pkg, rel))
  if (entries.length === 0 || !publishedStale(pkg, entries)) return null
  const script = body.scripts?.bundle ? "bundle" : body.scripts?.build ? "build" : null
  if (!script) return null
  return spawnCollected("npm", ["run", script, "--silent"], pkg, env, timeoutMs)
}

function publishedEntries(body: { types?: string; typings?: string; main?: string; module?: string; exports?: unknown }): string[] {
  const raw: string[] = []
  for (const value of [body.types, body.typings, body.main, body.module]) {
    if (typeof value === "string") raw.push(value)
  }
  collectExportPaths(body.exports, raw)
  const out: string[] = []
  for (const value of raw) {
    if (!value.startsWith(".")) continue
    if (!/\.(?:js|mjs|cjs|d\.ts)$/.test(value)) continue
    const rel = value.replace(/^\.\//, "")
    if (!out.includes(rel)) out.push(rel)
  }
  return out
}

function collectExportPaths(value: unknown, out: string[]): void {
  if (typeof value === "string") {
    out.push(value)
    return
  }
  if (!value || typeof value !== "object") return
  for (const item of Object.values(value as Record<string, unknown>)) collectExportPaths(item, out)
}

function publishedStale(pkg: string, entries: string[]): boolean {
  if (entries.some((file) => !existsSync(file))) return true
  const src = path.join(pkg, "src")
  if (!existsSync(src)) return false
  let newest = 0
  const visit = (dir: string) => {
    for (const name of readdirSync(dir)) {
      if (name === "node_modules" || name === "dist") continue
      const full = path.join(dir, name)
      const info = statSync(full)
      if (info.isDirectory()) visit(full)
      else if (info.mtimeMs > newest) newest = info.mtimeMs
    }
  }
  visit(src)
  const oldest = Math.min(...entries.map((file) => statSync(file).mtimeMs))
  return newest > oldest
}

/** A `.pyc` from the previous mutant can outlive the patch when both share a one-second mtime. */
function clearBytecode(pkg: string): void {
  const skip = new Set(["node_modules", ".git", "dist", "target", ".build", "vendor"])
  const stack = [pkg]
  while (stack.length > 0) {
    const dir = stack.pop()
    if (!dir) continue
    let entries
    try {
      entries = readdirSync(dir, { withFileTypes: true })
    } catch {
      continue
    }
    for (const entry of entries) {
      if (skip.has(entry.name)) continue
      const full = path.join(dir, entry.name)
      if (entry.isDirectory()) {
        if (entry.name === "__pycache__") rmSync(full, { recursive: true, force: true })
        else stack.push(full)
      } else if (entry.name.endsWith(".pyc")) {
        rmSync(full, { force: true })
      }
    }
  }
}

let gnuScriptCache: boolean | null = null

function suiteNeedsTty(pkg: string): boolean {
  const files = new Set<string>()
  const rootScript = path.join(pkg, "run_tests.sh")
  if (existsSync(rootScript)) files.add(rootScript)
  try {
    for (const name of readdirSync(pkg)) {
      if (name.endsWith(".sh")) files.add(path.join(pkg, name))
    }
  } catch {
    // A missing directory is not a tty.
  }
  let entry = ""
  try {
    entry = readFileSync(rootScript, "utf8")
  } catch {
    entry = ""
  }
  for (const match of entry.matchAll(/(?:^|[\s"'`/])([\w.-]+\.sh)\b/g)) {
    const full = path.resolve(pkg, match[1])
    if (full.startsWith(pkg + path.sep) && existsSync(full)) files.add(full)
  }
  for (const file of files) {
    let body = ""
    try {
      body = readFileSync(file, "utf8")
    } catch {
      continue
    }
    if (dockerWantsTty(body)) return true
  }
  return false
}

function dockerWantsTty(text: string): boolean {
  return text.split("\n").some((line) => {
    if (!/\bdocker\s+run\b/.test(line)) return false
    return /(?:^|\s)--tty(?:\s|=|$)/.test(line) || /(?:^|\s)-[A-Za-z]*t[A-Za-z]*(?:\s|$)/.test(line)
  })
}

function gnuScript(): boolean {
  if (gnuScriptCache !== null) return gnuScriptCache
  const probe = spawnSync("script", ["--version"], { encoding: "utf8", timeout: 2000 })
  const text = `${probe.stdout ?? ""}\n${probe.stderr ?? ""}`
  gnuScriptCache = probe.status === 0 && /util-linux|\bGNU\b/.test(text)
  return gnuScriptCache
}

function scriptInvocation(pkg: string): { bin: string; args: string[]; command: string } {
  if (!suiteNeedsTty(pkg)) return { bin: "bash", args: ["run_tests.sh"], command: "bash run_tests.sh" }
  if (gnuScript()) {
    const args = ["-q", "-c", "bash run_tests.sh", "/dev/null"]
    return { bin: "script", args, command: shown("script", args) }
  }
  const args = ["-q", "/dev/null", "bash", "run_tests.sh"]
  return { bin: "script", args, command: shown("script", args) }
}

function runScript(
  pkg: string,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
  launched: { bin: string; args: string[] },
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const next = { ...env }
  const shim = writeDockerShim(pkg)
  if (shim) next.PATH = `${shim}${path.delimiter}${next.PATH ?? ""}`
  // This project's CI sets ARCH_NATIVE=1. Without it, ci/run_build.sh skips the tests.
  const inner = path.join(pkg, "test", "run_inner_tests.py")
  const build = path.join(pkg, "ci", "run_build.sh")
  if (!next.ARCH_NATIVE && existsSync(inner) && existsSync(build) && readFileSync(build, "utf8").includes("ARCH_NATIVE")) {
    next.ARCH_NATIVE = "1"
  }
  return spawnCollected(launched.bin, launched.args, pkg, next, timeoutMs)
}

function writeDockerShim(pkg: string): string | null {
  const found = spawnSync("sh", ["-c", "command -v docker"], { encoding: "utf8" })
  const real = found.stdout?.trim() ?? ""
  if (found.status !== 0 || !real) return null
  const dir = path.join(pkg, ".probatio-suite", "bin")
  mkdirSync(dir, { recursive: true })
  const file = path.join(dir, "docker")
  writeFileSync(file, dockerShimScript(real), { mode: 0o755 })
  return dir
}

function dockerShimScript(real: string): string {
  return `#!/bin/bash
set -euo pipefail
REAL=${JSON.stringify(real)}
if [[ "\${1:-}" != "build" ]]; then
  exec "$REAL" "$@"
fi
shift
args=()
context=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build-arg|--file|-f|--tag|-t|--target|--platform|--iidfile|--label|--network|--output|-o|--secret|--ssh|--cache-from|--cache-to|--build-context|--ulimit|--add-host)
      args+=("$1" "$2"); shift 2 ;;
    --build-arg=*|--file=*|--tag=*|--target=*|--platform=*|--network=*|--output=*)
      args+=("$1"); shift ;;
    --)
      shift; context="\${1:-}"; break ;;
    -*)
      args+=("$1"); shift ;;
    *)
      context="$1"; shift ;;
  esac
done
if [[ -z "$context" ]]; then
  exec "$REAL" build --pull=false "\${args[@]}"
fi
src=$(cd "$context" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp -a "$src"/. "$tmp"/
deps="$tmp/ci/install_deps.sh"
if [[ -f "$deps" ]] && grep -q "pip install --upgrade pip" "$deps"; then
  python3 - "$deps" << 'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1])
text = p.read_text()
old = "python3 -m pip install --upgrade pip\\npython3 -m pip install virtualenv\\n"
new = """python3 -m pip install 'pip==20.3.4'
printf '%s\\\\n' '[global]' 'constraint = /etc/pip-constraints.txt' > /etc/pip.conf
printf '%s\\\\n' 'virtualenv==16.7.10' 'psutil==5.6.7' 'python-prctl==1.8.1' > /etc/pip-constraints.txt
python3 -m pip install 'virtualenv==16.7.10'
"""
if old not in text:
    raise SystemExit(0)
p.write_text(text.replace(old, new, 1))
p.chmod(0o755)
PY
fi
"$REAL" build --pull=false "\${args[@]}" "$tmp"
`
}

function runSwift(
  pkg: string,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const args = ["test"]
  if (declaresOwnPredicate(pkg)) {
    const overlay = foundationPredicateOverlay()
    if (overlay) {
      args.push(
        "-Xswiftc",
        "-vfsoverlay",
        "-Xswiftc",
        overlay.file,
        "-Xswiftc",
        "-module-cache-path",
        "-Xswiftc",
        overlay.cache,
      )
    }
  }
  return spawnCollected("swift", args, pkg, env, timeoutMs)
}

function declaresOwnPredicate(pkg: string): boolean {
  return walk(pkg, (rel) => rel.endsWith(".swift")).some((rel) => /struct\s+Predicate\s*</.test(readFileSync(path.join(pkg, rel), "utf8")))
}

function foundationPredicateOverlay(): { file: string; cache: string } | null {
  const sdk = spawnSync("xcrun", ["--sdk", "macosx", "--show-sdk-path"], { encoding: "utf8" })
  const sdkPath = sdk.stdout?.trim() ?? ""
  if (sdk.status !== 0 || !sdkPath) return null
  const moduleDir = path.join(sdkPath, "System/Library/Frameworks/Foundation.framework/Modules/Foundation.swiftmodule")
  if (!existsSync(moduleDir)) return null
  const interfaces = readdirSync(moduleDir).filter((name) => name.endsWith(".swiftinterface"))
  if (interfaces.length === 0) return null
  const stamp = String(statSync(path.join(moduleDir, interfaces[0])).mtimeMs)
  const root = path.join(tmpdir(), "probatio-foundation-overlay")
  const edited = path.join(root, "interfaces")
  const file = path.join(root, "overlay.json")
  const cache = path.join(root, "cache")
  const stampFile = path.join(root, "stamp")
  mkdirSync(cache, { recursive: true })
  if (!existsSync(stampFile) || readFileSync(stampFile, "utf8") !== stamp || !existsSync(file)) {
    mkdirSync(edited, { recursive: true })
    const contents: Array<{ name: string; type: string; "external-contents": string }> = []
    for (const name of readdirSync(moduleDir).sort()) {
      const original = path.join(moduleDir, name)
      if (!statSync(original).isFile()) continue
      let external = original
      if (name.endsWith(".swiftinterface")) {
        external = path.join(edited, name)
        writeFileSync(external, hideFoundationPredicate(readFileSync(original, "utf8")))
      }
      contents.push({ name, type: "file", "external-contents": external })
    }
    writeFileSync(file, `${JSON.stringify({ version: 0, roots: [{ name: moduleDir, type: "directory", contents }] }, null, 2)}\n`)
    writeFileSync(stampFile, stamp)
  }
  return { file, cache }
}

function hideFoundationPredicate(text: string): string {
  return text
    .split("public struct Predicate<each Input>")
    .join("public struct HiddenPredicate<each Input>")
    .split("public macro Predicate<each Input>")
    .join("public macro HiddenPredicate<each Input>")
    .replace(/Foundation::Predicate(?![A-Za-z])/g, "Foundation::HiddenPredicate")
}

function readMochaConfig(pkg: string): { extension?: string[]; require: string[]; spec?: string[] } | null {
  const file = path.join(pkg, "package.json")
  if (!existsSync(file)) return null
  try {
    const body = JSON.parse(readFileSync(file, "utf8")) as {
      mocha?: { extension?: string[]; require?: string[]; spec?: string[] }
    }
    const mocha = body.mocha
    if (!mocha) return null
    const spec = Array.isArray(mocha.spec) ? mocha.spec : undefined
    const require = Array.isArray(mocha.require) ? mocha.require : []
    if ((!spec || spec.length === 0) && require.length === 0) return null
    return { extension: mocha.extension, require, spec }
  } catch {
    return null
  }
}

async function runDotnetTest(
  pkg: string,
  spec: SuiteSpec,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
  names: string[] | null = null,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const child = dotnetEnv(env)
  await ensureDotnetTools(pkg, child, timeoutMs)
  const built = await ensureSiblingExes(pkg, spec, child, timeoutMs)
  if (built) return built
  const args = ["test", "--nologo", "--verbosity", "detailed"]
  if (spec.project) args.push(spec.project)
  if (names && names.length > 0) args.push("--filter", names.map((name) => `FullyQualifiedName~${name}`).join("|"))
  return spawnCollected("dotnet", args, pkg, child, timeoutMs)
}

async function runDotnetExe(
  pkg: string,
  spec: SuiteSpec,
  files: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const child = dotnetEnv(env)
  await ensureDotnetTools(pkg, child, timeoutMs)
  const project = spec.project ?? walk(pkg, (rel) => rel.endsWith(".csproj") && (hasSegment(rel, "tests") || hasSegment(rel, "test")))[0]
  const target = project ?? path.dirname(files[0] ?? ".")
  return spawnCollected("dotnet", ["run", "--project", target, "--nologo"], pkg, child, timeoutMs)
}

function dotnetEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  if (env.DOTNET_ROLL_FORWARD) return env
  return { ...env, DOTNET_ROLL_FORWARD: "Major" }
}

/** Build a tool dll that a test project execs in PreBuild when that dll is not a ProjectReference. */
async function ensureDotnetTools(pkg: string, env: NodeJS.ProcessEnv, timeoutMs: number): Promise<void> {
  const root = repoRoot(pkg)
  const projects = walk(root, (rel) => rel.endsWith(".csproj"))
  const missing = new Set<string>()
  const toolRe = /([A-Za-z0-9_.-]+)[/\\]bin[/\\]\$\(Configuration\)[/\\]([A-Za-z0-9_.-]+)\.dll/g
  for (const rel of projects) {
    const text = readFileSync(path.join(root, rel), "utf8")
    for (const match of text.matchAll(toolRe)) {
      const folder = match[1]
      const asm = match[2]
      const beside = path.join(root, path.dirname(rel), folder, "bin")
      const top = path.join(root, folder, "bin")
      if (toolDllExists(beside, asm) || toolDllExists(top, asm)) continue
      const found = projects.find((item) => path.posix.basename(item) === `${asm}.csproj` && item.split("/").includes(folder))
      if (found) missing.add(found)
    }
  }
  for (const rel of missing) {
    await spawnCollected("dotnet", ["build", rel, "-c", "Debug", "--nologo"], root, env, timeoutMs)
  }
}

function toolDllExists(bin: string, asm: string): boolean {
  return ["Debug", "Release"].some((cfg) => existsSync(path.join(bin, cfg, `${asm}.dll`)))
}

/**
 * Build an Exe in this package that the test project does not reference.
 * Suites often start that dll by a path under bin/$(Configuration), so `dotnet test` never compiles it.
 */
async function ensureSiblingExes(
  pkg: string,
  spec: SuiteSpec,
  env: NodeJS.ProcessEnv,
  timeoutMs: number,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean } | null> {
  const referenced = spec.project ? referencedProjects(pkg, spec.project) : new Set<string>()
  for (const rel of walk(pkg, (item) => item.endsWith(".csproj"))) {
    if (rel === spec.project || referenced.has(rel)) continue
    const text = readFileSync(path.join(pkg, rel), "utf8")
    if (!/<OutputType>\s*Exe\s*<\/OutputType>/i.test(text)) continue
    const built = await spawnCollected("dotnet", ["build", rel, "-c", "Debug", "--nologo"], pkg, env, timeoutMs)
    if (built.code !== 0 || built.timedOut) return built
  }
  return null
}

function referencedProjects(pkg: string, projectRel: string): Set<string> {
  const text = readFileSync(path.join(pkg, projectRel), "utf8")
  const dir = path.dirname(path.join(pkg, projectRel))
  const out = new Set<string>()
  for (const match of text.matchAll(/<ProjectReference\b[^>]*\bInclude\s*=\s*"([^"]+)"/gi)) {
    const abs = path.resolve(dir, match[1])
    out.add(path.relative(pkg, abs).split(path.sep).join("/"))
  }
  return out
}

function repoRoot(dir: string): string {
  let current = dir
  while (true) {
    if (existsSync(path.join(current, ".git"))) return current
    const parent = path.dirname(current)
    if (parent === current) return dir
    current = parent
  }
}

async function compileAndRun(
  pkg: string,
  files: string[],
  cxx: boolean,
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const out = path.join(pkg, ".probatio-suite")
  mkdirSync(out, { recursive: true })
  const bin = path.join(out, "suite")
  const tool = cxx ? "clang++" : "clang"
  const compiled = await spawnCollected(tool, [cxx ? "-std=c++17" : "-std=c11", "-o", bin, ...abs(pkg, files)], pkg, env, timeoutMs)
  if (compiled.code !== 0 || compiled.timedOut) return compileFailed(compiled)
  return spawnCollected(bin, [], pkg, env, timeoutMs)
}

async function compileJava(
  pkg: string,
  files: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const runtime = javaEnv(env)
  const out = path.join(pkg, ".probatio-suite")
  mkdirSync(out, { recursive: true })
  const javac = path.join(runtime.JAVA_HOME ?? "", "bin", "javac")
  const java = path.join(runtime.JAVA_HOME ?? "", "bin", "java")
  const compiler = existsSync(javac) ? javac : "javac"
  const runner = existsSync(java) ? java : "java"
  const compiled = await spawnCollected(compiler, ["-d", out, ...abs(pkg, files)], pkg, runtime, timeoutMs)
  if (compiled.code !== 0 || compiled.timedOut) return compileFailed(compiled)
  const main = javaMain(pkg, files) ?? "Main"
  return spawnCollected(runner, ["-cp", out, main], pkg, runtime, timeoutMs)
}

async function compileSwift(
  pkg: string,
  files: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const out = path.join(pkg, ".probatio-suite")
  mkdirSync(out, { recursive: true })
  const bin = path.join(out, "suite")
  const compiled = await spawnCollected("swiftc", ["-o", bin, ...abs(pkg, files)], pkg, env, timeoutMs)
  if (compiled.code !== 0 || compiled.timedOut) return compileFailed(compiled)
  return spawnCollected(bin, [], pkg, env, timeoutMs)
}

async function compileCobol(
  pkg: string,
  files: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const out = path.join(pkg, ".probatio-suite")
  mkdirSync(out, { recursive: true })
  const bin = path.join(out, "suite")
  const source = abs(pkg, files)[0]
  const free = readFileSync(source, "utf8").includes("SOURCE FORMAT FREE")
  const compiled = await spawnCollected("cobc", ["-x", ...(free ? ["-free"] : []), "-o", bin, source], pkg, env, timeoutMs)
  if (compiled.code !== 0 || compiled.timedOut) return compileFailed(compiled)
  return spawnCollected(bin, [], pkg, env, timeoutMs)
}

async function assembleX86(
  pkg: string,
  files: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const out = path.join(pkg, ".probatio-suite")
  mkdirSync(out, { recursive: true })
  const obj = path.join(out, "suite.o")
  const bin = path.join(out, "suite")
  const assembled = await spawnCollected("nasm", ["-f", "macho64", "-o", obj, abs(pkg, files)[0]], pkg, env, timeoutMs)
  if (assembled.code !== 0 || assembled.timedOut) return compileFailed(assembled)
  const linked = await spawnCollected("clang", ["-arch", "x86_64", "-o", bin, obj], pkg, env, timeoutMs)
  if (linked.code !== 0 || linked.timedOut) return compileFailed(linked)
  return spawnCollected("arch", ["-x86_64", bin], pkg, env, timeoutMs)
}

async function assembleArm(
  pkg: string,
  files: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const out = path.join(pkg, ".probatio-suite")
  mkdirSync(out, { recursive: true })
  const bin = path.join(out, "suite")
  const assembled = await spawnCollected("clang", ["-arch", "arm64", "-o", bin, abs(pkg, files)[0]], pkg, env, timeoutMs)
  if (assembled.code !== 0 || assembled.timedOut) return compileFailed(assembled)
  return spawnCollected(bin, [], pkg, env, timeoutMs)
}

async function assembleRiscv(
  pkg: string,
  files: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  const out = path.join(pkg, ".probatio-suite")
  mkdirSync(out, { recursive: true })
  const obj = path.join(out, "suite.o")
  const bin = path.join(out, "suite")
  const assembler = commandExists("riscv64-elf-as") ? "riscv64-elf-as" : "riscv64-unknown-elf-as"
  const linker = commandExists("riscv64-elf-ld") ? "riscv64-elf-ld" : "riscv64-unknown-elf-ld"
  const assembled = await spawnCollected(assembler, ["-o", obj, abs(pkg, files)[0]], pkg, env, timeoutMs)
  if (assembled.code !== 0 || assembled.timedOut) return compileFailed(assembled)
  // Homebrew QEMU on macOS ships qemu-system-riscv64 and no linux-user binary.
  // The virt machine loads the kernel at 0x80000000. The fixture exits by writing
  // (code << 16) | 0x5555 to the SiFive test finisher at 0x100000.
  const linkScript = path.join(out, "link.ld")
  writeFileSync(
    linkScript,
    ["OUTPUT_ARCH(riscv)", "ENTRY(_start)", "SECTIONS", "{", "  . = 0x80000000;", "  .text : { *(.text*) }", "}", ""].join("\n"),
  )
  const linked = await spawnCollected(linker, ["-nostdlib", "-T", linkScript, "-o", bin, obj], pkg, env, timeoutMs)
  if (linked.code !== 0 || linked.timedOut) return compileFailed(linked)
  if (commandExists("qemu-riscv64")) return spawnCollected("qemu-riscv64", [bin], pkg, env, timeoutMs)
  if (commandExists("qemu-riscv64-static")) return spawnCollected("qemu-riscv64-static", [bin], pkg, env, timeoutMs)
  return spawnCollected(
    "qemu-system-riscv64",
    ["-machine", "virt", "-nographic", "-bios", "none", "-kernel", bin, "-serial", "none", "-monitor", "none", "-display", "none"],
    pkg,
    env,
    timeoutMs,
  )
}

function compileFailed(result: { code: number; stdout: string; stderr: string; timedOut: boolean }): {
  code: number
  stdout: string
  stderr: string
  timedOut: boolean
} {
  return { ...result, stderr: `PROB_COMPILE_FAILED\n${result.stderr}` }
}

/** A compiler token when this run failed before any test result exists. Null when tests ran. */
export function detectCompile(kind: string, stdout: string, stderr: string, code: number): string | null {
  if (code === 0) return null
  const text = `${stdout}\n${stderr}`
  if (text.includes("PROB_COMPILE_FAILED")) return compilerToken(kind)
  if (kind === "maven" && /COMPILATION ERROR|Compilation failure/.test(text) && !/Tests run:\s*\d+/.test(text)) return "javac"
  if (kind === "cargo" && /could not compile|error\[E\d+\]/.test(text) && !/^test \S+ \.\.\. (ok|FAILED)/m.test(text)) return "cargo"
  if (kind === "swift" && /\berror:/.test(text) && !/Test Case '[^']+' (passed|failed)/.test(text)) return "swiftc"
  if ((kind === "dotnet" || kind === "dotnet-exe") && /error CS\d+|error MSB\d+/.test(text) && !/^\s*(Passed|Failed)\s+\S+/m.test(text)) return "dotnet"
  if (kind === "go" && /\.go:\d+:\d+:/.test(text) && !/"Test"\s*:/.test(text)) return "build"
  if ((kind === "pytest" || kind === "unittest") && /SyntaxError|IndentationError/.test(text) && jsonReport(stdout) === null) return "compiler"
  if (kind === "node" && /SyntaxError/.test(text) && jsonReport(stdout) === null) return "compiler"
  if (kind === "mocha" && /error TS\d+|Unable to compile/.test(text) && !/"tests"\s*:/.test(text)) return "tsc"
  if (kind === "make-test" && !/Tests run:\s*\d+/.test(text) && /error:|cobc:|syntax error/i.test(text)) return "cobc"
  // A shell suite that dies in the compiler never prints a test title. That is a compile kill.
  if (kind === "script" && !scriptReported(text) && /:\s*error:|fatal error:|undefined reference to|\[-Werror/.test(text)) return "clang"
  return null
}

function scriptReported(text: string): boolean {
  const testLine =
    /^(Testing (?!with FORCE_SUBREAPER\b).+|Running (?:exit code|reaping|process group|zombie|signal configuration|parent death).+|running signal test .+)$/
  return text.split(/\r?\n/).some((raw) => testLine.test(scriptLine(raw)))
}

function scriptLine(raw: string): string {
  // `script` prints ^D and a backspace before the program's own line.
  const stripped = raw.replace(/\u001b\[[0-9;?]*[A-Za-z]/g, "").replace(/[\u0000-\u0008\u000B-\u001F\u007F]/g, "")
  return stripped.replace(/^\^D+/, "").trim()
}

function compilerToken(kind: string): string {
  if (kind === "asm-x86_64") return "nasm"
  if (kind === "cobol" || kind === "make-test") return "cobc"
  if (kind === "java" || kind === "maven") return "javac"
  if (kind === "swift" || kind === "swiftc") return "swiftc"
  if (kind === "dotnet" || kind === "dotnet-exe") return "dotnet"
  if (kind === "cargo") return "cargo"
  if (kind === "c" || kind === "cpp" || kind === "asm-aarch64") return "clang"
  if (kind === "mocha") return "tsc"
  return "build"
}

function adapt(kind: string, pkg: string, stdout: string, stderr: string, code: number): TestReport | null {
  if (stderr.includes("PROB_COMPILE_FAILED")) return null
  if (kind === "node" || kind === "unittest" || kind === "pytest") return jsonReport(stdout)
  if (kind === "go") return parseGo(stdout)
  if (kind === "cargo") return parseCargo(`${stdout}\n${stderr}`)
  if (kind === "swift") return parseSwift(`${stdout}\n${stderr}`, code)
  if (kind === "script") return parseScriptReport(`${stdout}\n${stderr}`, code)
  if (kind === "maven") return mavenReport(pkg, `${stdout}\n${stderr}`, code)
  if (kind === "dotnet") return parseDotnet(`${stdout}\n${stderr}`, code)
  if (kind === "mocha") return parseMocha(stdout)
  if (kind === "make-test" || kind === "command") {
    const text = `${stdout}\n${stderr}`
    const ctest = parseCtest(text)
    if (ctest) return ctest
    if (/Tests run:\s*\d+/.test(text)) return parseMakeTest(text)
    if (kind === "make-test") return parseMakeTest(text)
    return exitReport(kind, code)
  }
  return exitReport(kind, code)
}

function jsonReport(stdout: string): TestReport | null {
  const lines = stdout
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("{"))
  if (lines.length === 0) return null
  try {
    const parsed = JSON.parse(lines[lines.length - 1]) as TestReport
    if (!Array.isArray(parsed.failed) || !Array.isArray(parsed.names)) return null
    if (parsed.names.length === 0) return null
    return parsed
  } catch {
    return null
  }
}

function parseGo(stdout: string): TestReport | null {
  const names: string[] = []
  const failed: TestReport["failed"] = []
  for (const line of stdout.split("\n")) {
    if (!line.startsWith("{")) continue
    try {
      const event = JSON.parse(line) as { Action?: string; Test?: string; Package?: string }
      if (!event.Test) continue
      if (event.Action === "run" && !names.includes(event.Test)) names.push(event.Test)
      if (event.Action === "fail") failed.push({ name: event.Test, file: event.Package ?? "", line: 0 })
    } catch {
      continue
    }
  }
  if (names.length === 0) return null
  return { tests: names.length, pass: names.length - failed.length, fail: failed.length, failed, names }
}

function parseCargo(text: string): TestReport | null {
  const names: string[] = []
  const failed: TestReport["failed"] = []
  for (const line of text.split("\n")) {
    const match = line.match(/^test (\S+) \.\.\. (ok|FAILED)/)
    if (!match) continue
    names.push(match[1])
    if (match[2] === "FAILED") failed.push({ name: match[1], file: "", line: 0 })
  }
  if (names.length === 0) return null
  return { tests: names.length, pass: names.length - failed.length, fail: failed.length, failed, names }
}

function parseScriptReport(text: string, code: number): TestReport | null {
  const names: string[] = []
  const testLine =
    /^(Testing (?!with FORCE_SUBREAPER\b).+|Running (?:exit code|reaping|process group|zombie|signal configuration|parent death).+|running signal test .+)$/
  for (const raw of text.split(/\r?\n/)) {
    const line = scriptLine(raw)
    if (!testLine.test(line) || names.includes(line)) continue
    names.push(line)
  }
  if (names.length === 0) return null
  const failed = text.includes("All done, tests as expected") || code === 0 ? [] : [{ name: names[names.length - 1], file: "", line: 0 }]
  return { tests: names.length, pass: names.length - failed.length, fail: failed.length, failed, names }
}

function parseSwift(text: string, code: number): TestReport | null {
  const names: string[] = []
  const failed: TestReport["failed"] = []
  for (const match of text.matchAll(/Test Case '([^']+)' (passed|failed)/g)) {
    names.push(match[1])
    if (match[2] === "failed") failed.push({ name: match[1], file: "", line: 0 })
  }
  for (const match of text.matchAll(/[✔✘]\s+([^\s(]+)/g)) {
    if (names.includes(match[1])) continue
    names.push(match[1])
    if (match[0].includes("✘")) failed.push({ name: match[1], file: "", line: 0 })
  }
  if (names.length === 0) return null
  return { tests: names.length, pass: names.length - failed.length, fail: failed.length, failed, names }
}

function parseMocha(stdout: string): TestReport | null {
  const start = stdout.indexOf("{")
  if (start < 0) return null
  try {
    const body = JSON.parse(stdout.slice(start)) as {
      tests?: Array<{ fullTitle?: string; title?: string; file?: string; err?: unknown }>
      failures?: Array<{ fullTitle?: string; title?: string; file?: string }>
    }
    const tests = body.tests ?? []
    const names = tests.map((item) => item.fullTitle || item.title || "").filter((name) => name.length > 0)
    const failed = (body.failures ?? []).map((item) => ({
      name: item.fullTitle || item.title || "",
      file: item.file ?? "",
      line: 0,
    }))
    if (names.length === 0) return null
    return { tests: names.length, pass: names.length - failed.length, fail: failed.length, failed, names }
  } catch {
    return null
  }
}

/** CTest prints `Test #N: name` and `N - name (Failed)`. Those names are the kill, not `::command`. */
function parseCtest(text: string): TestReport | null {
  if (!/Test #\d+:|The following tests FAILED:/.test(text)) return null
  const names: string[] = []
  const failed: TestReport["failed"] = []
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    const ran = /^\d+\/\d+\s+Test\s+#\d+:\s+(\S+)/.exec(line)
    if (ran && !names.includes(ran[1])) names.push(ran[1])
    if (ran && /\*\*\*Failed/.test(line) && !failed.some((item) => item.name === ran[1])) {
      failed.push({ name: ran[1], file: "", line: 0 })
    }
    const listed = /^\d+\s+-\s+(\S+)\s+\(Failed\)/.exec(line)
    if (listed && !failed.some((item) => item.name === listed[1])) failed.push({ name: listed[1], file: "", line: 0 })
  }
  if (names.length === 0 && failed.length === 0) return null
  for (const item of failed) {
    if (!names.includes(item.name)) names.push(item.name)
  }
  return { tests: names.length, pass: Math.max(names.length - failed.length, 0), fail: failed.length, failed, names }
}

function parseMakeTest(text: string): TestReport | null {
  const run = text.match(/Tests run:\s*(\d+)/)
  if (!run) return null
  const tests = Number(run[1])
  if (!Number.isFinite(tests) || tests <= 0) return null
  // Progress logs print Case: for every case. Only the Failures section is a result.
  // Tests failed: 0 is a green run even when those progress lines are present.
  const failedCountMatch = text.match(/Tests failed:\s*(\d+)/)
  const failCount = failedCountMatch ? Number(failedCountMatch[1]) : null
  const failed: TestReport["failed"] = []
  if (failCount !== 0) {
    const parts = text.split(/^Failures:\s*$/m)
    const failureLog = parts.length > 1 ? parts.slice(1).join("\n") : ""
    let suite = ""
    let unit = ""
    for (const raw of failureLog.split(/\r?\n/)) {
      const line = raw.trim()
      const suiteName = /^Suite:\s*(.*)$/.exec(line)
      if (suiteName) {
        suite = suiteName[1].trim()
        continue
      }
      const unitName = /^Test:\s*(.*)$/.exec(line)
      if (unitName) {
        unit = unitName[1].trim()
        continue
      }
      const caseName = /^Case:\s*(.*)$/.exec(line)
      if (!caseName) continue
      const name = [suite, unit, caseName[1].trim()].filter((part) => part.length > 0).join("::")
      failed.push({ name: name || caseName[1].trim(), file: "", line: 0 })
    }
    if (failed.length === 0 && failCount !== null && failCount > 0) failed.push({ name: "failed", file: "", line: 0 })
  }
  const names = failed.map((item) => item.name)
  while (names.length < tests) names.push(`passed-${names.length + 1}`)
  return { tests, pass: Math.max(tests - failed.length, 0), fail: failed.length, failed, names }
}

function parseDotnet(text: string, code: number): TestReport | null {
  const names: string[] = []
  const failed: TestReport["failed"] = []
  for (const line of text.split("\n")) {
    // A result line carries a duration: "Passed StaysClosed [4 ms]".
    // "Failed to load ..." is a diagnostic, not a test, and must not redden the baseline.
    const match = line.match(/^\s*(Passed|Failed)\s+(.+?)\s+\[(?:<\s*)?\d+(?:\.\d+)?\s*(?:ms|s)\]\s*$/)
    if (!match) continue
    names.push(match[2])
    if (match[1] === "Failed") failed.push({ name: match[2], file: "", line: 0 })
  }
  if (names.length === 0) return null
  return { tests: names.length, pass: names.length - failed.length, fail: failed.length, failed, names }
}

function mavenReport(pkg: string, text: string, code: number): TestReport | null {
  // A compile error exits before Surefire rewrites target/surefire-reports, so the
  // previous run's XML is still a green report. That is not this mutant's result.
  if (code !== 0 && /COMPILATION ERROR|Compilation failure/.test(text) && !/Tests run:\s*\d+/.test(text)) {
    return { tests: 1, pass: 0, fail: 1, failed: [{ name: "javac", file: "", line: 0 }], names: ["javac"] }
  }
  return parseSurefire(pkg) ?? parseMavenText(text)
}

function parseSurefire(pkg: string): TestReport | null {
  const dir = path.join(pkg, "target", "surefire-reports")
  if (!existsSync(dir)) return null
  const names: string[] = []
  const failed: TestReport["failed"] = []
  for (const name of readdirSync(dir)) {
    if (!name.endsWith(".xml")) continue
    const xml = readFileSync(path.join(dir, name), "utf8")
    for (const part of xml.split("<testcase ").slice(1)) {
      const test = /name="([^"]+)"/.exec(part)?.[1]
      const klass = /classname="([^"]+)"/.exec(part)?.[1] ?? ""
      if (!test) continue
      const id = `${klass}.${test}`
      names.push(id)
      if (part.includes("<failure") || part.includes("<error")) failed.push({ name: id, file: klass, line: 0 })
    }
  }
  if (names.length === 0) return null
  return { tests: names.length, pass: names.length - failed.length, fail: failed.length, failed, names }
}

function parseMavenText(text: string): TestReport | null {
  const match = text.match(/Tests run:\s*(\d+),\s*Failures:\s*(\d+),\s*Errors:\s*(\d+)/)
  if (!match) return null
  const tests = Number(match[1])
  const fail = Number(match[2]) + Number(match[3])
  if (tests === 0) return null
  const names = Array.from({ length: tests }, (_, index) => `maven-${index + 1}`)
  const failed = names.slice(0, fail).map((name) => ({ name, file: "pom.xml", line: 0 }))
  return { tests, pass: tests - fail, fail, failed, names }
}

function exitReport(kind: string, code: number): TestReport {
  const name = kind
  const failed = code === 0 ? [] : [{ name, file: "", line: 0 }]
  return { tests: 1, pass: code === 0 ? 1 : 0, fail: failed.length, failed, names: [name] }
}

function javaMain(pkg: string, files: string[]): string | null {
  for (const rel of files) {
    const text = readFileSync(path.join(pkg, rel), "utf8")
    const match = text.match(/public\s+class\s+(\w+)/)
    if (match && text.includes("static void main")) return match[1]
  }
  return null
}

function abs(pkg: string, files: string[]): string[] {
  return files.map((rel) => path.join(pkg, rel))
}

function walk(root: string, accept: (rel: string) => boolean): string[] {
  const out: string[] = []
  const visit = (dir: string) => {
    let entries: string[]
    try {
      entries = readdirSync(dir)
    } catch {
      return
    }
    for (const entry of entries.sort()) {
      if (SKIP_DIR.has(entry)) continue
      const full = path.join(dir, entry)
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
      const rel = path.relative(root, full).split(path.sep).join("/")
      if (accept(rel)) out.push(rel)
    }
  }
  visit(root)
  return out
}

function javaEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  if (env.JAVA_HOME && existsSync(path.join(env.JAVA_HOME, "bin", "java"))) return env
  const prefix = brewPrefix("openjdk")
  if (!prefix) return env
  return { ...env, JAVA_HOME: prefix, PATH: `${path.join(prefix, "bin")}${path.delimiter}${env.PATH ?? ""}` }
}

function brewPrefix(formula: string): string | null {
  const result = spawnSync("brew", ["--prefix", formula], { encoding: "utf8" })
  if (result.status !== 0) return null
  const prefix = result.stdout.trim()
  return prefix && existsSync(prefix) ? prefix : null
}

function commandExists(name: string): boolean {
  const result = spawnSync("sh", ["-c", `command -v ${name}`], { encoding: "utf8" })
  return result.status === 0 && result.stdout.trim().length > 0
}

function firstLine(text: string): string {
  return text.trim().split("\n").find((line) => line.trim()) ?? ""
}

function spawnCollected(
  bin: string,
  args: string[],
  cwd: string,
  env: NodeJS.ProcessEnv,
  timeoutMs: number,
): Promise<{ code: number; stdout: string; stderr: string; timedOut: boolean }> {
  return new Promise((resolve) => {
    const child = spawn(bin, args, { cwd, env, detached: true, stdio: ["ignore", "pipe", "pipe"] })
    let stdout = ""
    let stderr = ""
    let settled = false
    let timedOut = false
    const finish = (code: number) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      resolve({ code, stdout, stderr, timedOut })
    }
    const timer = setTimeout(() => {
      timedOut = true
      try {
        if (child.pid) process.kill(-child.pid, "SIGKILL")
      } catch {
        child.kill("SIGKILL")
      }
    }, timeoutMs)
    child.stdout?.on("data", (chunk) => {
      stdout += chunk
    })
    child.stderr?.on("data", (chunk) => {
      stderr += chunk
    })
    child.on("error", (error) => {
      stderr += error.message
      finish(1)
    })
    child.on("close", (code) => finish(code ?? 1))
  })
}
