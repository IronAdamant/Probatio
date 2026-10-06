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
