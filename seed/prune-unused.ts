// Remove what dropped tests left behind: unused imports and unused top-level helpers in test files.
//   npx tsx ../../experiments/slim-tests/prune-unused.ts   (from examples/auspex-ts)
// Asks the compiler (noUnusedLocals) which declarations are unused, deletes them, and repeats until
// nothing is flagged, since removing one helper can leave another unused.
import { readFileSync, readdirSync, writeFileSync } from "node:fs"
import { createRequire } from "node:module"
import path from "node:path"

const ts = createRequire(path.join(process.cwd(), "package.json"))("typescript") as typeof import("typescript")
const testsDir = path.join(process.cwd(), "tests")
const files = readdirSync(testsDir).filter((f) => f.endsWith(".ts")).map((f) => path.join(testsDir, f))

for (let round = 1; round <= 10; round++) {
  const program = ts.createProgram(files, {
    noUnusedLocals: true,
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    allowImportingTsExtensions: true,
    noEmit: true,
    strict: true,
    skipLibCheck: true,
    types: ["node"],
  })
  const edits = new Map<string, Array<{ start: number; end: number }>>()
  for (const sf of program.getSourceFiles().filter((s) => files.includes(path.resolve(s.fileName)))) {
    const diags = program.getSemanticDiagnostics(sf).filter((d) => [6133, 6192, 6196, 6198].includes(d.code))
    for (const d of diags) {
      let node: ts.Node | undefined = findNode(sf, d.start!)
      // Climb to what can be removed: an import specifier, a whole import, or a top-level statement.
      while (node && !ts.isImportSpecifier(node) && !ts.isImportDeclaration(node) && !(node.parent && ts.isSourceFile(node.parent))) node = node.parent
      if (!node) continue
      if (ts.isImportSpecifier(node)) {
        const named = node.parent
        const range = named.elements.length === 1 ? named.parent.parent : node // last specifier: the whole import
        add(edits, sf.fileName, removalRange(sf, range))
      } else add(edits, sf.fileName, removalRange(sf, node))
    }
  }
  if (!edits.size) {
    console.log(`clean after ${round - 1} round(s)`)
    break
  }
  for (const [file, ranges] of edits) {
    let text = readFileSync(file, "utf8")
    const merged = [...new Map(ranges.map((r) => [`${r.start}:${r.end}`, r])).values()].sort((a, b) => b.start - a.start)
    let lastStart = Infinity
    for (const r of merged) {
      if (r.end > lastStart) continue // overlapping (a specifier inside a removed import)
      text = text.slice(0, r.start) + text.slice(r.end)
      lastStart = r.start
    }
    writeFileSync(file, text.replace(/\n{3,}/g, "\n\n"))
  }
  console.log(`round ${round}: ${[...edits.values()].reduce((a, r) => a + r.length, 0)} removals in ${edits.size} files`)
}

function findNode(sf: ts.SourceFile, pos: number): ts.Node | undefined {
  let found: ts.Node | undefined
  const visit = (n: ts.Node) => {
    if (pos >= n.getStart(sf) && pos < n.getEnd()) {
      found = n
      ts.forEachChild(n, visit)
    }
  }
  visit(sf)
  return found
}
function removalRange(sf: ts.SourceFile, node: ts.Node): { start: number; end: number } {
  if (ts.isImportSpecifier(node)) {
    // "a, b, c": take the specifier and the comma after it (or before it, for the last one).
    const text = sf.text
    let start = node.getStart(sf)
    let end = node.getEnd()
    const after = text.slice(end).match(/^\s*,\s*/)
    if (after) end += after[0].length
    else {
      const before = text.slice(0, start).match(/,\s*$/)
      if (before) start -= before[0].length
    }
    return { start, end }
  }
  // A whole statement with the comment lines directly above it, from the start of its first line
  // through the line break after it.
  const text = sf.text
  const comments = ts.getLeadingCommentRanges(text, node.getFullStart()) ?? []
  let start = comments.length ? comments[0].pos : node.getStart(sf)
  while (start > 0 && text[start - 1] !== "\n") start--
  let end = node.getEnd()
  if (text[end] === "\n") end++
  return { start, end }
}
function add(map: Map<string, Array<{ start: number; end: number }>>, file: string, r: { start: number; end: number }) {
  if (!map.has(file)) map.set(file, [])
  map.get(file)!.push(r)
}
