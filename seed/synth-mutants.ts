// Synthetic mutants: small operator changes in src/*.ts code (never in strings or comments).
//
//   npx tsx ../../experiments/slim-tests/synth-mutants.ts <outDir> [perFileCap] [seed]   (from examples/auspex-ts)
//
// Operators: negate an if/conditional/while condition, && <-> ||, === <-> !==, == <-> !=,
// < <-> <=, > <-> >=, true <-> false, drop a logical !. Writes one patch per mutant (reverse form,
// like the historical ones: `git apply -R` puts the mutant in) plus index.tsv.
import { spawnSync } from "node:child_process"
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs"
import path from "node:path"
import { createRequire } from "node:module"

// TypeScript comes from the package being mutated (run from examples/auspex-ts).
const ts = createRequire(path.join(process.cwd(), "package.json"))("typescript") as typeof import("typescript")

const [outDir, capRaw, seedRaw, skipRaw, prefixRaw] = process.argv.slice(2)
if (!outDir) throw new Error("usage: synth-mutants.ts <outDir> [perFileCap] [seed] [skipPerFile] [idPrefix]")
const cap = Number(capRaw ?? 12)
// Same seed, skip N: the next points of the same shuffle, so a second batch never repeats the first.
const skip = Number(skipRaw ?? 0)
const prefix = prefixRaw ?? "s"
let seed = Number(seedRaw ?? 20261003)
const rand = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648)

// Prose and tool descriptions: mutating them tests wording, not behaviour.
const SKIP = new Set(["tool-copy.ts", "door-await-contract.ts", "contract.ts"])
type Point = { file: string; start: number; end: number; replacement: string; op: string; line: number }

function pointsFor(file: string, text: string): Point[] {
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS)
  const out: Point[] = []
  const add = (node: ts.Node, replacement: string, op: string) => {
    const start = node.getStart(sf)
    out.push({ file, start, end: node.getEnd(), replacement, op, line: sf.getLineAndCharacterOfPosition(start).line + 1 })
  }
  const swapOp = (n: ts.BinaryExpression, to: string, op: string) => {
    const t = n.operatorToken
    out.push({ file, start: t.getStart(sf), end: t.getEnd(), replacement: to, op, line: sf.getLineAndCharacterOfPosition(t.getStart(sf)).line + 1 })
  }
  const visit = (node: ts.Node) => {
    if (ts.isIfStatement(node) || ts.isWhileStatement(node)) add(node.expression, `!(${node.expression.getText(sf)})`, "negate-condition")
    if (ts.isConditionalExpression(node)) add(node.condition, `!(${node.condition.getText(sf)})`, "negate-ternary")
    if (ts.isBinaryExpression(node)) {
      const k = node.operatorToken.kind
      const K = ts.SyntaxKind
      if (k === K.AmpersandAmpersandToken) swapOp(node, "||", "and-to-or")
      if (k === K.BarBarToken) swapOp(node, "&&", "or-to-and")
      if (k === K.EqualsEqualsEqualsToken) swapOp(node, "!==", "eq-to-neq")
      if (k === K.ExclamationEqualsEqualsToken) swapOp(node, "===", "neq-to-eq")
      if (k === K.LessThanToken) swapOp(node, "<=", "lt-to-le")
      if (k === K.LessThanEqualsToken) swapOp(node, "<", "le-to-lt")
      if (k === K.GreaterThanToken) swapOp(node, ">=", "gt-to-ge")
      if (k === K.GreaterThanEqualsToken) swapOp(node, ">", "ge-to-gt")
    }
    if (node.kind === ts.SyntaxKind.TrueKeyword) add(node, "false", "true-to-false")
    if (node.kind === ts.SyntaxKind.FalseKeyword) add(node, "true", "false-to-true")
    if (ts.isPrefixUnaryExpression(node) && node.operator === ts.SyntaxKind.ExclamationToken && !ts.isPrefixUnaryExpression(node.operand))
      add(node, node.operand.getText(sf), "drop-not")
    // Types are not code: no mutants inside type annotations or declarations.
    if (ts.isTypeNode(node) || ts.isTypeAliasDeclaration(node) || ts.isInterfaceDeclaration(node)) return
    ts.forEachChild(node, visit)
  }
  visit(sf)
  return out
}

const srcDir = path.join(process.cwd(), "src")
const chosen: Point[] = []
for (const file of readdirSync(srcDir).filter((f) => f.endsWith(".ts") && !SKIP.has(f)).sort()) {
  const pts = pointsFor(file, readFileSync(path.join(srcDir, file), "utf8"))
  // Fixed-seed shuffle, then take up to `cap` per file so the large files do not dominate.
  for (let i = pts.length - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1))
    ;[pts[i], pts[j]] = [pts[j], pts[i]]
  }
  chosen.push(...pts.slice(skip, skip + cap))
}

mkdirSync(outDir, { recursive: true })
const index: string[] = ["id\tfile\tline\top"]
chosen.forEach((p, n) => {
  const id = `${prefix}${String(n + 1).padStart(3, "0")}`
  const text = readFileSync(path.join(srcDir, p.file), "utf8")
  const mutated = text.slice(0, p.start) + p.replacement + text.slice(p.end)
  // Reverse form (mutated -> original), written by diff -u so hunk headers are always right.
  const tmp = path.join(outDir, ".mutated.ts")
  writeFileSync(tmp, mutated)
  const rel = `examples/auspex-ts/src/${p.file}`
  const diff = spawnSync("diff", ["-u", "--label", `a/${rel}`, "--label", `b/${rel}`, tmp, path.join(srcDir, p.file)], { encoding: "utf8" }).stdout
  writeFileSync(path.join(outDir, `${id}.patch`), diff)
  index.push(`${id}\t${p.file}\t${p.line}\t${p.op}`)
})
rmSync(path.join(outDir, ".mutated.ts"), { force: true })
writeFileSync(path.join(outDir, "index.tsv"), `${index.join("\n")}\n`)
console.log(`${chosen.length} mutants from ${new Set(chosen.map((p) => p.file)).size} files`)
