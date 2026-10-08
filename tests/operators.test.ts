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

test("wide operators add arithmetic, constants, and a dropped call, and core stays the same", async () => {
  const { findInSource } = await import("../src/mutate/find.ts")
  const text = [
    "export function total(items: number[], log: (n: number) => void): number {",
    "  let sum = 0",
    "  for (const item of items) sum = sum + item * 2",
    "  log(sum)",
    '  const label = "a" + sum',
    "  return sum - 1",
    "}",
    "",
  ].join("\n")
  const core = findInSource("src/total.ts", text)
  const wide = findInSource("src/total.ts", text, "wide")
  const ops = (found: { points: Array<{ op: string; original: string }> }) => found.points.map((point) => `${point.op}:${point.original}`).sort()
  // Core has no arithmetic, constants, or deletions. Its ids and counts do not move.
  assert.equal(ops(core).some((op) => /^(add|sub|mul|div|mod|const|drop-call)/.test(op)), false, ops(core).join(" "))
  const got = ops(wide)
  assert.ok(got.includes("add-to-sub:+"), got.join(" "))
  assert.ok(got.includes("mul-to-div:*"), got.join(" "))
  assert.ok(got.includes("sub-to-add:-"), got.join(" "))
  assert.ok(got.includes("const-zero-to-one:0"), got.join(" "))
  assert.ok(got.includes("const-one-to-zero:1"), got.join(" "))
  assert.ok(got.includes("const-inc:2"), got.join(" "))
  assert.ok(got.includes("drop-call:log(sum)"), got.join(" "))
  // `"a" + sum` builds a string. Swapping it for `-` is not an arithmetic mutant.
  assert.equal(wide.points.filter((point) => point.op === "add-to-sub").length, 1, got.join(" "))
  assert.equal(wide.violations.length, 0)
})

test("wide text operators need spaced arithmetic, skip strings, and leave COBOL and assembly alone", async () => {
  const { findInSource } = await import("../src/mutate/find.ts")
  const py = 'def area(w, h):\n    note = "w * h + 1"\n    return w * h + 1 if w >= 0 else -w // 2\n'
  const ops = findInSource("src/area.py", py, "wide").points.map((point) => `${point.op}:${point.original}`)
  assert.ok(ops.includes("mul-to-div:*"), ops.join(" "))
  assert.ok(ops.includes("add-to-sub:+"), ops.join(" "))
  assert.ok(ops.includes("const-one-to-zero:1"), ops.join(" "))
  // `//` is floor division, `-w` is unary, and the string is text.
  assert.equal(ops.filter((op) => op.startsWith("mul-to-div")).length, 1, ops.join(" "))
  assert.equal(ops.some((op) => op.startsWith("div-to-mul") || op.startsWith("sub-to-add")), false, ops.join(" "))
  const c = "int size(int n) {\n  int *p = 0;\n  n++;\n  n += 2;\n  return n * 3 - 0x10 + 1.5f;\n}\n"
  const cOps = findInSource("src/size.c", c, "wide").points.map((point) => `${point.op}:${point.original}`)
  assert.ok(cOps.includes("mul-to-div:*"), cOps.join(" "))
  assert.ok(cOps.includes("sub-to-add:-"), cOps.join(" "))
  // A pointer star, `++`, `+=`, a hex literal, and a float are not these operators.
  assert.equal(cOps.filter((op) => op.startsWith("mul-to-div")).length, 1, cOps.join(" "))
  assert.equal(cOps.some((op) => op === "const-inc:0x10" || op.includes("1.5")), false, cOps.join(" "))
  const cobol = "       IDENTIFICATION DIVISION.\n       01 WS-COUNT PIC 9(2) VALUE 0.\n           COMPUTE WS-COUNT = WS-COUNT + 1.\n"
  assert.equal(findInSource("src/count.cbl", cobol, "wide").points.some((point) => /^(add|sub|mul|div|mod|const)/.test(point.op)), false)
})

test("mutate generate takes --operators wide, records it, and refuses an unknown set", async () => {
  const { spawnSync } = await import("node:child_process")
  const { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = await import("node:fs")
  const { tmpdir } = await import("node:os")
  const path = (await import("node:path")).default
  const { fileURLToPath } = await import("node:url")
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
  const tsx = path.join(root, "node_modules", ".bin", "tsx")
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-wide-"))
  try {
    mkdirSync(path.join(dir, "src"))
    writeFileSync(path.join(dir, "src", "price.ts"), "export function price(n: number): number {\n  if (n > 10) return n * 2\n  return n + 1\n}\n")
    for (const args of [["init", "-q"], ["add", "."], ["-c", "user.email=p@example.com", "-c", "user.name=p", "commit", "-qm", "i"]]) spawnSync("git", ["-C", dir, ...args])
    const run = (set: string) =>
      JSON.parse(spawnSync(tsx, ["src/cli.ts", "mutate", "generate", "--package", dir, "--out", path.join(dir, set), "--operators", set], { cwd: root, encoding: "utf8" }).stdout) as {
        ok: boolean
        summary: string
        mutantCount: number
        operators?: string
      }
    const core = run("core")
    const wide = run("wide")
    assert.equal(core.ok, true, core.summary)
    assert.equal(wide.ok, true, wide.summary)
    assert.equal(wide.operators, "wide")
    assert.ok(wide.mutantCount > core.mutantCount, `${core.mutantCount} core, ${wide.mutantCount} wide`)
    const recorded = JSON.parse(readFileSync(path.join(dir, "wide", "mutants", "mutants.json"), "utf8")) as { operators: string }
    assert.equal(recorded.operators, "wide")
    const bad = run("everything")
    assert.equal(bad.ok, false)
    assert.match(bad.summary, /--operators must be core or wide/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
