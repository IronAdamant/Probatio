#!/bin/sh
# Speak MCP on stdin. Cargo's status stays on stderr, so the JSON-RPC stream stays one object per line.
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd) || exit 1
cd "$ROOT" || exit 1
exec cargo run --quiet --offline --manifest-path "$ROOT/Cargo.toml" -- mcp
