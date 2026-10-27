export type KillCause = "test" | "build"

export type KillLabel = {
  cause: KillCause
  next: string
}

/** Names that mean the process failed to compile. These are not test titles. */
const BUILD_NAMES = new Set([
  "build",
  "javac",
  "compiler",
  "cobc",
  "nasm",
  "clang",
  "cargo",
  "tsc",
  "dotnet",
  "swiftc",
])

/**
 * A named test and a compile failure are different facts.
 * The build next step says the build failed and does not ask for a test
 * named after the compiler.
 */
export function killLabel(killedBy: string[]): KillLabel | null {
  if (killedBy.length === 0) return null
  if (killedBy.every((item) => isBuildName(item))) {
    return { cause: "build", next: "Killed because the build failed. Record it as a compile kill." }
  }
  return { cause: "test", next: `Killed by ${killedBy.join(", ")}.` }
}

/**
 * True when the id is a compiler token and nothing else.
 * `tests/gate.test.ts::build` is a test that happens to be titled `build`.
 */
export function isBuildName(id: string): boolean {
  const trimmed = id.trim()
  if (trimmed.includes("::") || trimmed.includes("#") || trimmed.includes("/") || trimmed.includes("\\")) return false
  return BUILD_NAMES.has(trimmed.toLowerCase())
}
