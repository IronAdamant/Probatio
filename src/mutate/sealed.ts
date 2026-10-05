import { readFileSync } from "node:fs"

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

export const LABEL_LEAK_SUMMARY = "The label was copied into the report."

function field(text: string, key: string): string {
  const match = text.match(new RegExp(`^${key}=(.*)$`, "m"))
  return match?.[1]?.trim() ?? ""
}
