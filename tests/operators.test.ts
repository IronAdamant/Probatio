import assert from "node:assert/strict"
import test from "node:test"
import { findMutants } from "../src/mutate/operators.ts"

test("operators stay out of strings, comments, and type positions", () => {
  const text = `
const label = "if (true && false === 'x') <="
const tick = \`ready === true && n > 0\`
// if (true && false)
/* while (false) { drop !flag } */
type Flag = true
interface Box { ok: false }
const typed: true = true
export function allow(n: number): boolean {
  if (n > 0 && n < 10) return true
  return false
}
`
  const { points, violations } = findMutants("src/gate.ts", text)
  assert.equal(violations.length, 0)
  const counts = new Map<string, number>()
  for (const point of points) counts.set(point.op, (counts.get(point.op) ?? 0) + 1)
  assert.equal(counts.get("and-to-or"), 1)
  assert.equal(counts.get("gt-to-ge"), 1)
  assert.equal(counts.get("lt-to-le"), 1)
  assert.equal(counts.get("negate-condition"), 1)
  assert.equal(counts.get("false-to-true"), 1)
  assert.equal(counts.get("true-to-false"), 2)
  assert.equal(points.length, 7)
  for (const point of points) assert.equal(point.original.includes("SENTINEL"), false)
  assert.ok(points.every((point) => point.line >= 8))
})

test("a condition that contains a string is still code", () => {
  const text = `export function named(name: string): boolean {\n  if (name === "x") return true\n  return false\n}\n`
  const { points, violations } = findMutants("src/named.ts", text)
  assert.equal(violations.length, 0)
  assert.ok(points.some((point) => point.op === "eq-to-neq" && point.original === "==="))
  assert.ok(points.some((point) => point.op === "negate-condition"))
})

test("template expressions are code and template text is not", () => {
  const text = "export const msg = `hello ${n > 0}`\n"
  const { points } = findMutants("src/msg.ts", text)
  assert.deepEqual(points.map((point) => point.op), ["gt-to-ge"])
  assert.equal(points[0].original, ">")
})

test("a block comment lookalike inside a regex does not hide later code", () => {
  const text = `
export function trim(spec: string, imports: string[]): boolean {
  const baseUrl = spec.replace(/\\/*$/, "")
  if (!imports.length) return false
  return spec.startsWith("./") || spec.startsWith("../")
}
`
  const { points, violations } = findMutants("src/trim.ts", text)
  assert.equal(violations.length, 0)
  assert.ok(points.some((point) => point.op === "negate-condition"))
  assert.ok(points.some((point) => point.op === "or-to-and" && point.original === "||"))
})

test("a slash comment inside a template does not hide the interpolation", () => {
  const text = `
mono_log_error(\`\${methodFullName || traceName}\${desc} failed: \${exc} \${exc.stack}\`);
return 0;
} finally {
  if (threw || (!rejected && ((trace >= 2) || mostRecentOptions!.dumpTraces)) || instrument) {
    mono_log_info(\`// \${methodFullName || traceName} generated, blob follows //\`);
  }
}
`
  const { points, violations } = findMutants("src/msg.ts", text)
  assert.equal(violations.length, 0)
  assert.ok(points.some((point) => point.op === "or-to-and" && point.original === "||" && text.split("\n")[point.line - 1].includes("generated, blob")))
})
