import { spawnSync } from "node:child_process"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"

export type PatchDirection = "forward" | "reverse"

export function readHeader(raw: string): { direction: PatchDirection | null; body: string } {
  const lines = raw.split(/\r?\n/)
  let direction: PatchDirection | null = null
  let index = 0
  while (index < lines.length && !lines[index].startsWith("---") && !lines[index].startsWith("diff ")) {
    const match = lines[index].match(/direction=(forward|reverse)/)
    if (match) direction = match[1] as PatchDirection
    index++
  }
  const body = lines.slice(index).join("\n")
  return { direction, body: body.endsWith("\n") || body.length === 0 ? body : `${body}\n` }
}

/** `diff -u` patch that applies onto the original and produces the mutant. */
export function forwardDiff(rel: string, original: string, mutated: string): string {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-diff-"))
  try {
    const before = path.join(dir, "before")
    const after = path.join(dir, "after")
    writeFileSync(before, original)
    writeFileSync(after, mutated)
    const diff = spawnSync("diff", ["-u", "-L", `a/${rel}`, "-L", `b/${rel}`, before, after], { encoding: "utf8" })
    if (diff.status === 0) return ""
    if (diff.status !== 1) throw new Error(diff.stderr.trim() || "diff failed")
    const body = diff.stdout.endsWith("\n") ? diff.stdout : `${diff.stdout}\n`
    return `# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug\n${body}`
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

export function filesInDiff(raw: string): Array<{ file: string; line: number }> {
  const body = readHeader(raw).body
  const files: Array<{ file: string; line: number }> = []
  const chunks = body.split(/^diff --git .*$/m)
  const pieces = chunks.length > 1 ? chunks.slice(1) : [body]
  for (const piece of pieces) {
    const plus = piece.match(/^\+\+\+ ([^\t\n]+)/m)
    const hunk = piece.match(/^@@ -\d+(?:,\d+)? \+(\d+)/m)
    if (!plus) continue
    let file = plus[1]
    if (file === "/dev/null") continue
    // git diff -R swaps the labels, so the path may be a/ or b/.
    if (file.startsWith("a/") || file.startsWith("b/")) file = file.slice(2)
    files.push({ file, line: hunk ? Number(hunk[1]) : 1 })
  }
  return files
}

/**
 * Original lines a patch removes or replaces.
 * A unified hunk starts on context, so the hunk header is not the edited line.
 */
export function changedLines(raw: string): Array<{ file: string; line: number }> {
  const body = readHeader(raw).body
  const chunks = body.split(/^diff --git .*$/m)
  const pieces = chunks.length > 1 ? chunks.slice(1) : [body]
  const found: Array<{ file: string; line: number }> = []
  for (const piece of pieces) {
    const plus = piece.match(/^\+\+\+ ([^\t\n]+)/m)
    if (!plus) continue
    let file = plus[1]
    if (file === "/dev/null") continue
    if (file.startsWith("a/") || file.startsWith("b/")) file = file.slice(2)
    let oldLine = 0
    let inHunk = false
    for (const line of piece.split(/\r?\n/)) {
      const hunk = /^@@ -(\d+)(?:,\d+)? \+\d+/.exec(line)
      if (hunk) {
        oldLine = Number(hunk[1])
        inHunk = true
        continue
      }
      if (!inHunk) continue
      if (line.startsWith("+") || line.startsWith("\\")) continue
      if (line.startsWith("-")) {
        found.push({ file, line: oldLine })
        oldLine += 1
        continue
      }
      if (line.startsWith(" ")) {
        oldLine += 1
        continue
      }
      inHunk = false
    }
  }
  return found
}

export function git(repo: string, args: string[], input?: string): { status: number; stdout: string; stderr: string } {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8", input })
  const stderr = [result.stderr ?? "", result.error?.message ?? ""].filter((part) => part.length > 0).join("\n")
  return { status: result.status ?? 1, stdout: result.stdout ?? "", stderr }
}

export function patchApplies(repo: string, raw: string, direction: PatchDirection): boolean {
  const args = ["apply", "--check", "--whitespace=nowarn", ...(direction === "reverse" ? ["-R"] : []), "-"]
  return git(repo, args, readHeader(raw).body).status === 0
}

export function chooseDirection(
  repo: string,
  raw: string,
  requested: "auto" | PatchDirection,
): PatchDirection | null {
  if (requested !== "auto") return requested
  const header = readHeader(raw).direction
  if (header) return header
  if (patchApplies(repo, raw, "forward")) return "forward"
  if (patchApplies(repo, raw, "reverse")) return "reverse"
  return null
}

export function applyPatch(repo: string, raw: string, direction: PatchDirection): { ok: true } | { ok: false; error: string } {
  const args = ["apply", "--whitespace=nowarn", ...(direction === "reverse" ? ["-R"] : []), "-"]
  const result = git(repo, args, readHeader(raw).body)
  if (result.status === 0) return { ok: true }
  return { ok: false, error: (result.stderr || result.stdout).trim() || "apply failed" }
}

/** Put a throwaway worktree back. A diff label is not trusted for the path list. */
export function restoreTree(repo: string): void {
  git(repo, ["checkout", "--", "."])
  // .probatio.json may be the user's uncommitted copy, put there when the worktree was made.
  git(repo, ["clean", "-fd", "-e", "node_modules", "-e", ".probatio.json"])
}
