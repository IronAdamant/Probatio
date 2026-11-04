#!/usr/bin/env node
"use strict";

// Validate server.json against the schema URL it names, and against the
// package versions in this repo. This does not publish.

const { execFileSync } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const root = path.resolve(__dirname, "..");
const schemaPins = ["ajv@8.17.1", "ajv-formats@3.0.1"];
const banned = ["secure", "security", "compliant", "certified", "guaranteed", "audit", "hack"];

function fail(message) {
  process.stderr.write(message + "\n");
  process.exit(1);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function cargoVersion() {
  const text = fs.readFileSync(path.join(root, "Cargo.toml"), "utf8");
  const match = text.match(/^version = "([^"]+)"/m);
  if (!match) {
    fail("Cargo.toml has no version");
  }
  return match[1];
}

function valuesOf(list) {
  if (!Array.isArray(list)) {
    return [];
  }
  return list.map((item) => item && item.value).filter((value) => typeof value === "string");
}

function checkDescription(description) {
  if (typeof description !== "string" || description.length < 1 || description.length > 100) {
    fail("description must be 1 to 100 characters");
  }
  for (const char of description) {
    if (char.charCodeAt(0) > 127) {
      fail("description is not plain ASCII");
    }
  }
  if (description.includes("-")) {
    fail("description uses a dash as punctuation");
  }
  const words = description.toLowerCase().split(/[^a-z0-9]+/);
  for (const word of banned) {
    if (words.includes(word)) {
      fail("description contains " + word);
    }
  }
}

function checkListing(server, version) {
  const npmPackage = readJson(path.join(root, "npm", "package.json"));
  if (server.name !== "io.github.IronAdamant/probatio") {
    fail("server.json name is " + server.name);
  }
  if (npmPackage.mcpName !== server.name) {
    fail("npm mcpName is " + npmPackage.mcpName);
  }
  if (server.version !== version || npmPackage.version !== version) {
    fail("version is server " + server.version + ", npm " + npmPackage.version + ", crate " + version);
  }
  checkDescription(server.description);
  const repository = server.repository || {};
  if (repository.url !== "https://github.com/IronAdamant/Probatio" || repository.source !== "github") {
    fail("repository does not point at https://github.com/IronAdamant/Probatio");
  }
  const packages = server.packages;
  if (!Array.isArray(packages) || packages.length !== 1) {
    fail("server.json needs one package entry");
  }
  const entry = packages[0];
  if (entry.registryType !== "npm" || entry.identifier !== "probatio" || entry.version !== version) {
    fail("package entry must be npm probatio " + version);
  }
  if (!entry.transport || entry.transport.type !== "stdio") {
    fail("transport must be stdio");
  }
  if (entry.runtimeHint !== "npx" || !valuesOf(entry.runtimeArguments).includes("-y")) {
    fail("runtime must be npx -y");
  }
  if (!valuesOf(entry.packageArguments).includes("mcp")) {
    fail("package arguments must include mcp");
  }
  const platforms = ["linux-x64", "darwin-arm64", "darwin-x64", "win32-x64"];
  for (const name of platforms) {
    const platform = readJson(path.join(root, "npm", name, "package.json"));
    if (platform.version !== version) {
      fail(name + " is " + platform.version + " and the crate is " + version);
    }
  }
}

async function schemaFor(server) {
  if (typeof server.$schema !== "string" || server.$schema.length === 0) {
    fail("server.json has no $schema");
  }
  const response = await fetch(server.$schema);
  if (!response.ok) {
    fail("GET " + server.$schema + " returned " + response.status);
  }
  return response.json();
}

function compileValidator(schema) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "probatio-server-schema-"));
  execFileSync("npm", ["install", "--prefix", tmp, "--silent", "--no-fund", "--no-audit", ...schemaPins], {
    stdio: "inherit",
  });
  const Ajv = require(path.join(tmp, "node_modules", "ajv"));
  const addFormats = require(path.join(tmp, "node_modules", "ajv-formats"));
  const ajv = new Ajv({ strict: false, allErrors: true });
  addFormats(ajv);
  return ajv.compile(schema);
}

async function main() {
  const server = readJson(path.join(root, "server.json"));
  const version = cargoVersion();
  checkListing(server, version);
  const schema = await schemaFor(server);
  const validate = compileValidator(schema);
  if (!validate(server)) {
    fail(validate.errors.map((error) => (error.instancePath || "/") + " " + error.message).join("\n"));
  }
  process.stdout.write("server.json matches " + server.$schema + "\n");
}

main().catch((error) => {
  fail(error && error.stack ? error.stack : String(error));
});
