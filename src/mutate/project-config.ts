import { existsSync, readFileSync } from "node:fs"
import path from "node:path"

/**
 * `.probatio.json` at the package root. How to run this project's suite, written by the
 * project and visible in review. Probatio does not patch a project's files or environment
 * to make one repository pass. A project that needs that says so here.
 */
export const CONFIG_FILE = ".probatio.json"

export type ProjectConfig = {
  /** Extra environment for the suite, such as a flag the project's own CI sets. */
  env: Record<string, string>
  /** A shell suite's test lines. The first capture group is the name, or the whole line. */
  testLine: RegExp | null
  /** A shell suite prints this when every test passed, whatever the exit code. */
  passLine: RegExp | null
}

const EMPTY: ProjectConfig = { env: {}, testLine: null, passLine: null }

export function readProjectConfig(pkg: string): ProjectConfig | { error: string } {
  const file = path.join(pkg, CONFIG_FILE)
  if (!existsSync(file)) return EMPTY
  let raw: unknown
  try {
    raw = JSON.parse(readFileSync(file, "utf8"))
  } catch (error) {
    return { error: `${CONFIG_FILE} is not JSON (${error instanceof Error ? error.message : String(error)})` }
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return { error: `${CONFIG_FILE} must be one JSON object` }
  const body = raw as { env?: unknown; testLine?: unknown; passLine?: unknown }
  const env: Record<string, string> = {}
  if (body.env !== undefined) {
    if (!body.env || typeof body.env !== "object" || Array.isArray(body.env)) return { error: `${CONFIG_FILE} env must be an object of strings` }
    for (const [key, value] of Object.entries(body.env)) {
      if (typeof value !== "string") return { error: `${CONFIG_FILE} env.${key} must be a string` }
      env[key] = value
    }
  }
  const testLine = pattern(body.testLine, "testLine")
  if (typeof testLine === "string") return { error: testLine }
  const passLine = pattern(body.passLine, "passLine")
  if (typeof passLine === "string") return { error: passLine }
  return { env, testLine, passLine }
}

/** The config, or the empty one when the file is unreadable. The runner refuses that case earlier. */
export function projectConfig(pkg: string): ProjectConfig {
  const read = readProjectConfig(pkg)
  return "error" in read ? EMPTY : read
}

function pattern(value: unknown, name: string): RegExp | null | string {
  if (value === undefined || value === null) return null
  if (typeof value !== "string" || value.length === 0) return `${CONFIG_FILE} ${name} must be a non-empty string`
  try {
    return new RegExp(value)
  } catch (error) {
    return `${CONFIG_FILE} ${name} is not a regular expression (${error instanceof Error ? error.message : String(error)})`
  }
}
