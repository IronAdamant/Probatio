import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { missingPytest } from "./require-tool.ts"
import { fileURLToPath } from "node:url"
import { forwardDiff } from "../src/mutate/patch.ts"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

// Same length on purpose. A one-second mtime plus an equal size keeps the old .pyc.
const fresh = ["def open(n):", "    return n >  0", ""].join("\n")
const mutated = ["def open(n):", "    return n >= 0", ""].join("\n")

test("a shell suite reads the patched source when bytecode was written a moment earlier", { timeout: 60_000, skip: missingPytest() }, () => {
  assert.equal(Buffer.byteLength(fresh), Buffer.byteLength(mutated))
  const dir = mkdtempSync(path.join(tmpdir(), "probatio-shell-pyc-"))
  const repo = path.join(dir, "repo")
  const bin = path.join(dir, "bin")
  mkdirSync(path.join(repo, "tests"), { recursive: true })
  mkdirSync(path.join(repo, "patches"), { recursive: true })
  mkdirSync(bin)
  writeFileSync(path.join(repo, "gate.py"), fresh)
  writeFileSync(
    path.join(repo, "tests", "test_gate.py"),
    ["from gate import open as gate_open", "def test_closed():", "    assert gate_open(0) is False", ""].join("\n"),
  )
  writeFileSync(path.join(repo, "patches", "m1.patch"), forwardDiff("gate.py", fresh, mutated))
  writeFileSync(path.join(repo, ".gitignore"), "__pycache__/\n*.pyc\n")
  const realGit = spawnSync("which", ["git"], { encoding: "utf8" }).stdout.trim()
  assert.ok(realGit, "git is required")
  writeFileSync(path.join(bin, "git"), wrapper(realGit))
  chmodSync(path.join(bin, "git"), 0o755)
  git(repo, ["init", "-q"])
  git(repo, ["add", "."])
  git(repo, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
  try {
    const result = spawnSync(
      tsx,
      [
        "src/cli.ts",
        "mutate",
        "run",
        "--package",
        repo,
        "--repo",
        repo,
        "--patches",
        path.join(repo, "patches"),
        "--out",
        path.join(repo, "out"),
        "--no-build",
        "--no-confirm",
        "--workers",
        "1",
        "--suite-timeout-ms",
        "60000",
        "--suite-command",
        "python3 -m pytest tests/test_gate.py -p no:cacheprovider --tb=line",
      ],
      {
        cwd: root,
        encoding: "utf8",
        env: { ...process.env, PATH: `${bin}${path.delimiter}${process.env.PATH ?? ""}` },
      },
    )
    assert.equal(result.status, 0, result.stderr + result.stdout)
    const body = JSON.parse(result.stdout) as { ok: boolean; summary: string; killed: number; survived: number }
    assert.equal(body.ok, true, body.summary)
    assert.equal(body.killed, 1, body.summary)
    assert.equal(body.survived, 0, body.summary)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

function wrapper(realGit: string): string {
  return [
    "#!/bin/sh",
    "if [ \"$1\" = \"-C\" ]; then",
    "  repo=$2",
    "  shift 2",
    `  "${realGit}" -C "$repo" "$@"`,
    "  status=$?",
    "  if [ $status -eq 0 ] && [ \"$1\" = \"apply\" ]; then",
    "    python3 - \"$repo\" << 'PY'",
    "import os, pathlib, struct, sys",
    "root = pathlib.Path(sys.argv[1])",
    "for pyc in root.rglob('*.pyc'):",
    "    if '.git' in pyc.parts:",
    "        continue",
    "    data = pyc.read_bytes()",
    "    if len(data) < 16:",
    "        continue",
    "    flags = struct.unpack_from('<I', data, 4)[0]",
    "    if flags & 1:",
    "        continue",
    "    mtime = struct.unpack_from('<I', data, 8)[0]",
    "    size = struct.unpack_from('<I', data, 12)[0]",
    "    if pyc.parent.name == '__pycache__':",
    "        src = pyc.parent.parent / (pyc.name.split('.')[0] + '.py')",
    "    else:",
    "        src = pyc.with_suffix('.py')",
    "    if src.is_file() and src.stat().st_size == size:",
    "        os.utime(src, (mtime, mtime))",
    "PY",
    "  fi",
    "  exit $status",
    "fi",
    `exec "${realGit}" "$@"`,
    "",
  ].join("\n")
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}
