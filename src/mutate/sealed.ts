import { spawnSync } from "node:child_process"
import { readdirSync, readFileSync, statSync } from "node:fs"
import path from "node:path"

export type SealedLabel = {
  bugId: string
  issueId: string
  expectedFail: string
}

/** The label stays in its own file. The run report must not repeat these three values. */
export function readSealedLabel(file: string): SealedLabel {
  let text = ""
  try {
    text = readFileSync(file, "utf8")
  } catch {
    throw new Error("label file could not be read")
  }
  const bugId = field(text, "bug_id")
  const issueId = field(text, "issue_id")
  const expectedFail = field(text, "expected_fail")
  if (!bugId || !issueId || !expectedFail) throw new Error("label file needs bug_id, issue_id, and expected_fail")
  return { bugId, issueId, expectedFail }
}

export function labelLeaked(body: string, label: SealedLabel): boolean {
  return body.includes(label.bugId) || body.includes(label.issueId) || body.includes(label.expectedFail)
}

/** The envelope can be clean while a result file on disk still repeats the label. */
export function sealedTreeLeaked(dir: string, label: SealedLabel): boolean {
  const visit = (current: string): boolean => {
    let names: string[]
    try {
      names = readdirSync(current)
    } catch {
      return false
    }
    for (const name of names) {
      const full = path.join(current, name)
      let info
      try {
        info = statSync(full)
      } catch {
        continue
      }
      if (info.isDirectory()) {
        if (visit(full)) return true
        continue
      }
      if (!info.isFile() || info.size > 2_000_000) continue
      try {
        if (labelLeaked(readFileSync(full, "utf8"), label)) return true
      } catch {
        // A binary artifact is not a report.
      }
    }
    return false
  }
  return visit(dir)
}

export const LABEL_LEAK_SUMMARY = "The label was copied into the report."

function field(text: string, key: string): string {
  const match = text.match(new RegExp(`^${key}=(.*)$`, "m"))
  return match?.[1]?.trim() ?? ""
}

/**
 * A sealed run hides the revealing test, but a fix commit often edits an older test too.
 * A kill by that edited test is the fix's own test, not an older test that caught the bug.
 * Returns the edited test files that killed, and whether any kill came from a test the fix left alone.
 */
export function fixEditedKillers(
  repo: string,
  packageDir: string,
  commit: string,
  hide: string[],
  kills: Array<{ killedBy: string[] }>,
): { files: string[]; olderTestCatch: boolean } {
  if (kills.length === 0) return { files: [], olderTestCatch: false }
  const diff = spawnSync("git", ["-C", repo, "diff", "--name-only", `${commit}^`, commit], { encoding: "utf8" })
  // No parent, or git failed: nothing is known to be edited, so every kill stays an older-test catch.
  if (diff.status !== 0) return { files: [], olderTestCatch: true }
  const hidden = new Set(hide.map((rel) => rel.split(path.sep).join("/")))
  const edited = diff.stdout
    .split("\n")
    .filter((rel) => rel.length > 0)
    .map((rel) => ({ repoRel: rel, pkgRel: path.relative(packageDir, path.join(repo, rel)).split(path.sep).join("/") }))
    .filter((item) => !item.pkgRel.startsWith("..") && !hidden.has(item.pkgRel))
  const text = new Map<string, string>()
  const contents = (repoRel: string) => {
    if (!text.has(repoRel)) {
      const shown = spawnSync("git", ["-C", repo, "show", `${commit}:${repoRel}`], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 })
      text.set(repoRel, shown.status === 0 ? shown.stdout : "")
    }
    return text.get(repoRel) ?? ""
  }
  const fileFor = (id: string) => edited.find((item) => killerInFile(id, item.pkgRel, () => contents(item.repoRel)))
  const files = new Set<string>()
  let olderTestCatch = false
  for (const kill of kills) {
    const owners = kill.killedBy.map(fileFor)
    for (const owner of owners) if (owner) files.add(owner.pkgRel)
    if (owners.some((owner) => !owner)) olderTestCatch = true
  }
  return { files: [...files].sort(), olderTestCatch }
}

/** True when a kill id names this file, its class, or a test written in it. */
function killerInFile(id: string, rel: string, read: () => string): boolean {
  if (id.includes(rel) || id.startsWith(`${path.posix.basename(rel)}::`)) return true
  const stem = path.posix.basename(rel).replace(/\.[^.]+$/, "")
  // JUnit `pkg.ClassTest.method` or `ClassTest#method`, Go and Rust paths.
  if (stem.length > 0 && id.split(/[.#:/]+/).includes(stem)) return true
  const leaf = id.split(/::|#/).pop()?.split(/\.(?=[^.]+$)/).pop()?.replace(/\[.*\]$/, "").trim() ?? ""
  if (leaf.length < 4) return false
  const body = read()
  return [`"${leaf}"`, `'${leaf}'`, `\`${leaf}\``, `${leaf}(`, `def ${leaf}`, `func ${leaf}`, `fn ${leaf}`].some((needle) => body.includes(needle))
}
