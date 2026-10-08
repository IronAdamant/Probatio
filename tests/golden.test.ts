import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { homedir, tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

test("golden re-records wording, refuses a contract change without a trailer, and keeps an invariant", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-golden-"))
  const recorded = path.join(dir, "recorded.json")
  const actual = path.join(dir, "actual.json")
  const original = {
    rows: {
      gate: { ok: true, reason: "opened", status: "pass", next: "Run the next check.", nextCall: { tool: "probatio" }, shot: "/tmp/x.png" },
      other: { ok: true, reason: "steady", status: "pass", next: "Stay.", nextCall: { tool: "probatio" } },
    },
  }
  try {
    writeFileSync(recorded, `${JSON.stringify(original, null, 2)}\n`)
    writeFileSync(actual, `${JSON.stringify({
      rows: {
        gate: { ...original.rows.gate, next: "Run the check again." },
        other: original.rows.other,
      },
    }, null, 2)}\n`)
    const wording = run(["golden", "check", "--recorded", recorded, "--actual", actual, "--update", "wording"])
    assert.equal(wording.status, 0, wording.stderr)
    const wordingJson = JSON.parse(wording.stdout) as { ok: boolean; updated: boolean; contract: string[] }
    assert.equal(wordingJson.ok, true)
    assert.equal(wordingJson.updated, true)
    assert.deepEqual(wordingJson.contract, [])
    const afterWording = JSON.parse(readFileSync(recorded, "utf8")) as typeof original
    assert.equal(afterWording.rows.gate.next, "Run the check again.")
    assert.equal(afterWording.rows.gate.ok, true)
    assert.equal(afterWording.rows.gate.reason, "opened")
    assert.equal(afterWording.rows.gate.shot, "/tmp/x.png")

    writeFileSync(recorded, `${JSON.stringify(original, null, 2)}\n`)
    writeFileSync(actual, `${JSON.stringify({
      rows: {
        gate: { ...original.rows.gate, ok: false, reason: "shut" },
        other: { ...original.rows.other, ok: false, reason: "also shut" },
      },
    }, null, 2)}\n`)
    const denied = run(["golden", "check", "--recorded", recorded, "--actual", actual, "--update", "all", "--message", "Golden-Change: gate: the gate now fails closed"])
    assert.equal(denied.status, 1, denied.stderr)
    assert.equal(readFileSync(recorded, "utf8"), `${JSON.stringify(original, null, 2)}\n`)

    const allowed = run(["golden", "check", "--recorded", recorded, "--actual", actual, "--update", "all", "--message", "Golden-Change: gate: the gate now fails closed\nGolden-Change: other: the other row follows"])
    assert.equal(allowed.status, 0, allowed.stderr)
    const allowedJson = JSON.parse(allowed.stdout) as { contract: string[] }
    assert.deepEqual(allowedJson.contract, ["gate", "other"])
    const afterContract = JSON.parse(readFileSync(recorded, "utf8")) as typeof original
    assert.equal(afterContract.rows.gate.ok, false)
    assert.equal(afterContract.rows.gate.reason, "shut")

    writeFileSync(recorded, `${JSON.stringify(original, null, 2)}\n`)
    const homeActual = {
      rows: {
        gate: { ...original.rows.gate, next: `see ${homedir()}/notes` },
        other: original.rows.other,
      },
    }
    writeFileSync(actual, `${JSON.stringify(homeActual, null, 2)}\n`)
    const invariant = run(["golden", "check", "--recorded", recorded, "--actual", actual, "--update", "all", "--message", "Golden-Change: gate: allow the path"])
    assert.equal(invariant.status, 1, invariant.stderr)
    const invariantJson = JSON.parse(invariant.stdout) as { ok: boolean; summary: string }
    assert.equal(invariantJson.ok, false)
    assert.equal(invariantJson.summary.includes(homedir()), false)
    assert.equal(readFileSync(recorded, "utf8"), `${JSON.stringify(original, null, 2)}\n`)

    writeFileSync(actual, `${JSON.stringify({
      rows: { gate: { ok: true, status: "pass", next: "Run the next check.", nextCall: { tool: "probatio" } }, other: original.rows.other },
    }, null, 2)}\n`)
    const noReason = run(["golden", "check", "--recorded", recorded, "--actual", actual, "--update", "all", "--message", "Golden-Change: gate: drop the reason"])
    assert.equal(noReason.status, 1, noReason.stderr)
    assert.match(JSON.parse(noReason.stdout).summary as string, /ok without a reason/)
    assert.equal(readFileSync(recorded, "utf8"), `${JSON.stringify(original, null, 2)}\n`)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("a golden row treats nextLead as wording and refuses a contract field without a trailer", () => {
  const source = path.join(root, "tests", "fixtures", "live-host-change.json")
  const originalBytes = readFileSync(source)
  const table = JSON.parse(originalBytes.toString("utf8")) as Record<string, unknown>
  const found = Object.entries(table).find(([key, value]) => {
    if (!value || typeof value !== "object" || Array.isArray(value) || key.includes(": ")) return false
    const row = value as Record<string, unknown>
    return typeof row.nextLead === "string" && ("hostChanged" in row || "nextCall" in row)
  })
  assert.ok(found, "live-host-change.json has no prose-and-contract row")
  const [key, rowValue] = found
  const row = rowValue as Record<string, unknown>
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-golden-auspex-"))
  const recorded = path.join(dir, "recorded.json")
  const actual = path.join(dir, "actual.json")
  const prose = `${row.nextLead} Wording only.`
  try {
    writeFileSync(recorded, originalBytes)
    const wordingTable = structuredClone(table)
    ;(wordingTable[key] as Record<string, unknown>).nextLead = prose
    writeFileSync(actual, `${JSON.stringify(wordingTable, null, 2)}\n`)
    const wording = run(["golden", "check", "--recorded", recorded, "--actual", actual, "--update", "wording"])
    assert.equal(wording.status, 0, wording.stderr + wording.stdout)
    const wordingJson = JSON.parse(wording.stdout) as { ok: boolean; wording: string[]; contract: string[] }
    assert.equal(wordingJson.ok, true)
    assert.ok(wordingJson.wording.includes(key), JSON.stringify(wordingJson.wording))
    assert.equal(wordingJson.contract.includes(key), false, JSON.stringify(wordingJson.contract))
    const afterWording = JSON.parse(readFileSync(recorded, "utf8")) as Record<string, unknown>
    assert.deepEqual(Object.keys(afterWording).sort(), Object.keys(table).sort())
    assert.deepEqual(changedPaths(table, afterWording), [`${key}.nextLead`])
    const updated = afterWording[key] as Record<string, unknown>
    assert.equal(updated.nextLead, prose)
    assert.deepEqual(updated.hostChanged, row.hostChanged)
    assert.deepEqual(updated.nextCall, row.nextCall)

    writeFileSync(recorded, originalBytes)
    const contractTable = structuredClone(table)
    const contractRow = contractTable[key] as Record<string, unknown>
    contractRow.hostChanged = contractRow.hostChanged === true ? false : true
    writeFileSync(actual, `${JSON.stringify(contractTable, null, 2)}\n`)
    const denied = run(["golden", "check", "--recorded", recorded, "--actual", actual, "--update", "all"])
    assert.notEqual(denied.status, 0)
    const deniedJson = JSON.parse(denied.stdout) as { ok: boolean; summary: string }
    assert.equal(deniedJson.ok, false)
    assert.match(deniedJson.summary, /Contract changed/)
    assert.equal(deniedJson.summary.includes(key), true)
    assert.equal(readFileSync(recorded).equals(originalBytes), true)

    const allowed = run([
      "golden", "check",
      "--recorded", recorded,
      "--actual", actual,
      "--update", "all",
      "--message", `Golden-Change: ${key}: the host flag is allowed to change`,
    ])
    assert.equal(allowed.status, 0, allowed.stderr + allowed.stdout)
    const afterContract = JSON.parse(readFileSync(recorded, "utf8")) as Record<string, unknown>
    assert.deepEqual(Object.keys(afterContract).sort(), Object.keys(table).sort())
    assert.deepEqual(changedPaths(table, afterContract), [`${key}.hostChanged`])
    assert.equal((afterContract[key] as Record<string, unknown>).hostChanged, contractRow.hostChanged)
    assert.equal(readFileSync(source).equals(originalBytes), true)
  } finally {
    rmSync(dir, { recursive: true, force: true })
    assert.equal(readFileSync(source).equals(originalBytes), true)
  }
})

function changedPaths(before: unknown, after: unknown, prefix: string[] = []): string[] {
  if (JSON.stringify(before) === JSON.stringify(after)) return []
  if (isRecord(before) && isRecord(after)) {
    const keys = [...new Set([...Object.keys(before), ...Object.keys(after)])].sort()
    return keys.flatMap((item) => changedPaths(before[item], after[item], [...prefix, item]))
  }
  return [prefix.join(".")]
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value)
}

function run(args: string[]) {
  return spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", timeout: 300_000 })
}
