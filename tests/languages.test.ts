import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const tsx = path.join(root, "node_modules", ".bin", "tsx")

const dotnetMajor = dotnetTfm()

for (const item of fixtures()) {
  test(`mutate run scores ${item.language}`, { timeout: 180_000 }, () => {
    const dir = mkdtempSync(path.join(tmpdir(), `probatio-${item.language}-`))
    try {
      item.write(dir)
      git(dir, ["init", "-q"])
      git(dir, ["add", "."])
      git(dir, ["-c", "user.email=probatio@example.com", "-c", "user.name=probatio", "commit", "-qm", "init"])
      const generated = launch(dir, [
        "mutate",
        "generate",
        "--package",
        dir,
        "--src",
        ".",
        "--out",
        path.join(dir, "generated"),
        "--max-mutants",
        "4",
        "--max-minutes",
        "1",
      ])
      const generateBody = JSON.parse(generated.stdout) as { ok: boolean; mutants?: Array<{ file: string }> }
      assert.equal(generateBody.ok, true, generated.stderr + generated.stdout)
      const files = generateBody.mutants ?? []
      assert.ok(files.some((mutant) => item.extension(mutant.file)), generated.stdout)
      const first = score(dir, item.command)
      const second = score(dir, item.command)
      assert.equal(first.killed, second.killed, `${first.summary} vs ${second.summary}`)
      assert.equal(first.survived, second.survived, `${first.summary} vs ${second.summary}`)
      assert.match(first.summary, new RegExp(`\\b${first.killed} killed\\b`))
      assert.match(first.summary, new RegExp(`\\b${first.survived} survived\\b`))
      assert.ok(first.killed >= 1, first.summary)
      assert.equal(first.stdout.includes("Traceback"), false, first.stdout)
      const fixtureDir = process.env.PROBATIO_FIXTURE_OUT
      const fixtureLanguages = new Set(["cobol", "cpp", "csharp", "asm-x86_64", "asm-aarch64", "asm-riscv"])
      if (fixtureDir && fixtureLanguages.has(item.language)) {
        writeFileSync(path.join(fixtureDir, `${item.language}-fixture-run.json`), `${first.stdout.trim()}\n`)
      }
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
}

type Fixture = {
  language: string
  extension: (file: string) => boolean
  write: (dir: string) => void
  /** One-file layouts have no proved runner. The command is the suite. */
  command?: string
}

function fixtures(): Fixture[] {
  return [
    { language: "cobol", extension: (file) => file.endsWith(".cob"), write: writeCobol, command: "cobc -x -free -o tests/gate tests/gate.cob && tests/gate" },
    { language: "rust", extension: (file) => file.endsWith(".rs"), write: writeRust },
    { language: "c", extension: (file) => file.endsWith(".c"), write: writeC, command: "clang -std=c11 -o tests/gate tests/main.c && tests/gate" },
    { language: "cpp", extension: (file) => file.endsWith(".cpp"), write: writeCpp, command: "clang++ -std=c++17 -o tests/gate tests/main.cpp && tests/gate" },
    { language: "java", extension: (file) => file.endsWith(".java"), write: writeJava, command: "javac -d tests tests/Main.java && java -cp tests Main" },
    { language: "go", extension: (file) => file.endsWith(".go"), write: writeGo },
    { language: "python", extension: (file) => file.endsWith(".py"), write: writePython },
    { language: "javascript", extension: (file) => file.endsWith(".js"), write: writeJavaScript },
    { language: "typescript", extension: (file) => file.endsWith(".ts"), write: writeTypeScript },
    { language: "csharp", extension: (file) => file.endsWith(".cs"), write: writeCSharp },
    { language: "swift", extension: (file) => file.endsWith(".swift"), write: writeSwift, command: "swiftc -o tests/gate tests/main.swift && tests/gate" },
    {
      language: "asm-x86_64",
      extension: (file) => file.endsWith(".asm"),
      write: writeX86,
      command: "nasm -f macho64 -o tests/gate.o tests/gate.asm && clang -arch x86_64 -o tests/gate tests/gate.o && arch -x86_64 tests/gate",
    },
    { language: "asm-aarch64", extension: (file) => file.endsWith(".s"), write: writeArm, command: "clang -arch arm64 -o tests/gate tests/gate.s && tests/gate" },
    { language: "asm-riscv", extension: (file) => file.endsWith(".S"), write: writeRiscv, command: "bash tests/run.sh" },
  ]
}

function writeCobol(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(
    path.join(dir, "tests", "gate.cob"),
    [
      ">>SOURCE FORMAT FREE",
      "IDENTIFICATION DIVISION.",
      "PROGRAM-ID. GATE.",
      "DATA DIVISION.",
      "WORKING-STORAGE SECTION.",
      "01 N PIC 9 VALUE 0.",
      "PROCEDURE DIVISION.",
      "MAIN-PARA.",
      "    IF N > 0",
      "        MOVE 1 TO RETURN-CODE",
      "    ELSE",
      "        MOVE 0 TO RETURN-CODE",
      "    END-IF",
      "    STOP RUN.",
      "",
    ].join("\n"),
  )
}

function writeRust(dir: string): void {
  mkdirSync(path.join(dir, "src"))
  writeFileSync(path.join(dir, "Cargo.toml"), '[package]\nname = "gate"\nversion = "0.1.0"\nedition = "2021"\n')
  writeFileSync(
    path.join(dir, "src", "lib.rs"),
    [
      "pub fn gate(n: i32) -> bool { n > 0 && n < 10 }",
      "#[cfg(test)]",
      "mod tests {",
      "    use super::*;",
      "    #[test]",
      "    fn low() { assert!(!gate(0)); }",
      "    #[test]",
      "    fn inside() { assert!(gate(1)); }",
      "}",
      "",
    ].join("\n"),
  )
}

function writeC(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "tests", "main.c"), gateC())
}

function writeCpp(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "tests", "main.cpp"), gateC())
}

function writeJava(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(
    path.join(dir, "tests", "Main.java"),
    [
      "public class Main {",
      "  static boolean gate(int n) { return n > 0 && n < 10; }",
      "  public static void main(String[] args) {",
      "    if (gate(0)) System.exit(1);",
      "    if (!gate(1)) System.exit(1);",
      "    System.exit(0);",
      "  }",
      "}",
      "",
    ].join("\n"),
  )
}

function writeGo(dir: string): void {
  writeFileSync(path.join(dir, "go.mod"), "module example.com/gate\n\ngo 1.22\n")
  writeFileSync(path.join(dir, "gate.go"), "package gate\n\nfunc Gate(n int) bool { return n > 0 && n < 10 }\n")
  writeFileSync(
    path.join(dir, "gate_test.go"),
    [
      "package gate",
      "",
      'import "testing"',
      "",
      "func TestLow(t *testing.T) {",
      '  if Gate(0) { t.Fatal("low") }',
      "}",
      "func TestInside(t *testing.T) {",
      '  if !Gate(1) { t.Fatal("inside") }',
      "}",
      "",
    ].join("\n"),
  )
}

function writePython(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(
    path.join(dir, "tests", "test_gate.py"),
    [
      "import pytest",
      "",
      "def gate(n):",
      "    return n > 0 and n < 10",
      "",
      "def test_low():",
      "    assert gate(0) is False",
      "",
      "def test_inside():",
      "    assert gate(1) is True",
      "",
    ].join("\n"),
  )
}

function writeJavaScript(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "tests", "gate.test.js"), gateNode("js"))
}

function writeTypeScript(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "package.json"), '{ "type": "module" }\n')
  writeFileSync(path.join(dir, "tests", "gate.test.ts"), gateNode("ts"))
}

function writeCSharp(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(
    path.join(dir, "tests", "Gate.csproj"),
    [
      "<Project Sdk=\"Microsoft.NET.Sdk\">",
      "  <PropertyGroup>",
      "    <OutputType>Exe</OutputType>",
      `    <TargetFramework>net${dotnetMajor}.0</TargetFramework>`,
      "    <ImplicitUsings>disable</ImplicitUsings>",
      "    <Nullable>disable</Nullable>",
      "  </PropertyGroup>",
      "</Project>",
      "",
    ].join("\n"),
  )
  writeFileSync(path.join(dir, "tests", "Program.cs"), "int n = 0;\nif (n > 0) return 1;\nreturn 0;\n")
}

function writeSwift(dir: string): void {
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(
    path.join(dir, "tests", "main.swift"),
    ["import Darwin", "let n = 0", "if n > 0 {", "  exit(1)", "}", "exit(0)", ""].join("\n"),
  )
}

function writeX86(dir: string): void {
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "src", "decoy.c"), "int decoy(int n) { return n > 0; }\n")
  writeFileSync(
    path.join(dir, "tests", "gate.asm"),
    [
      "global _main",
      "section .text",
      "_main:",
      "    mov eax, 1",
      "    cmp eax, 1",
      "    je good",
      "    mov edi, 1",
      "    jmp quit",
      "good:",
      "    xor edi, edi",
      "quit:",
      "    mov eax, 0x2000001",
      "    syscall",
      "",
    ].join("\n"),
  )
}

function writeArm(dir: string): void {
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "src", "decoy.c"), "int decoy(int n) { return n > 0; }\n")
  writeFileSync(
    path.join(dir, "tests", "gate.s"),
    [
      ".global _main",
      ".align 2",
      "_main:",
      "    mov x0, #1",
      "    cmp x0, #1",
      "    b.eq good",
      "    mov x0, #1",
      "    b quit",
      "good:",
      "    mov x0, #0",
      "quit:",
      "    mov x16, #1",
      "    svc #0",
      "",
    ].join("\n"),
  )
}

function writeRiscv(dir: string): void {
  mkdirSync(path.join(dir, "src"))
  mkdirSync(path.join(dir, "tests"))
  writeFileSync(path.join(dir, "src", "decoy.c"), "int decoy(int n) { return n > 0; }\n")
  writeFileSync(
    path.join(dir, "tests", "gate.S"),
    [
      ".section .text",
      ".global _start",
      "_start:",
      "    li a0, 1",
      "    li t0, 1",
      "    beq a0, t0, good",
      "    li a0, 1",
      "    j quit",
      "good:",
      "    li a0, 0",
      "quit:",
      "    slli a0, a0, 16",
      "    li t1, 0x5555",
      "    or a0, a0, t1",
      "    li t2, 0x100000",
      "    sw a0, 0(t2)",
      "spin:",
      "    j spin",
      "",
    ].join("\n"),
  )
  writeFileSync(
    path.join(dir, "tests", "run.sh"),
    [
      "#!/bin/bash",
      "set -euo pipefail",
      "cd \"$(dirname \"$0\")/..\"",
      "mkdir -p .probatio-suite",
      "as=riscv64-elf-as",
      "ld=riscv64-elf-ld",
      "command -v \"$as\" >/dev/null",
      "\"$as\" -o .probatio-suite/suite.o tests/gate.S",
      "cat > .probatio-suite/link.ld <<'EOF'",
      "OUTPUT_ARCH(riscv)",
      "ENTRY(_start)",
      "SECTIONS",
      "{",
      "  . = 0x80000000;",
      "  .text : { *(.text*) }",
      "}",
      "EOF",
      "\"$ld\" -nostdlib -T .probatio-suite/link.ld -o .probatio-suite/suite .probatio-suite/suite.o",
      "if command -v qemu-riscv64 >/dev/null 2>&1; then exec qemu-riscv64 .probatio-suite/suite; fi",
      "if command -v qemu-riscv64-static >/dev/null 2>&1; then exec qemu-riscv64-static .probatio-suite/suite; fi",
      "exec qemu-system-riscv64 -machine virt -nographic -bios none -kernel .probatio-suite/suite -serial none -monitor none -display none",
      "",
    ].join("\n"),
  )
}

function gateC(): string {
  return ["int gate(int n) { return n > 0 && n < 10; }", "int main(void) {", "  if (gate(0)) return 1;", "  if (!gate(1)) return 1;", "  return 0;", "}", ""].join("\n")
}

function gateNode(kind: "js" | "ts"): string {
  const head = kind === "ts" ? "function gate(n: number): boolean { return n > 0 && n < 10 }\n" : "function gate(n) { return n > 0 && n < 10 }\n"
  return [
    head.trimEnd(),
    'import test from "node:test"',
    'import assert from "node:assert/strict"',
    'test("low", () => { assert.equal(gate(0), false) })',
    'test("inside", () => { assert.equal(gate(1), true) })',
    "",
  ].join("\n")
}

function score(dir: string, command?: string): { killed: number; survived: number; summary: string; stdout: string } {
  const out = mkdtempSync(path.join(dir, "run-"))
  const result = launch(dir, [
    "mutate",
    "run",
    "--package",
    dir,
    "--repo",
    dir,
    "--patches",
    path.join(dir, "generated", "mutants"),
    "--out",
    out,
    "--no-build",
    "--no-confirm",
    "--workers",
    "1",
    "--max-mutants",
    "4",
    "--max-minutes",
    "1",
    "--suite-timeout-ms",
    "120000",
    ...(command ? ["--suite-command", command] : []),
  ])
  const body = JSON.parse(result.stdout) as { ok: boolean; killed: number; survived: number; summary: string }
  assert.equal(body.ok, true, result.stderr + result.stdout)
  assert.equal(typeof body.killed, "number")
  assert.equal(typeof body.survived, "number")
  for (const refusal of ["no tests in tests", "no patches", "did not return a test report", "duplicate test title"]) {
    assert.equal(body.summary.includes(refusal), false, body.summary)
  }
  return { killed: body.killed, survived: body.survived, summary: body.summary, stdout: result.stdout }
}

function launch(dir: string, args: string[]) {
  const result = spawnSync(tsx, ["src/cli.ts", ...args], { cwd: root, encoding: "utf8" })
  assert.equal(result.status === 0 || result.stdout.trim().startsWith("{"), true, result.stderr + result.stdout)
  return result
}

function git(repo: string, args: string[]) {
  const result = spawnSync("git", ["-C", repo, ...args], { encoding: "utf8" })
  if (result.status !== 0) throw new Error(result.stderr || result.stdout)
}

function dotnetTfm(): string {
  const result = spawnSync("dotnet", ["--version"], { encoding: "utf8" })
  const major = result.status === 0 ? result.stdout.trim().split(".")[0] : "8"
  return major || "8"
}
