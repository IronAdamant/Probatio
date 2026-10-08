import ts from "typescript"

export type MutantPoint = {
  /** Package-relative path, posix slashes. */
  file: string
  start: number
  end: number
  line: number
  op: string
  original: string
  replacement: string
}

/**
 * `core`: conditions, `&&`/`||`, equality, boundaries, booleans, a dropped `!`. Ids and counts are stable.
 * `wide`: core plus arithmetic swaps, numeric constants, and a dropped call statement.
 * More mutants per line, so a survivor or a clean batch says more, and a run costs more.
 */
export type OperatorSet = "core" | "wide"

export type FoundMutants = {
  points: MutantPoint[]
  /** Points whose whole span sits inside a string or a comment. These are never written. */
  violations: MutantPoint[]
}

const ARITHMETIC: Record<number, { op: string; replacement: string } | undefined> = {
  [ts.SyntaxKind.PlusToken]: { op: "add-to-sub", replacement: "-" },
  [ts.SyntaxKind.MinusToken]: { op: "sub-to-add", replacement: "+" },
  [ts.SyntaxKind.AsteriskToken]: { op: "mul-to-div", replacement: "/" },
  [ts.SyntaxKind.SlashToken]: { op: "div-to-mul", replacement: "*" },
  [ts.SyntaxKind.PercentToken]: { op: "mod-to-mul", replacement: "*" },
}

const SWAP: Record<number, { op: string; replacement: string } | undefined> = {
  [ts.SyntaxKind.AmpersandAmpersandToken]: { op: "and-to-or", replacement: "||" },
  [ts.SyntaxKind.BarBarToken]: { op: "or-to-and", replacement: "&&" },
  [ts.SyntaxKind.EqualsEqualsEqualsToken]: { op: "eq-to-neq", replacement: "!==" },
  [ts.SyntaxKind.ExclamationEqualsEqualsToken]: { op: "neq-to-eq", replacement: "===" },
  [ts.SyntaxKind.LessThanToken]: { op: "lt-to-le", replacement: "<=" },
  [ts.SyntaxKind.LessThanEqualsToken]: { op: "le-to-lt", replacement: "<" },
  [ts.SyntaxKind.GreaterThanToken]: { op: "gt-to-ge", replacement: ">=" },
  [ts.SyntaxKind.GreaterThanEqualsToken]: { op: "ge-to-gt", replacement: ">" },
}

/**
 * Operator mutants in code only. Strings, comments, and type positions are not code.
 * A condition that contains a string (`name === "x"`) is still code: the span is not
 * itself inside the string.
 */
export function findMutants(file: string, text: string, operators: OperatorSet = "core"): FoundMutants {
  const scriptKind = file.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, scriptKind)
  const raw: MutantPoint[] = []
  const visit = (node: ts.Node) => {
    if (isTypePosition(node)) return
    const point = pointAt(source, file, text, node)
    if (point) raw.push(point)
    if (operators === "wide") {
      const extra = widePointAt(source, file, text, node)
      if (extra) raw.push(extra)
    }
    ts.forEachChild(node, visit)
  }
  visit(source)
  const hidden = hiddenSpans(source, text, scriptKind)
  const points: MutantPoint[] = []
  const violations: MutantPoint[] = []
  for (const point of raw) {
    const inside = hidden.some((span) => point.start >= span.start && point.end <= span.end)
    if (inside) violations.push(point)
    else points.push(point)
  }
  return { points, violations }
}

function isTypePosition(node: ts.Node): boolean {
  return (
    ts.isTypeNode(node) ||
    ts.isTypeAliasDeclaration(node) ||
    ts.isInterfaceDeclaration(node) ||
    ts.isTypeParameterDeclaration(node) ||
    ts.isIndexSignatureDeclaration(node)
  )
}

function pointAt(source: ts.SourceFile, file: string, text: string, node: ts.Node): MutantPoint | undefined {
  if (ts.isIfStatement(node) || ts.isWhileStatement(node) || ts.isDoStatement(node)) {
    return negate(source, file, text, node.expression, "negate-condition")
  }
  if (ts.isConditionalExpression(node)) {
    return negate(source, file, text, node.condition, "negate-ternary")
  }
  if (ts.isForStatement(node) && node.condition) {
    return negate(source, file, text, node.condition, "negate-condition")
  }
  if (ts.isBinaryExpression(node)) {
    const swap = SWAP[node.operatorToken.kind]
    if (!swap) return undefined
    const start = node.operatorToken.getStart(source)
    const end = node.operatorToken.getEnd()
    return make(source, file, text, start, end, swap.op, swap.replacement)
  }
  if (node.kind === ts.SyntaxKind.TrueKeyword || node.kind === ts.SyntaxKind.FalseKeyword) {
    const start = node.getStart(source)
    const end = node.getEnd()
    const original = text.slice(start, end)
    const replacement = original === "true" ? "false" : "true"
    const op = original === "true" ? "true-to-false" : "false-to-true"
    return make(source, file, text, start, end, op, replacement)
  }
  if (
    ts.isPrefixUnaryExpression(node) &&
    node.operator === ts.SyntaxKind.ExclamationToken &&
    !ts.isPrefixUnaryExpression(node.operand)
  ) {
    const start = node.getStart(source)
    const end = node.operand.getStart(source)
    return make(source, file, text, start, end, "drop-not", "")
  }
  return undefined
}

/** Arithmetic, constants, and a dropped call. Only in the wide set. */
function widePointAt(source: ts.SourceFile, file: string, text: string, node: ts.Node): MutantPoint | undefined {
  if (ts.isBinaryExpression(node)) {
    const swap = ARITHMETIC[node.operatorToken.kind]
    if (!swap) return undefined
    // `"a" + n` builds a string. Swapping it for `-` changes the type, not the arithmetic.
    if (node.operatorToken.kind === ts.SyntaxKind.PlusToken && (stringish(node.left) || stringish(node.right))) return undefined
    const start = node.operatorToken.getStart(source)
    return make(source, file, text, start, node.operatorToken.getEnd(), swap.op, swap.replacement)
  }
  if (ts.isNumericLiteral(node) && /^\d+$/.test(node.text) && node.getText(source) === node.text) {
    // A property name or an enum key written as a number is a name, not a value.
    if (ts.isPropertyAssignment(node.parent) && node.parent.name === node) return undefined
    if (ts.isEnumMember(node.parent) && node.parent.name === node) return undefined
    const start = node.getStart(source)
    const value = node.text
    if (value === "0") return make(source, file, text, start, node.getEnd(), "const-zero-to-one", "1")
    if (value === "1") return make(source, file, text, start, node.getEnd(), "const-one-to-zero", "0")
    if (value.length <= 15) return make(source, file, text, start, node.getEnd(), "const-inc", String(Number(value) + 1))
    return undefined
  }
  if (ts.isExpressionStatement(node) && ts.isBlock(node.parent)) {
    const call = ts.isAwaitExpression(node.expression) ? node.expression.expression : node.expression
    if (!ts.isCallExpression(call) || call.expression.kind === ts.SyntaxKind.SuperKeyword) return undefined
    // `void 0` keeps the statement valid wherever it stands. The call and its effects are gone.
    const start = node.expression.getStart(source)
    return make(source, file, text, start, node.expression.getEnd(), "drop-call", "void 0")
  }
  return undefined
}

function stringish(node: ts.Expression): boolean {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) || ts.isTemplateExpression(node)) return true
  if (ts.isParenthesizedExpression(node)) return stringish(node.expression)
  if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) return stringish(node.left) || stringish(node.right)
  return false
}

function negate(source: ts.SourceFile, file: string, text: string, expr: ts.Expression, op: string): MutantPoint {
  const start = expr.getStart(source)
  const end = expr.getEnd()
  return make(source, file, text, start, end, op, `!(${text.slice(start, end)})`)
}

function make(
  source: ts.SourceFile,
  file: string,
  text: string,
  start: number,
  end: number,
  op: string,
  replacement: string,
): MutantPoint {
  const { line } = source.getLineAndCharacterOfPosition(start)
  return { file, start, end, line: line + 1, op, original: text.slice(start, end), replacement }
}

function hiddenSpans(source: ts.SourceFile, text: string, scriptKind: ts.ScriptKind): Array<{ start: number; end: number }> {
  const literals: Array<{ start: number; end: number }> = []
  const visit = (node: ts.Node) => {
    if (
      ts.isStringLiteral(node) ||
      ts.isNoSubstitutionTemplateLiteral(node) ||
      ts.isTemplateHead(node) ||
      ts.isTemplateMiddle(node) ||
      ts.isTemplateTail(node) ||
      ts.isRegularExpressionLiteral(node) ||
      ts.isJsxText(node)
    ) {
      literals.push({ start: node.getStart(source), end: node.getEnd() })
    }
    ts.forEachChild(node, visit)
  }
  visit(source)
  const variant = scriptKind === ts.ScriptKind.TSX ? ts.LanguageVariant.JSX : ts.LanguageVariant.Standard
  const scanner = ts.createScanner(ts.ScriptTarget.Latest, false, variant, text)
  const comments: Array<{ start: number; end: number }> = []
  let token = scanner.scan()
  while (token !== ts.SyntaxKind.EndOfFileToken) {
    const leading = ts.getLeadingCommentRanges(text, scanner.getTokenStart()) ?? []
    const trailing = ts.getTrailingCommentRanges(text, scanner.getTokenEnd()) ?? []
    for (const comment of [...leading, ...trailing]) comments.push({ start: comment.pos, end: comment.end })
    token = scanner.scan()
  }
  // A `//` inside a template is text, not a comment. Drop ranges that begin in a literal.
  const real = comments.filter((comment) => !literals.some((span) => comment.start >= span.start && comment.start < span.end))
  return [...literals, ...real]
}
