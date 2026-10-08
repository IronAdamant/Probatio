import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { createRequire } from "node:module"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"
import { SCHEMA_VERSION } from "../src/contract.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")
const require = createRequire(import.meta.url)
const Ajv2020 = require("ajv/dist/2020").default as new (options: object) => {
  compile: (schema: object) => ((data: unknown) => boolean) & { errors?: unknown }
}

function schemaFor(name: string): object {
  return JSON.parse(readFileSync(path.join(root, "schemas", `${name}.schema.json`), "utf8")) as object
}

function walk(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? walk(path.join(dir, entry.name)) : [path.join(dir, entry.name)],
  )
}

test("every command name the CLI prints has a published schema at the current schemaVersion", () => {
  const names = new Set<string>()
  // suites.ts names suite kinds (`command: "nasm"`), not envelopes.
  for (const file of walk(path.join(root, "src")).filter((file) => file.endsWith(".ts") && !file.endsWith(`${path.sep}suites.ts`))) {
    for (const match of readFileSync(file, "utf8").matchAll(/command: "([a-z-]+(?:\.[a-z-]+)?)"/g)) names.add(match[1])
  }
  names.add("schema")
  const published = readdirSync(path.join(root, "schemas")).map((file) => file.replace(/\.schema\.json$/, ""))
  for (const name of names) assert.ok(published.includes(name), `no schema for ${name}`)
  for (const name of published) {
    const schema = schemaFor(name) as { properties: { schemaVersion: { const: number }; command: { const: string } } }
    assert.equal(schema.properties.schemaVersion.const, SCHEMA_VERSION, `${name} is not at schemaVersion ${SCHEMA_VERSION}`)
    assert.equal(schema.properties.command.const, name)
  }
})

test("real output of every command validates against its schema", { timeout: 300_000 }, () => {
  const ajv = new Ajv2020({ allErrors: true, strict: false })
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-schemas-"))
  const state = path.join(dir, "state")
  const pkg = path.join(dir, "pkg")
  const cli = (args: string[]) => {
    const result = spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8", timeout: 200_000 })
    return JSON.parse(result.stdout) as { command: string; ok: boolean; [key: string]: unknown }
  }
  const seen = new Map<string, number>()
  const validators = new Map<string, ReturnType<typeof ajv.compile>>()
  const check = (body: { command: string; ok: boolean }) => {
    if (!validators.has(body.command)) validators.set(body.command, ajv.compile(schemaFor(body.command)))
    const validate = validators.get(body.command)!
    assert.equal(validate(body), true, `${body.command} (ok ${body.ok}): ${JSON.stringify(validate.errors)}\n${JSON.stringify(body).slice(0, 600)}`)
    seen.set(body.command, (seen.get(body.command) ?? 0) + 1)
    return body
  }
  try {
    mkdirSync(path.join(pkg, "src"), { recursive: true })
    mkdirSync(path.join(pkg, "tests"))
    writeFileSync(path.join(pkg, "package.json"), '{ "type": "module" }\n')
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n >= 0\n}\n")
    writeFileSync(path.join(pkg, "tests", "gate.test.ts"), 'import test from "node:test"\ntest("placeholder", () => {})\n')
    git(pkg, ["init", "-q"])
    commit(pkg, "init")
    writeFileSync(path.join(pkg, "src", "gate.ts"), "export function gate(n: number): boolean {\n  return n > 0\n}\n")
    writeFileSync(
      path.join(pkg, "tests", "gate.test.ts"),
      'import assert from "node:assert/strict"\nimport test from "node:test"\nimport { gate } from "../src/gate.ts"\ntest("zero stays shut", () => {\n  assert.equal(gate(0), false)\n})\n',
    )
    commit(pkg, "Keep zero shut\n\nFixes-bug: zero opened the gate")

    const generated = check(cli(["mutate", "generate", "--package", pkg, "--out", path.join(dir, "gen"), "--operators", "wide"]))
    check(cli(["mutate", "run", "--package", pkg, "--patches", path.join(dir, "gen", "mutants"), "--out", path.join(dir, "run"), "--no-confirm"]))
    check(cli(["mutate", "run", "--package", pkg, "--patches", path.join(dir, "missing"), "--out", path.join(dir, "bad")]))
    check(cli(["mutate", "tally", "--out", path.join(dir, "run")]))
    check(cli(["mutate", "tally", "--out", path.join(dir, "nothing")]))
    check(cli(["ledger", "build", "--package", pkg, "--out", path.join(pkg, "ledger")]))
    commit(pkg, "ledger")
    check(cli(["ledger", "check", "--package", pkg, "--out", path.join(dir, "check")]))
    check(cli(["verify-change", "--package", pkg, "--base", "HEAD~2", "--out", path.join(dir, "verify"), "--operators", "wide"]))
    const id = (generated.mutants as Array<{ id: string }>)[0].id
    check(cli(["check-kill", id, "--package", pkg, "--patches", path.join(dir, "gen", "mutants"), "--out", path.join(dir, "ck")]))
    const fixture = path.join(root, "tests", "fixtures", "live-host-change.json")
    check(cli(["golden", "check", "--recorded", fixture, "--actual", fixture]))
    check(cli(["golden", "record", "--package", pkg, "--module", "src/gate.ts", "--tests", "tests/gate.test.ts", "--dir", "tests/golden"]))
    check(cli(["golden", "compare", "--package", pkg]))
    check(cli(["gap", "fix", "--state", state, "--id", "g1", "--commit", "HEAD", "--file", "src/gate.ts", "--line", "2", "--guard", "tests/gate.test.ts:5", "--root", pkg]))
    check(cli(["gap", "revert", "--state", state, "--id", "g1"]))
    check(cli(["guard", "check", "--state", state, "--guard", "tests/gate.test.ts:5", "--root", pkg]))
    check(cli(["findings", "add", "--state", state, "--id", "f1", "--status", "equivalent", "--reason", "same behaviour"]))
    check(cli(["seal", "--state", state, "--id", "g9"]))
    check(cli(["status", "--state", state, "--page", path.join(dir, "PROBATIO.md")]))
    check(cli(["queue", "seed", "--state", state, "--id", "q1"]))
    check(cli(["queue", "claim", "--state", state, "--id", "q1", "--agent", "a", "--now", "1000"]))
    check(cli(["queue", "reap", "--state", state, "--now", "999999"]))
    check(cli(["schema"]))
    check(cli(["schema", "mutate.run"]))
    check(cli(["nonsense"]))
    for (const name of ["mutate.generate", "mutate.run", "mutate.tally", "ledger.build", "ledger.check", "verify-change", "check-kill", "golden.record", "status"]) {
      assert.ok(seen.has(name), `${name} was not exercised`)
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}

function commit(repo: string, message: string) {
  git(repo, ["add", "."])
  git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", message])
}
