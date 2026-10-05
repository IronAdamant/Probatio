/** Pure suite decisions. The process spawn stays in the runner. */

export const UNKNOWN_NEXT =
  "The suite command is unknown. Pass --suite-command with the command that runs this package's tests."

export const TIMEOUT_NEXT = "Timed out. The mutant ran longer than the baseline allows."

export const WHOLE_SUITE_NEXT = "The suite command cannot take a test name, so the whole suite ran."

export const TIMEOUT_MULTIPLE = 5

/** cargo-mutants uses 20s. A fixture can pass a smaller positive floor. */
export const TIMEOUT_FLOOR_MS = 20_000

/** Marker on a coverage map when the binary has no per-test names. */
export const WHOLE_PROGRAM_TEST = "suite"

export function cannotRunNext(artifact: string): string {
  return `The suite cannot run. ${artifact} is missing and the recipe would download it. Nothing was fetched.`
}

export function resumeNext(ids: string[]): string {
  const listed = ids.length > 0 ? ` Not started: ${ids.join(", ")}.` : ""
  return `Budget stopped the run.${listed} The same command resumes from the mutants already written.`
}

export function noCoverageNext(file: string, line: number): string {
  return `No coverage for ${file}:${line}. No test executed that line.`
}

export function timeoutLimitMs(baselineMs: number, multiple: number, floorMs: number): number {
  const floor = Math.max(1, floorMs)
  const scaled = Math.ceil(Math.max(0, baselineMs) * Math.max(1, multiple))
  return Math.max(floor, scaled)
}

export type SuiteChoice = {
  kind: string
  command: string
}

export type SuiteDecision =
  | { action: "run" }
  | { action: "unknown"; summary: string; next: string }
  | { action: "cannot-run"; summary: string; next: string; command: string }
  | { action: "no-tests" }

/**
 * Unknown layout asks for a command. An explicit command is what runs.
 * `make test` that would curl or wget a missing file does not run.
 */
export function decideSuite(input: {
  discovered: SuiteChoice | null
  suiteCommand: string | null
  candidates: string[]
  makefile: string | null
  artifactExists: (rel: string) => boolean
}): SuiteDecision {
  const command = input.suiteCommand?.trim() ? input.suiteCommand.trim() : input.discovered?.command ?? ""
  if (isMakeTest(command) && input.makefile) {
    const artifact = missingFetchArtifact(input.makefile, input.artifactExists)
    if (artifact) {
      const next = cannotRunNext(artifact)
      return { action: "cannot-run", summary: next, next, command: "make test" }
    }
  }
  if (input.suiteCommand?.trim()) return { action: "run" }
  if (input.discovered) return { action: "run" }
  if (input.candidates.length > 0) {
    return { action: "unknown", summary: "The suite command is unknown.", next: UNKNOWN_NEXT }
  }
  return { action: "no-tests" }
}

export function isMakeTest(command: string): boolean {
  return /^\s*make\s+test\b/.test(command)
}

/**
 * Output paths of curl or wget recipes whose file is not on disk.
 * Returns the base name, which is the artifact named in `next`.
 * A Makefile that fetches for `test` is refused even when variable
 * expansion cannot prove the edge, so a recipe cannot download a jar.
 */
export function missingFetchArtifact(makefile: string, exists: (rel: string) => boolean): string | null {
  const lines = makefile.split(/\r?\n/)
  for (const raw of lines) {
    if (/^\s*#/.test(raw)) continue
    const line = raw.trim()
    if (!/\b(curl|wget)\b/.test(line)) continue
    const artifact = artifactOf(line)
    if (!artifact) continue
    const rel = artifact.replace(/^\.\//, "")
    if (exists(rel) || exists(pathBase(rel))) continue
    return pathBase(rel)
  }
  return null
}

function artifactOf(line: string): string | null {
  const flagged = line.match(/(?:-o|--output|-O|--output-document)(?:=|\s+)["']?([^\s"']+)["']?/)
  if (flagged && !flagged[1].startsWith("$") && !flagged[1].startsWith("-")) return flagged[1]
  const url = line.match(/https?:\/\/\S+/)
  if (url) {
    const base = url[0].replace(/["']$/, "").split("?")[0].split("/").pop() ?? ""
    if (base.includes(".")) return base
  }
  if (flagged && !flagged[1].startsWith("-")) return flagged[1]
  return null
}

function pathBase(rel: string): string {
  const parts = rel.split(/[/\\]/)
  return parts[parts.length - 1] || rel
}
