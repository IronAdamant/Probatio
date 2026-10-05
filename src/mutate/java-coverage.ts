import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import type { CoverageMap } from "./coverage-map.js"

const JACOCO = "0.8.15"
const ASM = "9.10.1"
const RUNNER = fileURLToPath(new URL("./probatio-jacoco-run.java", import.meta.url))

/**
 * One JVM, one JaCoCo dump per Surefire test method.
 * The default jacoco.exec file does not name the test.
 */
export function writeJavaCoverage(pkg: string, testNames: string[], dest: string): { ok: boolean; detail: string } {
  const tests = testNames.map(junitMethod).filter((name): name is string => name !== null)
  if (tests.length === 0) return { ok: false, detail: "java coverage: no class.method test names" }
  const classDirs = findClassDirs(pkg)
  if (classDirs.length === 0) return { ok: false, detail: "java coverage: no target/classes" }
  const work = path.join(path.dirname(dest), "java-cover")
  mkdirSync(work, { recursive: true })
  const jars = resolveJars(pkg, work)
  if ("error" in jars) return { ok: false, detail: jars.error }
  const source = path.join(work, "ProbatioJacocoRun.java")
  writeFileSync(source, readFileSync(RUNNER))
  const compileCp = [jars.agent, jars.junit, jars.core, jars.asm, jars.asmCommons, jars.asmTree].join(path.delimiter)
  const compiled = run(javaTool("javac"), ["-encoding", "UTF-8", "-cp", compileCp, "-d", work, source], pkg)
  if (compiled.status !== 0) return { ok: false, detail: clip(compiled.stderr || compiled.stdout || compiled.error?.message || "javac failed") }
  const testsFile = path.join(work, "tests.txt")
  writeFileSync(testsFile, `${tests.join("\n")}\n`)
  const projectCp = buildClasspath(pkg, work)
  if ("error" in projectCp) return { ok: false, detail: projectCp.error }
  // Surefire brings the platform launcher. A project's test classpath often has the Jupiter engine and not the launcher.
  const launcher = platformLauncherJar(pkg, work, projectCp.cp)
  if ("error" in launcher) return { ok: false, detail: launcher.error }
  // Our asm and JaCoCo must win over a project that still ships an older asm. That older reader rejects Java 27 class files.
  const cp = [work, jars.core, jars.asm, jars.asmCommons, jars.asmTree, jars.junit, launcher.jar, ...classDirs, ...testClassDirs(pkg), projectCp.cp].filter(Boolean).join(path.delimiter)
  const dump = path.join(work, "hits.txt")
  const ran = run(
    javaTool("java"),
    [`-javaagent:${jars.agent}=output=none,excludes=ProbatioJacocoRun:org.junit.*:org.apiguardian.*:org.opentest4j.*`, "-cp", cp, "ProbatioJacocoRun", classDirs.join(path.delimiter), testsFile, dump, pkg],
    pkg,
    900_000,
  )
  if (ran.error && (ran.error as { code?: string }).code === "ETIMEDOUT") {
    return { ok: false, detail: "java coverage: timed out" }
  }
  if (ran.status !== 0 || !existsSync(dump)) {
    return { ok: false, detail: clip(ran.stderr || ran.stdout || ran.error?.message || "java coverage runner failed") }
  }
  const files = hitsToMap(readFileSync(dump, "utf8"))
  if (Object.keys(files).length === 0) return { ok: false, detail: "java coverage: no covered lines" }
  writeFileSync(dest, `${JSON.stringify({ files }, null, 2)}\n`)
  return { ok: true, detail: `${Object.keys(files).length} files` }
}

export function hitsToMap(text: string): CoverageMap["files"] {
  const files: CoverageMap["files"] = {}
  for (const raw of text.split("\n")) {
    const line = raw.trim()
    if (!line.startsWith("HIT ")) continue
    const parts = line.slice(4).split(" ")
    if (parts.length < 3) continue
    const test = parts.pop() ?? ""
    const number = parts.pop() ?? ""
    const file = parts.join(" ")
    if (!file || !/^\d+$/.test(number) || !test) continue
    const lines = files[file] ?? {}
    const names = lines[number] ?? []
    if (!names.includes(test)) names.push(test)
    lines[number] = names
    files[file] = lines
  }
  return files
}

function junitMethod(name: string): string | null {
  const leaf = name.split("::").pop() ?? name
  const dot = leaf.lastIndexOf(".")
  if (dot <= 0 || dot === leaf.length - 1) return null
  if (leaf.startsWith("maven-") || leaf === "javac") return null
  return `${leaf.slice(0, dot)}#${leaf.slice(dot + 1)}`
}

function findClassDirs(root: string): string[] {
  const found: string[] = []
  const walk = (dir: string, depth: number) => {
    if (depth > 4) return
    let entries: string[] = []
    try {
      entries = readdirSync(dir)
    } catch {
      return
    }
    for (const name of entries) {
      if (name === "node_modules" || name === ".git" || name === "java-cover") continue
      const full = path.join(dir, name)
      if (name === "target") {
        const classes = path.join(full, "classes")
        if (existsSync(classes)) found.push(classes)
        continue
      }
      if (name.startsWith(".")) continue
      walk(full, depth + 1)
    }
  }
  walk(root, 0)
  return found
}

function testClassDirs(root: string): string[] {
  return findClassDirs(root).map((dir) => path.join(path.dirname(dir), "test-classes")).filter((dir) => existsSync(dir))
}

function resolveJars(pkg: string, work: string): { agent: string; core: string; asm: string; asmCommons: string; asmTree: string; junit: string } | { error: string } {
  const copies: Array<[string, string]> = [
    [`org.jacoco:org.jacoco.agent:${JACOCO}:jar:runtime`, "agent.jar"],
    [`org.jacoco:org.jacoco.core:${JACOCO}`, "core.jar"],
    [`org.ow2.asm:asm:${ASM}`, "asm.jar"],
    [`org.ow2.asm:asm-commons:${ASM}`, "asm-commons.jar"],
    [`org.ow2.asm:asm-tree:${ASM}`, "asm-tree.jar"],
    ["junit:junit:4.13.2", "junit.jar"],
  ]
  const paths: Record<string, string> = {}
  for (const [artifact, name] of copies) {
    const dest = path.join(work, name)
    if (!existsSync(dest)) {
      const before = new Set(readdirSync(work))
      const copied = run(
        "mvn",
        ["-B", "-q", "org.apache.maven.plugins:maven-dependency-plugin:3.6.1:copy", `-Dartifact=${artifact}`, `-DoutputDirectory=${work}`],
        pkg,
      )
      if (copied.status !== 0) return { error: clip(copied.stderr || copied.stdout || copied.error?.message || `mvn copy ${artifact} failed`) }
      const created = readdirSync(work).find((item) => !before.has(item) && item.endsWith(".jar"))
      if (!created) return { error: `java coverage: missing ${artifact}` }
      writeFileSync(dest, readFileSync(path.join(work, created)))
    }
    paths[name] = dest
  }
  return {
    agent: paths["agent.jar"],
    core: paths["core.jar"],
    asm: paths["asm.jar"],
    asmCommons: paths["asm-commons.jar"],
    asmTree: paths["asm-tree.jar"],
    junit: paths["junit.jar"],
  }
}

/** Launcher jar matching the project's junit-platform-engine, or none when this suite is not Jupiter. */
function platformLauncherJar(pkg: string, work: string, projectCp: string): { jar: string } | { error: string } {
  const entries = projectCp.split(path.delimiter).filter(Boolean)
  if (entries.some((item) => path.basename(item).startsWith("junit-platform-launcher"))) return { jar: "" }
  const engine = entries.find((item) => path.basename(item).startsWith("junit-platform-engine-"))
  if (!engine) return { jar: "" }
  const version = path.basename(engine).replace(/^junit-platform-engine-/, "").replace(/\.jar$/, "")
  if (!version) return { error: "java coverage: junit-platform-engine version missing" }
  const dest = path.join(work, "launcher.jar")
  if (!existsSync(dest)) {
    const artifact = `org.junit.platform:junit-platform-launcher:${version}`
    const before = new Set(readdirSync(work))
    const copied = run(
      "mvn",
      ["-B", "-q", "org.apache.maven.plugins:maven-dependency-plugin:3.6.1:copy", `-Dartifact=${artifact}`, `-DoutputDirectory=${work}`],
      pkg,
    )
    if (copied.status !== 0) return { error: clip(copied.stderr || copied.stdout || copied.error?.message || `mvn copy ${artifact} failed`) }
    const created = readdirSync(work).find((item) => !before.has(item) && item.endsWith(".jar"))
    if (!created) return { error: `java coverage: missing ${artifact}` }
    writeFileSync(dest, readFileSync(path.join(work, created)))
  }
  return { jar: dest }
}

function buildClasspath(pkg: string, work: string): { cp: string } | { error: string } {
  const file = path.join(work, "classpath.txt")
  const built = run("mvn", ["-B", "-q", "dependency:build-classpath", "-DincludeScope=test", `-Dmdep.outputFile=${file}`], pkg)
  if (built.status !== 0) return { error: clip(built.stderr || built.stdout || built.error?.message || "mvn classpath failed") }
  try {
    return { cp: readFileSync(file, "utf8").trim() }
  } catch {
    return { error: "java coverage: classpath file missing" }
  }
}

function javaTool(name: "java" | "javac"): string {
  const home = process.env.JAVA_HOME
  if (home) {
    const candidate = path.join(home, "bin", name)
    if (existsSync(candidate) && toolWorks(candidate)) return candidate
  }
  if (toolWorks(name)) return name
  for (const dir of ["/opt/homebrew/opt/openjdk/bin", "/usr/local/opt/openjdk/bin"]) {
    const candidate = path.join(dir, name)
    if (existsSync(candidate) && toolWorks(candidate)) return candidate
  }
  return name
}

function toolWorks(bin: string): boolean {
  const probed = spawnSync(bin, ["-version"], { encoding: "utf8" })
  const text = `${probed.stdout ?? ""}${probed.stderr ?? ""}`
  return probed.status === 0 && !/Unable to locate a Java Runtime/i.test(text)
}

function run(bin: string, args: string[], cwd: string, timeout = 180_000) {
  return spawnSync(bin, args, { cwd, encoding: "utf8", timeout })
}

function clip(text: string): string {
  const line = text.trim().split("\n").find((item) => item.trim()) ?? "java coverage failed"
  return line.slice(0, 240)
}
