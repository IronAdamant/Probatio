/** `--experimental-strip-types` needs this. Older Node is refused before a command runs. */
export const MIN_NODE = "22.6"

export function nodeTooOld(version: string): string | null {
  const match = /^(\d+)\.(\d+)/.exec(version)
  const required = `Node ${version} is too old. Probatio needs Node ${MIN_NODE} or newer.`
  if (!match) return required
  const major = Number(match[1])
  const minor = Number(match[2])
  if (major > 22 || (major === 22 && minor >= 6)) return null
  return required
}
