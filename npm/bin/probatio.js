#!/usr/bin/env node
"use strict";

const childProcess = require("child_process");
const fs = require("fs");
const path = require("path");

const PACKAGES = {
  "darwin-arm64": "probatio-darwin-arm64",
  "darwin-x64": "probatio-darwin-x64",
  "linux-x64": "probatio-linux-x64",
  "win32-x64": "probatio-win32-x64",
};

function fail(message) {
  process.stderr.write("probatio: " + message + "\n");
  process.exit(1);
}

function binaryPath() {
  const key = process.platform + "-" + process.arch;
  const name = PACKAGES[key];
  if (!name) {
    fail("no binary for " + key + ". This install ships darwin-arm64, darwin-x64, linux-x64, and win32-x64.");
  }
  const file = process.platform === "win32" ? "probatio.exe" : "probatio";
  let root;
  try {
    root = path.dirname(require.resolve(name + "/package.json"));
  } catch (_error) {
    fail("optional package " + name + " is not installed.");
  }
  const binary = path.join(root, "bin", file);
  if (!fs.existsSync(binary)) {
    fail("missing binary " + binary);
  }
  return binary;
}

const child = childProcess.spawn(binaryPath(), process.argv.slice(2), { stdio: "inherit" });

child.on("error", function (error) {
  fail("could not start the binary: " + error.message);
});

child.on("exit", function (code) {
  process.exit(code === null ? 1 : code);
});
