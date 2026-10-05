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
  if (killedBy.every((item) => BUILD_NAMES.has(bareName(item)))) {
    return { cause: "build", next: "Killed because the build failed. Record it as a compile kill." }
  }
  return { cause: "test", next: `Killed by ${killedBy.join(", ")}.` }
}

function bareName(id: string): string {
  const mark = id.lastIndexOf("::")
  return (mark >= 0 ? id.slice(mark + 2) : id).trim().toLowerCase()
}
