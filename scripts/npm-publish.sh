#!/usr/bin/env bash
# Publish the platform packages, then the main probatio package.
# A missing NPM_TOKEN skips. Release assets are copied in first.
# PROBATIO_ASSETS defaults to ./assets, where the release workflow downloads them.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

if [ -z "${NPM_TOKEN:-}" ]; then
  echo "NPM_TOKEN is not set. Skipping npm publish."
  exit 0
fi
export NODE_AUTH_TOKEN="${NODE_AUTH_TOKEN:-$NPM_TOKEN}"

version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
if [ -z "$version" ]; then
  echo "Cargo.toml has no version" >&2
  exit 1
fi

check_version() {
  local file="$1"
  local got
  got=$(node -p "require(process.argv[1]).version" "$root/$file")
  if [ "$got" != "$version" ]; then
    echo "$file is $got and Cargo.toml is $version" >&2
    exit 1
  fi
}

check_version npm/package.json
for name in linux-x64 darwin-arm64 darwin-x64 win32-x64; do
  check_version "npm/$name/package.json"
done

if [ -n "${GITHUB_REF_NAME:-}" ]; then
  tag=${GITHUB_REF_NAME#v}
  if [ "$tag" != "$version" ]; then
    echo "Tag ${GITHUB_REF_NAME} does not match package version ${version}." >&2
    exit 1
  fi
fi

assets=${PROBATIO_ASSETS:-assets}
stage_bin() {
  local src="$1"
  local dest="$2"
  if [ ! -s "$src" ]; then
    echo "Missing release asset $src" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$dest")"
  cp "$src" "$dest"
  chmod +x "$dest"
}

stage_bin "$assets/probatio-x86_64-unknown-linux-musl" npm/linux-x64/bin/probatio
stage_bin "$assets/probatio-aarch64-apple-darwin" npm/darwin-arm64/bin/probatio
stage_bin "$assets/probatio-x86_64-apple-darwin" npm/darwin-x64/bin/probatio
stage_bin "$assets/probatio-x86_64-pc-windows-msvc.exe" npm/win32-x64/bin/probatio.exe

publish_args=(--access public)
if npm publish --help 2>&1 | grep -q -- '--provenance'; then
  publish_args+=(--provenance)
  echo "Publishing with --provenance."
else
  echo "This npm does not support --provenance. Publishing without it."
fi

for dir in npm/linux-x64 npm/darwin-arm64 npm/darwin-x64 npm/win32-x64 npm; do
  echo "Publishing $(node -p "require(process.argv[1]).name" "$root/$dir/package.json")@${version}"
  npm publish "$dir" "${publish_args[@]}"
done
