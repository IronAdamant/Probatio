import { createHash } from "node:crypto"

/** Stable across reruns: hash of the package-relative file, the span, and the operator. */
export function mutantId(file: string, start: number, end: number, op: string): string {
  const hex = createHash("sha256").update(`${file}\0${start}\0${end}\0${op}`).digest("hex").slice(0, 12)
  return `m${hex}`
}

/** Same sequence for the same seed. Not the Auspex experiment's floating-point shuffle. */
export function makeRng(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (Math.imul(state, 1103515245) + 12345) >>> 0
    return state / 4294967296
  }
}

export function shuffle<T>(items: T[], rng: () => number): T[] {
  const out = [...items]
  for (let index = out.length - 1; index > 0; index--) {
    const swap = Math.floor(rng() * (index + 1))
    ;[out[index], out[swap]] = [out[swap], out[index]]
  }
  return out
}
