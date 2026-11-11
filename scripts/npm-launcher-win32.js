#!/usr/bin/env node
"use strict";

// Prove the launcher resolves @iron_adamant/probatio-win32-x64 for win32,
// including on a machine that is not Windows.
const fs = require("fs");
const os = require("os");
const path = require("path");

const repo = path.resolve(__dirname, "..");
const cargoVersion = (fs.readFileSync(path.join(repo, "Cargo.toml"), "utf8").match(/^version = "([^"]+)"/m) || [])[1];
if (!cargoVersion) {
  process.stderr.write("Cargo.toml has no version\n");
  process.exit(1);
}
const launcherPath = path.join(repo, "npm", "bin", "probatio.js");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "probatio-win32-"));
process.on("exit", () => fs.rmSync(tmp, { recursive: true, force: true }));
const scopeDir = path.join(tmp, "node_modules", "@iron_adamant", "probatio-win32-x64");
const binDir = path.join(scopeDir, "bin");
fs.mkdirSync(binDir, { recursive: true });
fs.writeFileSync(
  path.join(scopeDir, "package.json"),
  JSON.stringify({ name: "@iron_adamant/probatio-win32-x64", version: cargoVersion }) + "\n"
);
const exe = path.join(binDir, "probatio.exe");
fs.writeFileSync(exe, "windows-binary");

const launcherHome = path.join(tmp, "node_modules", "probatio", "bin");
fs.mkdirSync(launcherHome, { recursive: true });
fs.copyFileSync(launcherPath, path.join(launcherHome, "probatio.js"));

const launcher = require(path.join(launcherHome, "probatio.js"));
if (launcher.PACKAGES["win32-x64"] !== "@iron_adamant/probatio-win32-x64") {
  process.stderr.write("win32 package name is " + launcher.PACKAGES["win32-x64"] + "\n");
  process.exit(1);
}
const resolved = launcher.binaryPath("win32", "x64");
if (path.resolve(resolved) !== path.resolve(exe)) {
  process.stderr.write("resolved " + resolved + "\nexpected " + exe + "\n");
  process.exit(1);
}
if (fs.readFileSync(resolved, "utf8") !== "windows-binary") {
  process.stderr.write("resolved file is not the Windows binary\n");
  process.exit(1);
}
process.stdout.write("launcher resolved " + resolved + "\n");
