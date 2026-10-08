import { randomBytes } from "node:crypto"
import { existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, renameSync, rmSync, unlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { scrub } from "../contract.js"
import { filesInDiff, git, patchApplies, readHeader } from "../mutate/patch.js"

export type LedgerOptions = {
  packageDir: string
  repoDir: string
  commit: string
  outDir: string
  srcDir: string
  testsDir: string
  maxLines: number
  /** Newest non-merge commits to read. Null reads the whole history. */
  maxCommits?: number | null
}

export type LedgerStatus = "clean" | "hand" | "handmade" | "skipped"

export type LedgerEntry = {
  id: string
  commit: string
  date: string
  subject: string
  fixer: "human" | "agent"
  status: LedgerStatus
  lines: number
  files: Array<{ file: string; line: number }>
  reason?: string
  patch?: string
}

export type LedgerReport = {
  ok: boolean
  summary: string
  next: string
  nextCall: { argv: string[] } | null
  commit: string
  clean: number
  hand: number
  handmade: number
  skipped: number
  handNeeded: Array<{ id: string; commit: string; file: string; line: number }>
  rest: number
  full: string
  historyBudgetHit: boolean
  commitsScanned: number
}

type DiskPatch = { id: string; raw: string; kind: "history" | "hand" | "user"; fix: string | null }

/** Revert each fix commit's src diff. A clean apply becomes a forward patch. A conflict is reported, never fuzzy-applied. */
export function buildLedger(options: LedgerOptions): LedgerReport {
  const repo = realpathSync(path.resolve(options.repoDir))
  const packageDir = realpathSync(path.resolve(options.packageDir))
  const outDir = path.resolve(options.outDir)
  const resolved = git(repo, ["rev-parse", "--verify", `${options.commit}^{commit}`])
  if (resolved.status !== 0) return fail(outDir, options.commit, "that commit does not resolve")
  const commit = resolved.stdout.trim()
  const packageRel = path.relative(repo, packageDir)
  if (packageRel.startsWith("..")) return fail(outDir, commit, "package is outside the repository")
  const srcPrefix = gitPath(packageRel, options.srcDir)
  const testsPrefix = gitPath(packageRel, options.testsDir)
  const collected = collect(repo, commit, srcPrefix, testsPrefix, options.maxCommits ?? null)
  const candidates = collected.found
  mkdirSync(outDir, { recursive: true })
  const ids = assignIds(candidates.map((item) => item.commit))
  const disk = loadDisk(outDir)
  let workDir: string | null = null
  try {
    const entries: LedgerEntry[] = []
    const bodies = new Map<string, string>()
    for (const candidate of candidates) {
      const id = ids.get(candidate.commit) ?? candidate.commit.slice(0, 7)
      if (candidate.lines > options.maxLines) {
        entries.push(entry(candidate, id, "skipped", [], `src diff is ${candidate.lines} lines, over the ${options.maxLines} line cap`))
        continue
      }
      const fixDiff = git(repo, ["diff", `${candidate.commit}^`, candidate.commit, "--", srcPrefix]).stdout
      const bugDiff = git(repo, ["diff", "-R", `${candidate.commit}^`, candidate.commit, "--", srcPrefix]).stdout
      if (!bugDiff.trim()) continue
      if (!workDir) {
        const work = withWorktree(repo, commit)
        if ("error" in work) return fail(outDir, commit, work.error)
        workDir = work.dir
      }
      // A hand-made mutant that names this fix wins over the history revert. The revert can be
      // unviable (it removes an export a test imports), and the hand mutant keeps the API.
      const named = disk.find((item) => item.kind === "hand" && item.fix === candidate.commit)
      if (named && handmadeApplies(workDir, named.raw)) {
        entries.push({ ...entry(candidate, id, "handmade", filesInDiff(named.raw)), patch: `${named.id}.patch` })
        continue
      }
      const applied = git(workDir, ["apply", "--check", "--whitespace=nowarn", "-"], bugDiff)
      if (applied.status === 0) {
        bodies.set(id, historyPatch(candidate.commit, bugDiff))
        entries.push({ ...entry(candidate, id, "clean", filesInDiff(fixDiff)), patch: `${id}.patch` })
        continue
      }
      const where = failedAt(applied.stderr) ?? filesInDiff(fixDiff)[0] ?? { file: srcPrefix, line: 1 }
      const owned = disk.find((item) => item.kind !== "history" && (item.fix === candidate.commit || item.id === id))
      if (owned && handmadeApplies(workDir, owned.raw)) {
        entries.push({ ...entry(candidate, id, "handmade", [where]), patch: `${owned.id}.patch` })
        continue
      }
      const reason = owned ? "hand-made patch does not apply" : "patch does not apply"
      entries.push(entry(candidate, id, "hand", [where], reason))
    }
    writeOut(outDir, commit, packageRel || ".", entries, bodies, disk)
    return report(outDir, repo, packageDir, commit, entries, collected)
  } finally {
    if (workDir) removeWorktree(repo, workDir)
  }
}

type Candidate = {
  commit: string
  date: string
  subject: string
  fixer: "human" | "agent"
  lines: number
}

function collect(
  repo: string,
  commit: string,
  srcPrefix: string,
  testsPrefix: string,
  maxCommits: number | null,
): { found: Candidate[]; scanned: number; budgetHit: boolean } {
  const args = ["rev-list", "--no-merges"]
  if (maxCommits !== null) args.push("-n", String(maxCommits + 1))
  args.push(commit)
  const list = git(repo, args)
  if (list.status !== 0) return { found: [], scanned: 0, budgetHit: false }
  const shas = list.stdout.split("\n").filter((sha) => sha.length > 0)
  const budgetHit = maxCommits !== null && shas.length > maxCommits
  const limited = budgetHit ? shas.slice(0, maxCommits) : shas
  const found: Candidate[] = []
  for (const sha of limited) {
    if (git(repo, ["rev-parse", "--verify", `${sha}^`]).status !== 0) continue
    const rows = numstat(repo, sha)
    let lines = 0
    let src = false
    let tests = false
    for (const row of rows) {
      if (under(row.file, srcPrefix)) {
        src = true
        if (row.add !== "-" && row.del !== "-") lines += Number(row.add) + Number(row.del)
      }
      if (under(row.file, testsPrefix)) tests = true
    }
    if (!src) continue
    const meta = git(repo, ["log", "-1", `--format=%cI%n%s%n%b`, sha])
    if (meta.status !== 0) continue
    const [date, subject, ...rest] = meta.stdout.replace(/\n$/, "").split("\n")
    const body = rest.join("\n")
    const hasTrailer = /^Fixes-bug:/m.test(body)
    if (!hasTrailer && !(src && tests)) continue
    if (!hasTrailer && lines === 0) continue
    // A version bump ships whatever was finished. Reverting it puts back several changes, not one bug.
    if (!hasTrailer && bumpsVersion(repo, sha)) continue
    found.push({
      commit: sha,
      date: date ?? "",
      subject: subject ?? "",
      fixer: /^Made-by:[ \t]*\S+[ \t]*$/m.test(body) ? "agent" : "human",
      lines,
    })
  }
  return { found, scanned: limited.length, budgetHit }
}

const MANIFESTS = ["package.json", "Cargo.toml", "pyproject.toml", "setup.cfg"].map((name) => `:(glob)**/${name}`)

/** True when the commit changes a project version line in a manifest. */
function bumpsVersion(repo: string, sha: string): boolean {
  const diff = git(repo, ["show", "--format=", "-U0", sha, "--", ...MANIFESTS])
  if (diff.status !== 0) return false
  return diff.stdout
    .split("\n")
    .filter((line) => /^[-+]/.test(line) && !line.startsWith("+++") && !line.startsWith("---"))
    .some((line) => /"version"\s*:/.test(line) || /^[-+]\s*version\s*=/.test(line))
}

function entry(candidate: Candidate, id: string, status: LedgerStatus, files: Array<{ file: string; line: number }>, reason?: string): LedgerEntry {
  return {
    id,
    commit: candidate.commit,
    date: candidate.date,
    subject: candidate.subject,
    fixer: candidate.fixer,
    status,
    lines: candidate.lines,
    files,
    ...(reason ? { reason } : {}),
  }
}

function writeOut(outDir: string, commit: string, packageRel: string, entries: LedgerEntry[], bodies: Map<string, string>, disk: DiskPatch[]): void {
  const cleanIds = new Set(entries.filter((item) => item.status === "clean").map((item) => item.id))
  for (const item of disk) {
    if (item.kind === "history" && !cleanIds.has(item.id)) unlinkSync(path.join(outDir, `${item.id}.patch`))
  }
  for (const item of entries) {
    const body = bodies.get(item.id)
    if (item.status !== "clean" || !body) continue
    const dest = path.join(outDir, `${item.id}.patch`)
    const temp = `${dest}.${process.pid}.tmp`
    writeFileSync(temp, body)
    renameSync(temp, dest)
  }
  const recorded = {
    commit,
    package: packageRel.split(path.sep).join("/"),
    entries,
  }
  const dest = path.join(outDir, "ledger.json")
  const temp = `${dest}.${process.pid}.tmp`
  writeFileSync(temp, `${JSON.stringify(scrub(recorded), null, 2)}\n`)
  renameSync(temp, dest)
}

function report(
  outDir: string,
  repo: string,
  packageDir: string,
  commit: string,
  entries: LedgerEntry[],
  collected: { scanned: number; budgetHit: boolean },
): LedgerReport {
  const clean = entries.filter((item) => item.status === "clean").length
  const handEntries = entries.filter((item) => item.status === "hand").sort(byNewest)
  const handmade = entries.filter((item) => item.status === "handmade").length
  const skipped = entries.filter((item) => item.status === "skipped").length
  const shown = handEntries.slice(0, 10).map((item) => ({
    id: item.id,
    commit: item.commit,
    file: item.files[0]?.file ?? "",
    line: item.files[0]?.line ?? 0,
  }))
  const parts = [`${clean} fixes reverse cleanly`]
  if (handEntries.length > 0) parts.push(`${handEntries.length} need a hand-made mutant`)
  if (handmade > 0) parts.push(`${handmade} hand-made mutants apply`)
  if (skipped > 0) parts.push(`${skipped} are over the line cap`)
  if (collected.budgetHit) {
    const commits = collected.scanned === 1 ? "1 commit" : `${collected.scanned} commits`
    parts.push(`Stopped after ${commits}. History was not fully scanned`)
  }
  const summary = `${parts.join(". ")}.`
  let next = "No fix commit to revert."
  if (handEntries.length > 0) {
    const first = shown[0]
    next = first.file
      ? `First fix that needs a hand-made mutant is ${first.id} at ${first.file}:${first.line}.`
      : `First fix that needs a hand-made mutant is ${first.id}.`
  } else if (clean + handmade > 0) next = "Run the ledger mutants."
  const nextCall =
    handEntries.length === 0 && clean + handmade > 0
      ? {
          argv: [
            "mutate",
            "run",
            "--package",
            packageDir,
            "--repo",
            repo,
            "--commit",
            commit,
            "--patches",
            outDir,
            "--out",
            path.join(outDir, "runs"),
          ],
        }
      : null
  return {
    ok: true,
    summary,
    next,
    nextCall,
    commit,
    clean,
    hand: handEntries.length,
    handmade,
    skipped,
    handNeeded: shown,
    rest: Math.max(handEntries.length - shown.length, 0),
    full: path.join(outDir, "ledger.json"),
    historyBudgetHit: collected.budgetHit,
    commitsScanned: collected.scanned,
  }
}

function fail(outDir: string, commit: string, error: string): LedgerReport {
  return {
    ok: false,
    summary: error,
    next: "Fix that and run the same command again.",
    nextCall: null,
    commit,
    clean: 0,
    hand: 0,
    handmade: 0,
    skipped: 0,
    handNeeded: [],
    rest: 0,
    full: path.join(outDir, "ledger.json"),
    historyBudgetHit: false,
    commitsScanned: 0,
  }
}

function historyPatch(commit: string, bugDiff: string): string {
  const body = bugDiff.endsWith("\n") ? bugDiff : `${bugDiff}\n`
  return `# probatio-mutant direction=forward meaning=apply-to-introduce-the-bug\n# probatio-ledger source=history fix=${commit}\n${body}`
}

function handmadeApplies(worktree: string, raw: string): boolean {
  const named = readHeader(raw).direction
  if (named) return patchApplies(worktree, raw, named)
  return patchApplies(worktree, raw, "forward")
}

function failedAt(stderr: string): { file: string; line: number } | null {
  const match = stderr.match(/patch failed: ([^:\n]+):(\d+)/)
  if (!match) return null
  return { file: match[1], line: Number(match[2]) }
}

function numstat(repo: string, sha: string): Array<{ add: string; del: string; file: string }> {
  const result = git(repo, ["diff-tree", "-r", "--numstat", "--no-commit-id", sha])
  if (result.status !== 0) return []
  const rows: Array<{ add: string; del: string; file: string }> = []
  for (const line of result.stdout.split("\n")) {
    if (!line.trim()) continue
    const [add, del, file] = line.split("\t")
    if (!file) continue
    rows.push({ add, del, file })
  }
  return rows
}

function loadDisk(outDir: string): DiskPatch[] {
  if (!existsSync(outDir)) return []
  const found: DiskPatch[] = []
  for (const name of readdirSync(outDir)) {
    if (!name.endsWith(".patch")) continue
    const raw = readFileSync(path.join(outDir, name), "utf8")
    const header = raw.split("\n").filter((line) => line.startsWith("#")).join("\n")
    const kind = /source=hand\b/.test(header) ? "hand" : /source=history\b/.test(header) ? "history" : "user"
    const fix = header.match(/\bfix=([0-9a-f]{7,40})\b/)?.[1] ?? null
    found.push({ id: path.basename(name, ".patch"), raw, kind, fix })
  }
  return found
}

function assignIds(shas: string[]): Map<string, string> {
  const used = new Set<string>()
  const ids = new Map<string, string>()
  for (const sha of [...shas].sort()) {
    let size = Math.min(7, sha.length)
    let id = sha.slice(0, size)
    while (used.has(id) && size < sha.length) {
      size += 1
      id = sha.slice(0, size)
    }
    used.add(id)
    ids.set(sha, id)
  }
  return ids
}

function withWorktree(repo: string, commit: string): { dir: string } | { error: string } {
  const dir = path.join(tmpdir(), `probatio-ledger-${randomBytes(4).toString("hex")}`)
  rmSync(dir, { recursive: true, force: true })
  const added = git(repo, ["worktree", "add", "--detach", "--quiet", dir, commit])
  if (added.status !== 0) return { error: (added.stderr || "worktree add failed").trim() }
  return { dir }
}

function removeWorktree(repo: string, dir: string): void {
  git(repo, ["worktree", "remove", "--force", dir])
  rmSync(dir, { recursive: true, force: true })
  git(repo, ["worktree", "prune"])
}

function gitPath(packageRel: string, dir: string): string {
  const joined = packageRel ? path.join(packageRel, dir) : dir
  return joined.split(path.sep).join("/")
}

function under(file: string, prefix: string): boolean {
  return file === prefix || file.startsWith(`${prefix}/`)
}

function byNewest(left: LedgerEntry, right: LedgerEntry): number {
  if (left.date !== right.date) return right.date < left.date ? -1 : 1
  return left.id < right.id ? -1 : left.id > right.id ? 1 : 0
}
