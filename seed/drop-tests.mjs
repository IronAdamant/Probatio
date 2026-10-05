// Usage: node drop-tests.mjs <testsDir> <list.tsv>
// list.tsv rows: "<file>\t<exact test title>". Removes each top-level test block
// together with the comment lines directly above it. Prints anything not found.
import { readFileSync, writeFileSync } from "node:fs"
import path from "node:path"

const [dir, listPath] = process.argv.slice(2)
const byFile = new Map()
for (const row of readFileSync(listPath, "utf8").split("\n")) {
  if (!row.trim() || row.startsWith("#")) continue
  const [file, title] = row.split("\t")
  if (!byFile.has(file)) byFile.set(file, new Set())
  byFile.get(file).add(title)
}

function leadStart(lines, i) {
  let s = i
  while (s > 0 && /^\s*\/\//.test(lines[s - 1])) s--
  return s
}

let missing = 0
for (const [file, titles] of byFile) {
  const p = path.join(dir, file)
  const lines = readFileSync(p, "utf8").split("\n")
  const starts = []
  lines.forEach((l, i) => { if (/^(test|it)\(/.test(l)) starts.push(i) })
  const drop = new Set()
  const found = new Set()
  starts.forEach((s, k) => {
    const title = (lines[s].match(/^(?:test|it)\((["'`])(.*?)\1/) || [])[2]
    if (!titles.has(title)) return
    found.add(title)
    const from = leadStart(lines, s)
    // a test ends at its own closing "})" in column 0; helpers after it stay
    let to = s + 1
    while (to < lines.length && !/^\}\)/.test(lines[to])) to++
    to = Math.min(to + 1, lines.length)
    for (let i = from; i < to; i++) drop.add(i)
  })
  for (const t of titles) if (!found.has(t)) { console.log(`NOT FOUND ${file}: ${t}`); missing++ }
  const kept = lines.filter((_, i) => !drop.has(i)).join("\n").replace(/\n{3,}/g, "\n\n")
  writeFileSync(p, kept.endsWith("\n") ? kept : kept + "\n")
  const remaining = kept.split("\n").filter((l) => /^(test|it)\(/.test(l)).length
  console.log(`${file}: dropped ${found.size}, ${remaining} left`)
}
process.exitCode = missing ? 1 : 0
