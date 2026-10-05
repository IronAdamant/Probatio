import { findMutants, type FoundMutants } from "./operators.js"
import { findTextMutants, textKind } from "./text-operators.js"

/** TypeScript stays on the TypeScript parser. Every other recognized source uses its own finder. */
export function findInSource(file: string, text: string): FoundMutants {
  if (textKind(file)) return findTextMutants(file, text)
  return findMutants(file, text)
}

export function isGeneratedSource(name: string): boolean {
  if (textKind(name)) return true
  return (name.endsWith(".ts") || name.endsWith(".tsx")) && !name.endsWith(".d.ts")
}
