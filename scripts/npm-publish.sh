#!/usr/bin/env bash
# Publish the platform packages, then the main probatio package.
# A missing NPM_TOKEN skips. Release assets are copied in first.
# PROBATIO_ASSETS defaults to ./assets, where the release workflow downloads them.
# PROBATIO_TAG is the tag to check (v0.5.0). A tag push falls back to GITHUB_REF_NAME.
# NPM_PUBLISH_DRY_RUN=1 packs and prints the publish plan without uploading.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

dry=0
if [ "${NPM_PUBLISH_DRY_RUN:-}" = "1" ]; then
  dry=1
fi

if [ "$dry" -eq 0 ] && [ -z "${NPM_TOKEN:-}" ]; then
  echo "NPM_TOKEN is not set. Skipping npm publish."
  exit 0
fi
if [ -n "${NPM_TOKEN:-}" ]; then
  export NODE_AUTH_TOKEN="${NODE_AUTH_TOKEN:-$NPM_TOKEN}"
fi

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

ref="${PROBATIO_TAG:-}"
if [ -z "$ref" ]; then
  ref="${GITHUB_REF_NAME:-}"
fi
if [ -n "$ref" ]; then
  tag=${ref#v}
  if [ "$tag" != "$version" ]; then
    echo "Tag ${ref} does not match package version ${version}." >&2
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
if [ "$dry" -eq 1 ]; then
  publish_args+=(--dry-run)
  echo "Dry run. Nothing will be published."
elif npm publish --help 2>&1 | grep -q -- '--provenance'; then
  publish_args+=(--provenance)
  echo "Publishing with --provenance."
else
  echo "This npm does not support --provenance. Publishing without it."
fi

# Absolute paths. `npm publish npm/linux-x64` is GitHub shorthand for
# github.com/npm/linux-x64, not this directory.
# npm/win32-x64 publishes as @iron_adamant/probatio-win32-x64 (--access public).
for dir in npm/linux-x64 npm/darwin-arm64 npm/darwin-x64 npm/win32-x64 npm; do
  pkg_dir="$root/$dir"
  name=$(node -p "require(process.argv[1]).name" "$pkg_dir/package.json")
  if npm view "${name}@${version}" version >/dev/null 2>&1; then
    echo "${name}@${version} is already on npm. Skipping."
    continue
  fi
  echo "Publishing ${name}@${version} from ${pkg_dir}"
  npm publish "${publish_args[@]}" "$pkg_dir"
done
