// Cache-busts package files so the next mutant in this process loads the patched source.
// The generation lives in a file because the loader thread does not see later env edits.
import { createRequire } from "node:module"
import { readFileSync, realpathSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const require = createRequire(import.meta.url)
const ts = require("typescript")

function generation() {
  const file = process.env.PB_GEN
  if (!file) return ""
  try {
    return readFileSync(file, "utf8").trim()
  } catch {
    return ""
  }
}

function packageRoot() {
  const root = process.env.PB_ROOT
  if (!root) return ""
  try {
    return realpathSync(root)
  } catch {
    return root
  }
}

function underPackage(abs) {
  const root = packageRoot()
  if (!root) return false
  let real = abs
  try {
    real = realpathSync(abs)
  } catch {
    return false
  }
  return real === root || real.startsWith(root.endsWith(path.sep) ? root : root + path.sep)
}

export async function resolve(specifier, context, nextResolve) {
  const resolved = await nextResolve(specifier, context)
  const gen = generation()
  if (!gen) return resolved
  let abs
  try {
    abs = fileURLToPath(resolved.url.split("?")[0])
  } catch {
    return resolved
  }
  if (!underPackage(abs)) return resolved
  if (abs.split(path.sep).includes("node_modules")) return resolved
  const url = new URL(resolved.url.split("?")[0])
  url.searchParams.set("pb", gen)
  return { url: url.href, shortCircuit: true }
}

export async function load(url, context, nextLoad) {
  if (!url.includes("pb=")) return nextLoad(url, context)
  const clean = url.split("?")[0]
  let abs
  try {
    abs = fileURLToPath(clean)
  } catch {
    return nextLoad(url, context)
  }
  const source = readFileSync(abs, "utf8")
  if (abs.endsWith(".ts") || abs.endsWith(".tsx") || abs.endsWith(".mts")) {
    const transpiled = ts.transpileModule(source, {
      fileName: abs,
      compilerOptions: {
        module: ts.ModuleKind.ESNext,
        target: ts.ScriptTarget.ES2022,
        sourceMap: false,
      },
    })
    return { format: "module", source: transpiled.outputText, shortCircuit: true }
  }
  if (abs.endsWith(".mjs") || abs.endsWith(".js") || source.includes("import ") || source.includes("export ")) {
    return { format: "module", source, shortCircuit: true }
  }
  return { format: "commonjs", source, shortCircuit: true }
}
