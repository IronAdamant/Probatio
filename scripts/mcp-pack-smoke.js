#!/usr/bin/env node
"use strict";

// From a temp install of a local npm pack, run `npx -y probatio mcp` and
// require an MCP initialize plus tools/list on stdio. The caller installs
// the main tarball and the matching platform tarball into prefix first.
// `npx -y probatio@<tarball>` starts a fresh tree and does not see that
// platform package. This does not publish.

const { spawn } = require("child_process");
const path = require("path");

const prefix = process.argv[2];
const pack = process.argv[3] || "";
const platformPack = process.argv[4] || "";
const expectedTools = ["mutate_diff", "mutate_generate", "mutate_run", "mutate_tally", "schema", "probatio"];

function fail(message) {
  process.stderr.write(message + "\n");
  process.exit(1);
}

if (!prefix) {
  fail("usage: scripts/mcp-pack-smoke.js <prefix> <pack.tgz>");
}

const installed = require(path.join(prefix, "node_modules", "probatio", "package.json"));
const version = installed.version;
const messages = [
  {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "probatio-pack", version: version },
    },
  },
  { jsonrpc: "2.0", method: "notifications/initialized" },
  { jsonrpc: "2.0", id: 2, method: "tools/list" },
];

process.stdout.write("local pack: " + pack + "\n");
process.stdout.write("platform pack: " + platformPack + "\n");
process.stdout.write("cwd: " + prefix + "\n");

const args = ["-y", "probatio", "mcp"];
process.stdout.write("command: npx " + args.join(" ") + "\n");
// npx is npx.cmd on Windows, and Node starts a .cmd shim only through a shell.
const child = spawn("npx", args, {
  shell: process.platform === "win32",
  cwd: prefix,
  stdio: ["pipe", "pipe", "pipe"],
  env: Object.assign({}, process.env, {
    npm_config_update_notifier: "false",
    NO_UPDATE_NOTIFIER: "1",
  }),
});

let stdout = "";
let stderr = "";
child.stdout.on("data", (chunk) => {
  stdout += chunk.toString("utf8");
});
child.stderr.on("data", (chunk) => {
  stderr += chunk.toString("utf8");
});

const timer = setTimeout(() => {
  child.kill("SIGKILL");
  fail("npx -y probatio mcp did not finish within 30s\n" + stderr);
}, 30000);

child.stdin.on("error", () => {});
child.stdin.write(messages.map((message) => JSON.stringify(message)).join("\n") + "\n");
child.stdin.end();

child.on("error", (error) => {
  clearTimeout(timer);
  fail("could not start npx: " + error.message);
});

child.on("exit", (code) => {
  clearTimeout(timer);
  process.stdout.write(stdout);
  if (stderr.trim()) {
    process.stderr.write(stderr);
  }
  if (code !== 0) {
    fail("npx -y probatio mcp exited " + code);
  }
  const replies = new Map();
  for (const line of stdout.split("\n")) {
    if (!line.startsWith("{")) {
      continue;
    }
    const message = JSON.parse(line);
    if (Object.prototype.hasOwnProperty.call(message, "id")) {
      replies.set(message.id, message);
    }
  }
  const init = replies.get(1);
  const listed = replies.get(2);
  const info = init && init.result && init.result.serverInfo;
  if (!info || info.name !== "probatio" || info.version !== version) {
    fail("initialize did not report probatio " + version);
  }
  if (!init.result || init.result.protocolVersion !== "2025-06-18") {
    fail("initialize did not accept protocol 2025-06-18");
  }
  const tools = listed && listed.result && listed.result.tools;
  const names = Array.isArray(tools) ? tools.map((tool) => tool.name) : [];
  if (names.join(",") !== expectedTools.join(",")) {
    fail("tools/list returned " + names.join(","));
  }
  process.stdout.write("mcp stdio ok\n");
});
