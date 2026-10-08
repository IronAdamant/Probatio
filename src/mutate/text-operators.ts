import path from "node:path"
import type { FoundMutants, MutantPoint, OperatorSet } from "./operators.js"

type Kind =
  | "c"
  | "rust"
  | "go"
  | "python"
  | "cobol"
  | "js"
  | "csharp"
  | "swift"
  | "asm-x86_64"
  | "asm-aarch64"
  | "asm-riscv"

type Op = { original: string; op: string; replacement: string }

const C_OPS: Op[] = [
  { original: "&&", op: "and-to-or", replacement: "||" },
  { original: "||", op: "or-to-and", replacement: "&&" },
  { original: "<=", op: "le-to-lt", replacement: "<" },
  { original: ">=", op: "ge-to-gt", replacement: ">" },
  { original: "==", op: "eq-to-neq", replacement: "!=" },
  { original: "!=", op: "neq-to-eq", replacement: "==" },
  { original: "<", op: "lt-to-le", replacement: "<=" },
  { original: ">", op: "gt-to-ge", replacement: ">=" },
]

const PY_WORDS: Op[] = [
  { original: "and", op: "and-to-or", replacement: "or" },
  { original: "or", op: "or-to-and", replacement: "and" },
]

const EQ_OPS: Op[] = [
  { original: "===", op: "eq-to-neq", replacement: "!==" },
  { original: "!==", op: "neq-to-eq", replacement: "===" },
]

const X86_OPS: Op[] = [
  { original: "je", op: "eq-to-neq", replacement: "jne" },
  { original: "jne", op: "neq-to-eq", replacement: "je" },
  { original: "jz", op: "eq-to-neq", replacement: "jnz" },
  { original: "jnz", op: "neq-to-eq", replacement: "jz" },
  { original: "jg", op: "gt-to-ge", replacement: "jge" },
  { original: "jge", op: "ge-to-gt", replacement: "jg" },
  { original: "jl", op: "lt-to-le", replacement: "jle" },
  { original: "jle", op: "le-to-lt", replacement: "jl" },
  { original: "ja", op: "gt-to-ge", replacement: "jae" },
  { original: "jae", op: "ge-to-gt", replacement: "ja" },
  { original: "jb", op: "lt-to-le", replacement: "jbe" },
  { original: "jbe", op: "le-to-lt", replacement: "jb" },
  { original: "and", op: "and-to-or", replacement: "or" },
  { original: "or", op: "or-to-and", replacement: "and" },
]

const ARM_OPS: Op[] = [
  { original: "b.eq", op: "eq-to-neq", replacement: "b.ne" },
  { original: "b.ne", op: "neq-to-eq", replacement: "b.eq" },
  { original: "b.gt", op: "gt-to-ge", replacement: "b.ge" },
  { original: "b.ge", op: "ge-to-gt", replacement: "b.gt" },
  { original: "b.lt", op: "lt-to-le", replacement: "b.le" },
  { original: "b.le", op: "le-to-lt", replacement: "b.lt" },
  { original: "b.hi", op: "gt-to-ge", replacement: "b.hs" },
  { original: "b.hs", op: "ge-to-gt", replacement: "b.hi" },
  { original: "b.lo", op: "lt-to-le", replacement: "b.ls" },
  { original: "b.ls", op: "le-to-lt", replacement: "b.lo" },
  { original: "cbz", op: "eq-to-neq", replacement: "cbnz" },
  { original: "cbnz", op: "neq-to-eq", replacement: "cbz" },
]

const RISCV_OPS: Op[] = [
  { original: "beq", op: "eq-to-neq", replacement: "bne" },
  { original: "bne", op: "neq-to-eq", replacement: "beq" },
  { original: "blt", op: "lt-to-le", replacement: "bge" },
  { original: "bge", op: "le-to-lt", replacement: "blt" },
  { original: "bltu", op: "lt-to-le", replacement: "bgeu" },
  { original: "bgeu", op: "le-to-lt", replacement: "bltu" },
]

/** Which text finder owns this path. TypeScript stays null so the TS parser is not used. */
export function textKind(file: string): Kind | null {
  switch (path.extname(file).toLowerCase()) {
    case ".py":
      return "python"
    case ".cob":
    case ".cbl":
    case ".cobol":
      return "cobol"
    case ".rs":
      return "rust"
    case ".go":
      return "go"
    case ".java":
    case ".c":
    case ".h":
    case ".cpp":
    case ".cc":
    case ".cxx":
    case ".hpp":
    case ".hh":
    case ".hxx":
      return "c"
    case ".js":
    case ".mjs":
    case ".cjs":
      return "js"
    case ".cs":
      return "csharp"
    case ".swift":
      return "swift"
    case ".asm":
    case ".nasm":
    case ".yasm":
      return "asm-x86_64"
    case ".s":
    case ".S":
      return "asm-aarch64"
    default:
      return null
  }
}

/**
 * Operator mutants for one non-TypeScript source. Strings and comments are skipped
 * before a point is proposed, so a `/*`, `*>`, or `#` inside a string cannot hide later code.
 */
export function findTextMutants(file: string, text: string, operators: OperatorSet = "core"): FoundMutants {
  const kind = resolveKind(file, text)
  if (!kind) return { points: [], violations: [] }
  const code = codeMask(text, kind)
  const points: MutantPoint[] = []
  let line = 1
  let index = 0
  while (index < text.length) {
    if (text[index] === "\n") {
      line += 1
      index += 1
      continue
    }
    const match = code[index] ? matchAt(text, index, code, kind) ?? (operators === "wide" ? wideAt(text, index, code, kind) : null) : null
    if (!match) {
      index += 1
      continue
    }
    points.push({
      file,
      start: index,
      end: index + match.original.length,
      line,
      op: match.op,
      original: match.original,
      replacement: match.replacement,
    })
    index += match.original.length
  }
  return { points, violations: [] }
}

function matchAt(text: string, index: number, code: Uint8Array, kind: Kind): Op | null {
  const word = matchWord(text, index, code, kind)
  if (word) return word
  for (const op of symbolOps(kind)) {
    if (!text.startsWith(op.original, index)) continue
    if (!spanIsCode(code, index, op.original.length)) continue
    if (blocked(text, index, op.original)) continue
    return op
  }
  return null
}

const ARITHMETIC: Op[] = [
  { original: "+", op: "add-to-sub", replacement: "-" },
  { original: "-", op: "sub-to-add", replacement: "+" },
  { original: "*", op: "mul-to-div", replacement: "/" },
  { original: "/", op: "div-to-mul", replacement: "*" },
  { original: "%", op: "mod-to-mul", replacement: "*" },
]

/**
 * Wide set for text languages: spaced arithmetic and integer constants. No statement deletion here:
 * without a parser, a removed line can be half of a statement. COBOL and assembly are left out,
 * because a COBOL level number or an assembler immediate is not a value the program computes.
 */
function wideAt(text: string, index: number, code: Uint8Array, kind: Kind): Op | null {
  if (kind === "cobol" || kind.startsWith("asm-")) return null
  const prev = text[index - 1] ?? ""
  const next = text[index + 1] ?? ""
  for (const op of ARITHMETIC) {
    if (text[index] !== op.original) continue
    // Spaced on both sides, the way a binary operator is written. That leaves `++`, `+=`, `->`,
    // `//`, `**`, unary minus, and most pointer stars alone.
    if (!(/[ \t]/.test(prev) && /[ \t]/.test(next))) return null
    const before = text.slice(0, index).trimEnd().slice(-1)
    if (before === "" || /[=(,[{:?!<>&|+\-*/%^~]/.test(before)) return null
    return spanIsCode(code, index, 1) ? op : null
  }
  if (!/[0-9]/.test(text[index]) || /[A-Za-z0-9_.$#]/.test(prev)) return null
  const digits = /^[0-9]+/.exec(text.slice(index, index + 20))?.[0] ?? ""
  // A hex, binary, float, exponent, or suffixed literal (`0x10`, `1.5`, `1e5`, `1u32`) is left alone.
  if (!digits || /[A-Za-z0-9_.]/.test(text[index + digits.length] ?? "")) return null
  if (!spanIsCode(code, index, digits.length)) return null
  if (digits === "0") return { original: "0", op: "const-zero-to-one", replacement: "1" }
  if (digits === "1") return { original: "1", op: "const-one-to-zero", replacement: "0" }
  if (digits.length > 15 || digits.startsWith("0")) return null
  return { original: digits, op: "const-inc", replacement: String(Number(digits) + 1) }
}

function resolveKind(file: string, text: string): Kind | null {
  const kind = textKind(file)
  if (!kind) return null
  if (kind.startsWith("asm-")) return sniffAsm(file, text)
  return kind
}

function sniffAsm(file: string, text: string): Kind {
  const ext = path.extname(file)
  const lower = ext.toLowerCase()
  if (lower === ".asm" || lower === ".nasm" || lower === ".yasm") return "asm-x86_64"
  const riscv = /\b(beq|bne|blt|bge|bltu|bgeu|ecall)\b/.test(text)
  const arm = /\bb\.(eq|ne|lt|le|gt|ge|hi|hs|lo|ls)\b/.test(text) || /\b(adrp|svc)\b/.test(text)
  if (riscv && !arm) return "asm-riscv"
  if (arm && !riscv) return "asm-aarch64"
  // `.S` lowercases to `.s`. A capital S stays RISC-V when the body has no marker.
  return lower === ".s" && ext !== ".S" ? "asm-aarch64" : "asm-riscv"
}

function symbolOps(kind: Kind): Op[] {
  if (kind.startsWith("asm-")) return []
  if (kind === "cobol") return C_OPS.filter((op) => op.original === "<" || op.original === ">" || op.original === "<=" || op.original === ">=")
  if (kind === "python") return C_OPS.filter((op) => op.original !== "&&" && op.original !== "||")
  if (kind === "js" || kind === "swift") return [...EQ_OPS, ...C_OPS]
  return C_OPS
}

function matchWord(text: string, index: number, code: Uint8Array, kind: Kind): Op | null {
  if (kind === "python") return matchExactWord(text, index, code, PY_WORDS, "")
  if (kind === "cobol") return matchCobolWord(text, index, code)
  if (kind === "asm-x86_64") return matchMnemonic(text, index, code, X86_OPS, true)
  if (kind === "asm-aarch64") return matchMnemonic(text, index, code, ARM_OPS, false)
  if (kind === "asm-riscv") return matchMnemonic(text, index, code, RISCV_OPS, false)
  return matchExactWord(
    text,
    index,
    code,
    [
      { original: "true", op: "true-to-false", replacement: "false" },
      { original: "false", op: "false-to-true", replacement: "true" },
    ],
    "",
  )
}

function matchMnemonic(text: string, index: number, code: Uint8Array, words: Op[], insensitive: boolean): Op | null {
  const ordered = [...words].sort((left, right) => right.original.length - left.original.length)
  for (const word of ordered) {
    const slice = text.slice(index, index + word.original.length)
    const same = insensitive ? slice.toLowerCase() === word.original : slice === word.original
    if (!same) continue
    if (!wordEdge(text, index, index + word.original.length, ".")) continue
    if (text[index + word.original.length] === ":") continue
    if (!spanIsCode(code, index, word.original.length)) continue
    return { original: slice, op: word.op, replacement: mirrorCase(slice, word.replacement) }
  }
  return null
}

function matchExactWord(text: string, index: number, code: Uint8Array, words: Op[], extra: string): Op | null {
  for (const word of words) {
    if (!text.startsWith(word.original, index)) continue
    if (!wordEdge(text, index, index + word.original.length, extra)) continue
    if (!spanIsCode(code, index, word.original.length)) continue
    return word
  }
  return null
}

function matchCobolWord(text: string, index: number, code: Uint8Array): Op | null {
  const words = ["AND", "OR", "TRUE", "FALSE"]
  for (const word of words) {
    const slice = text.slice(index, index + word.length)
    if (slice.toUpperCase() !== word) continue
    if (!wordEdge(text, index, index + word.length, "-")) continue
    if (!spanIsCode(code, index, word.length)) continue
    if (word === "AND") return { original: slice, op: "and-to-or", replacement: mirrorCase(slice, "OR") }
    if (word === "OR") return { original: slice, op: "or-to-and", replacement: mirrorCase(slice, "AND") }
    if (word === "TRUE") return { original: slice, op: "true-to-false", replacement: mirrorCase(slice, "FALSE") }
    return { original: slice, op: "false-to-true", replacement: mirrorCase(slice, "TRUE") }
  }
  return null
}

function mirrorCase(source: string, replacement: string): string {
  if (source === source.toUpperCase()) return replacement.toUpperCase()
  if (source === source.toLowerCase()) return replacement.toLowerCase()
  return replacement[0].toUpperCase() + replacement.slice(1).toLowerCase()
}

function wordEdge(text: string, start: number, end: number, extra: string): boolean {
  const ident = (ch: string | undefined) => ch !== undefined && (/[A-Za-z0-9_]/.test(ch) || extra.includes(ch))
  return !ident(text[start - 1]) && !ident(text[end])
}

function spanIsCode(code: Uint8Array, start: number, length: number): boolean {
  for (let offset = 0; offset < length; offset += 1) if (!code[start + offset]) return false
  return true
}

function blocked(text: string, index: number, original: string): boolean {
  const prev = text[index - 1] ?? ""
  const next = text[index + original.length] ?? ""
  // `<-` is a Go channel operator. `=>` is a Rust fat arrow. Neither is a comparison.
  if (original === "<" && (next === "<" || next === "=" || next === "-" || prev === "<")) return true
  if (original === ">" && (next === ">" || next === "=" || prev === ">" || prev === "-" || prev === "=")) return true
  // Both sides must be whitespace. One side still matches `#include <stdio.h>` and `vector<int>`.
  if ((original === "<" || original === ">") && !(/\s/.test(prev) && /\s/.test(next))) return true
  // `<<=`, `>>=`, and `<=>` contain `<=` or `>=` but are not relational operators.
  if (original === "<=" && (prev === "<" || next === ">" || next === "=")) return true
  if (original === ">=" && (prev === ">" || next === "=")) return true
  if ((original === "==" || original === "!=" || original === "===" || original === "!==") && next === "=") return true
  if (original === "&&" && (next === "&" || prev === "&")) return true
  if (original === "||" && (next === "|" || prev === "|")) return true
  return false
}

function codeMask(text: string, kind: Kind): Uint8Array {
  const code = new Uint8Array(text.length)
  let index = 0
  while (index < text.length) {
    const hidden = skipHidden(text, index, kind)
    if (hidden > index) {
      index = hidden
      continue
    }
    code[index] = 1
    index += 1
  }
  return code
}

function skipHidden(text: string, index: number, kind: Kind): number {
  if (kind === "python") return skipPython(text, index)
  if (kind === "cobol") return skipCobol(text, index)
  if (kind === "js") return skipJs(text, index)
  if (kind === "swift") return skipSwift(text, index)
  if (kind === "csharp") return skipCSharp(text, index)
  if (kind === "asm-x86_64") return skipAsm(text, index, { hash: false, semi: true, slash: true })
  if (kind === "asm-aarch64") return skipAsm(text, index, { hash: false, semi: true, slash: true })
  if (kind === "asm-riscv") return skipAsm(text, index, { hash: true, semi: false, slash: true })
  if (kind === "go") return skipC(text, index, { nested: false, rust: false, raw: true })
  if (kind === "rust") return skipC(text, index, { nested: true, rust: true, raw: false })
  return skipC(text, index, { nested: false, rust: false, raw: false })
}

function skipJs(text: string, index: number): number {
  const hidden = skipC(text, index, { nested: false, rust: false, raw: false })
  if (hidden > index) return hidden
  if (text[index] === "`") return skipTemplate(text, index)
  return index
}

function skipSwift(text: string, index: number): number {
  if (text.startsWith('"""', index)) return skipQuotes(text, index, '"""', false)
  return skipC(text, index, { nested: false, rust: false, raw: false })
}

function skipCSharp(text: string, index: number): number {
  if (text.startsWith('@"', index) || text.startsWith('$@"', index)) {
    const start = text[index] === "@" ? index + 1 : index + 2
    let cursor = start + 1
    while (cursor < text.length) {
      if (text.startsWith('""', cursor)) {
        cursor += 2
        continue
      }
      if (text[cursor] === '"') return cursor + 1
      cursor += 1
    }
    return text.length
  }
  return skipC(text, index, { nested: false, rust: false, raw: false })
}

function skipTemplate(text: string, index: number): number {
  let cursor = index + 1
  while (cursor < text.length) {
    if (text[cursor] === "\\") {
      cursor += 2
      continue
    }
    if (text[cursor] === "`") return cursor + 1
    cursor += 1
  }
  return text.length
}

function skipAsm(text: string, index: number, mode: { hash: boolean; semi: boolean; slash: boolean }): number {
  if (mode.slash && text.startsWith("//", index)) return eol(text, index)
  if (mode.semi && text[index] === ";") return eol(text, index)
  if (mode.hash && text[index] === "#") return eol(text, index)
  if (text.startsWith("/*", index)) return skipBlock(text, index, false)
  if (text[index] === '"') return skipEscaped(text, index, '"')
  if (text[index] === "'") return skipEscaped(text, index, "'")
  return index
}

function skipPython(text: string, index: number): number {
  if (text[index] === "#") return eol(text, index)
  const quoted = pythonStringAt(text, index)
  if (!quoted) return index
  return skipQuotes(text, quoted.quoteAt, quoted.quote, quoted.raw)
}

function pythonStringAt(text: string, index: number): { quoteAt: number; quote: string; raw: boolean } | null {
  if (text[index] === "'" || text[index] === '"') {
    const quote = text.startsWith(text[index].repeat(3), index) ? text[index].repeat(3) : text[index]
    return { quoteAt: index, quote, raw: false }
  }
  if (index > 0 && /[A-Za-z0-9_]/.test(text[index - 1])) return null
  if (!/[rRuUbBfF]/.test(text[index])) return null
  let cursor = index
  let letters = ""
  while (cursor < text.length && letters.length < 2 && /[rRuUbBfF]/.test(text[cursor])) {
    letters += text[cursor]
    cursor += 1
  }
  if (text[cursor] !== "'" && text[cursor] !== '"') return null
  if (!/^(?:[rRfFbBuU]|[rR][fFbB]|[fFbB][rR])$/.test(letters)) return null
  const quote = text.startsWith(text[cursor].repeat(3), cursor) ? text[cursor].repeat(3) : text[cursor]
  return { quoteAt: cursor, quote, raw: /[rR]/.test(letters) }
}

function skipCobol(text: string, index: number): number {
  // Column 7 is the fixed-format indicator. `*` and `/` comment out the whole line.
  if ((index === 0 || text[index - 1] === "\n") && (text[index + 6] === "*" || text[index + 6] === "/")) return eol(text, index)
  if (text.startsWith("*>", index)) return eol(text, index)
  if (text[index] !== '"' && text[index] !== "'") return index
  const quote = text[index]
  let cursor = index + 1
  while (cursor < text.length) {
    if (text[cursor] === "\n") return cursor
    if (text[cursor] === quote) {
      if (text[cursor + 1] === quote) {
        cursor += 2
        continue
      }
      return cursor + 1
    }
    cursor += 1
  }
  return text.length
}

function skipC(text: string, index: number, mode: { nested: boolean; rust: boolean; raw: boolean }): number {
  if (mode.rust) {
    const raw = rustRawEnd(text, index)
    if (raw !== null) return raw
  }
  if (text.startsWith("//", index)) return eol(text, index)
  if (text.startsWith("/*", index)) return skipBlock(text, index, mode.nested)
  if (text[index] === '"') return skipEscaped(text, index, '"')
  if (mode.raw && text[index] === "`") {
    const end = text.indexOf("`", index + 1)
    return end === -1 ? text.length : end + 1
  }
  if (text[index] !== "'") return index
  if (mode.rust) return rustCharEnd(text, index) ?? index
  return skipEscaped(text, index, "'")
}

function skipBlock(text: string, index: number, nested: boolean): number {
  if (!nested) {
    const end = text.indexOf("*/", index + 2)
    return end === -1 ? text.length : end + 2
  }
  let depth = 1
  let cursor = index + 2
  while (cursor < text.length && depth > 0) {
    if (text.startsWith("/*", cursor)) {
      depth += 1
      cursor += 2
      continue
    }
    if (text.startsWith("*/", cursor)) {
      depth -= 1
      cursor += 2
      continue
    }
    cursor += 1
  }
  return cursor
}

function rustRawEnd(text: string, index: number): number | null {
  if (text[index] !== "r" && !(text[index] === "b" && text[index + 1] === "r")) return null
  if (index > 0 && /[A-Za-z0-9_]/.test(text[index - 1])) return null
  let cursor = text[index] === "b" ? index + 2 : index + 1
  let hashes = 0
  while (text[cursor] === "#") {
    hashes += 1
    cursor += 1
  }
  if (text[cursor] !== '"') return null
  cursor += 1
  const closer = `"${"#".repeat(hashes)}`
  const end = text.indexOf(closer, cursor)
  return end === -1 ? text.length : end + closer.length
}

function rustCharEnd(text: string, index: number): number | null {
  if (text[index + 1] === "\\" && text[index + 2] === "'" && text[index + 3] === "'") return index + 4
  if (text[index + 1] === "\\") {
    let cursor = index + 2
    while (cursor < text.length && cursor < index + 12 && text[cursor] !== "'") cursor += 1
    return text[cursor] === "'" ? cursor + 1 : null
  }
  if (text[index + 2] === "'") return index + 3
  return null
}

function skipEscaped(text: string, index: number, quote: string): number {
  let cursor = index + 1
  while (cursor < text.length) {
    if (text[cursor] === "\\") {
      cursor += 2
      continue
    }
    if (text[cursor] === "\n") return cursor
    if (text[cursor] === quote) return cursor + 1
    cursor += 1
  }
  return text.length
}

function skipQuotes(text: string, quoteAt: number, quote: string, raw: boolean): number {
  let cursor = quoteAt + quote.length
  while (cursor < text.length) {
    if (!raw && text[cursor] === "\\") {
      cursor += 2
      continue
    }
    if (text.startsWith(quote, cursor)) return cursor + quote.length
    cursor += 1
  }
  return text.length
}

function eol(text: string, index: number): number {
  let cursor = index
  while (cursor < text.length && text[cursor] !== "\n") cursor += 1
  return cursor
}
