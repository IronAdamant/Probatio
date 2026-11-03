#!/usr/bin/env bash
# Build a host binary, then npm-pack the matching platform package and the
# main probatio package into npm/dist. --smoke installs those tarballs in a
# temp directory and runs `probatio --version`. This script does not publish.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
smoke=0
if [ "${1:-}" = "--smoke" ]; then
  smoke=1
elif [ -n "${1:-}" ]; then
  echo "usage: scripts/npm-pack-local.sh [--smoke]" >&2
  exit 2
fi

version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then
  echo "Cargo.toml has no version" >&2
  exit 1
fi

check_version() {
  local file="$1"
  local got
  got=$(node -p "require(process.argv[1]).version" "$file")
  if [ "$got" != "$version" ]; then
    echo "$file is $got and Cargo.toml is $version" >&2
    exit 1
  fi
}

check_version "$root/npm/package.json"
for name in linux-x64 darwin-arm64 darwin-x64 win32-x64; do
  check_version "$root/npm/$name/package.json"
done

case "$(uname -s):$(uname -m)" in
  Linux:x86_64) platform=linux-x64; exe=probatio ;;
  Darwin:arm64) platform=darwin-arm64; exe=probatio ;;
  Darwin:x86_64) platform=darwin-x64; exe=probatio ;;
  *)
    echo "No platform package for $(uname -s) $(uname -m)" >&2
    exit 1
    ;;
esac

if [ -n "${PROBATIO_BIN:-}" ]; then
  bin=$PROBATIO_BIN
  if [ ! -f "$bin" ]; then
    echo "PROBATIO_BIN does not exist: $bin" >&2
    exit 1
  fi
else
  cargo build --release --offline --manifest-path "$root/Cargo.toml"
  bin=$root/target/release/probatio
fi

stage=$root/npm/$platform/bin
mkdir -p "$stage"
cp "$bin" "$stage/$exe"
chmod +x "$stage/$exe"

dist=$root/npm/dist
mkdir -p "$dist"
rm -f "$dist"/*.tgz
(cd "$root/npm/$platform" && npm pack --pack-destination "$dist")
(cd "$root/npm" && npm pack --pack-destination "$dist")
echo "Packed tarballs in $dist"

if [ "$smoke" -ne 1 ]; then
  exit 0
fi

platform_tgz=$dist/probatio-${platform}-${version}.tgz
main_tgz=$dist/probatio-${version}.tgz
if [ ! -f "$platform_tgz" ] || [ ! -f "$main_tgz" ]; then
  echo "Expected $platform_tgz and $main_tgz" >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
npm install --no-fund --no-audit --prefix "$tmp" "$platform_tgz"
npm install --no-fund --no-audit --omit=optional --prefix "$tmp" "$main_tgz"
launcher=$tmp/node_modules/.bin/probatio
if [ ! -x "$launcher" ]; then
  echo "The probatio launcher was not installed at $launcher" >&2
  exit 1
fi

output=$(cd "$tmp" && "$launcher" --version)
printf '%s\n' "$output"
case "$output" in
  *'"command": "version"'*) ;;
  *)
    echo "probatio --version did not run the binary" >&2
    exit 1
    ;;
esac
case "$output" in
  *"\"$version\""*) ;;
  *)
    echo "probatio --version did not report $version" >&2
    exit 1
    ;;
esac
echo "npm launcher ran probatio $version"
