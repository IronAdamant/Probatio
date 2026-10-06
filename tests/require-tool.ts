import { spawnSync } from "node:child_process"

/** `false` when the binary is on PATH. A string is a node:test skip reason. */
export function missingTool(bin: string): string | false {
  const found = spawnSync("which", [bin], { encoding: "utf8" })
  if (found.status === 0 && found.stdout.trim().length > 0) return false
  return `missing tool: ${bin}`
}

export function missingPytest(): string | false {
  if (missingTool("python3")) return "missing tool: python3"
  const found = spawnSync("python3", ["-c", "import pytest"], { encoding: "utf8" })
  if (found.status === 0) return false
  return "missing tool: pytest"
}

/** True when any one of the binaries exists. */
export function missingAny(bins: string[]): string | false {
  if (bins.length === 0) return false
  if (bins.some((bin) => missingTool(bin) === false)) return false
  return `missing tool: ${bins[0]}`
}

export function darwinOnly(why: string): string | false {
  if (process.platform === "darwin") return false
  return `darwin only: ${why}`
}

/** Mach-O fixtures need Rosetta or a native arch. A missing arch is a named skip. */
export function missingDarwinArch(arch: "x86_64" | "arm64", why: string): string | false {
  const platform = darwinOnly(why)
  if (platform) return platform
  const probe = spawnSync("arch", [`-${arch}`, "/usr/bin/true"], { encoding: "utf8" })
  if (probe.status === 0) return false
  return `missing tool: arch -${arch}`
}

export function firstSkip(...reasons: Array<string | false>): string | false {
  for (const reason of reasons) if (reason) return reason
  return false
}
