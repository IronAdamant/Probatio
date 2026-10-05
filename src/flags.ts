export type FlagValue = string | boolean

export type ParsedArgs = {
  command: string[]
  flags: Map<string, FlagValue[]>
  human: boolean
}

/** `--key value`, `--key=value`, `--flag`, `--no-flag`. Words before the first flag are the command. */
export function parseArgs(argv: string[]): ParsedArgs {
  const command: string[] = []
  const flags = new Map<string, FlagValue[]>()
  let seenFlag = false
  for (let i = 0; i < argv.length; i++) {
    const token = argv[i]
    if (token === "--") {
      seenFlag = true
      continue
    }
    if (!token.startsWith("--")) {
      if (!seenFlag) command.push(token)
      else push(flags, "_", token)
      continue
    }
    seenFlag = true
    const body = token.slice(2)
    const eq = body.indexOf("=")
    if (eq !== -1) {
      push(flags, body.slice(0, eq), body.slice(eq + 1))
      continue
    }
    if (body.startsWith("no-") && body.length > 3) {
      push(flags, body.slice(3), false)
      continue
    }
    const next = argv[i + 1]
    if (next === undefined || next.startsWith("--")) push(flags, body, true)
    else {
      push(flags, body, next)
      i++
    }
  }
  const human = flags.get("human")?.some((value) => value === true) ?? false
  return { command, flags, human }
}

function push(flags: Map<string, FlagValue[]>, key: string, value: FlagValue) {
  const list = flags.get(key)
  if (list) list.push(value)
  else flags.set(key, [value])
}

export function text(flags: Map<string, FlagValue[]>, key: string): string | undefined {
  const values = flags.get(key)
  if (!values || values.length === 0) return undefined
  const last = values[values.length - 1]
  if (typeof last === "boolean") return undefined
  return last
}

export function texts(flags: Map<string, FlagValue[]>, key: string): string[] {
  return (flags.get(key) ?? []).filter((value): value is string => typeof value === "string")
}

export function bool(flags: Map<string, FlagValue[]>, key: string, fallback: boolean): boolean {
  const values = flags.get(key)
  if (!values || values.length === 0) return fallback
  const last = values[values.length - 1]
  if (typeof last === "boolean") return last
  if (last === "true") return true
  if (last === "false") return false
  return fallback
}

export function int(flags: Map<string, FlagValue[]>, key: string): number | undefined {
  const raw = text(flags, key)
  if (raw === undefined) return undefined
  const value = Number(raw)
  if (!Number.isInteger(value) || value < 0) throw new Error(`--${key} must be a non-negative integer`)
  return value
}

export function requireText(flags: Map<string, FlagValue[]>, key: string): string {
  const value = text(flags, key)
  if (!value) throw new Error(`--${key} is required`)
  return value
}
