#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
checker="$repo_root/scripts/check-bundle.sh"
temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT
dist="$temp_dir/dist"
report="$temp_dir/report.json"
mkdir -p "$dist/assets"
printf '<!doctype html><title>Kosh</title>\n' >"$dist/index.html"
printf 'console.log("production");\n' >"$dist/assets/main.js"

KOSH_BUNDLE_ROOT="$dist" KOSH_BUNDLE_REPORT="$report" "$checker" >/dev/null
jq -e '.result == "pass" and .fileCount == 2' "$report" >/dev/null

printf 'window.__KOSH_FAKE_BACKEND__ = {};\n' >"$dist/assets/main.js"
if KOSH_BUNDLE_ROOT="$dist" KOSH_BUNDLE_REPORT="$report" "$checker" >/dev/null 2>&1; then
  echo "bundle safety accepted a fake-backend marker" >&2
  exit 1
fi

printf 'console.log("production");\n' >"$dist/assets/main.js"
ln -s "$dist/assets/main.js" "$dist/assets/linked.js"
if KOSH_BUNDLE_ROOT="$dist" KOSH_BUNDLE_REPORT="$report" "$checker" >/dev/null 2>&1; then
  echo "bundle safety accepted a symlink" >&2
  exit 1
fi

echo "bundle safety tests passed"
