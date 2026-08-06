#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "bundle safety failed: $*" >&2
  exit 1
}

# The restricted BlockNote production editor deliberately carries its editor,
# Mantine, and math runtime. Keep explicit ceilings close to that measured
# production shape so later growth still fails loudly.
total_byte_budget=4500000
javascript_byte_budget=2900000
javascript_chunk_byte_budget=1750000

repo_root="$(git rev-parse --show-toplevel)"
dist_root="${KOSH_BUNDLE_ROOT:-$repo_root/app/dist}"
report="${KOSH_BUNDLE_REPORT:-$repo_root/app/test-results/bundle/report.json}"
[[ "$dist_root" == /* ]] || fail "bundle root must be absolute"
[[ -d "$dist_root" && ! -L "$dist_root" ]] || fail "production dist is missing or is a symlink"
if find "$dist_root" -type l -print -quit | grep -q .; then
  fail "bundle contains a symlink"
fi

files=()
while IFS= read -r -d '' file; do
  files+=("$file")
done < <(find "$dist_root" -type f -print0 | sort -z)
((${#files[@]} > 0)) || fail "production dist contains no files"

total_bytes=0
total_javascript_bytes=0
largest_javascript_bytes=0
largest_javascript_file=""
for file in "${files[@]}"; do
  [[ ! -L "$file" ]] || fail "bundle contains a symlink: ${file#"$dist_root"/}"
  relative="${file#"$dist_root"/}"
  case "$relative" in
    *.html | *.css | *.js | *.svg | *.png | *.webp | *.woff | *.woff2 | *.ttf) ;;
    *) fail "bundle contains an unexpected artifact: $relative" ;;
  esac
  bytes="$(wc -c <"$file" | tr -d '[:space:]')"
  total_bytes=$((total_bytes + bytes))
  if [[ "$file" == *.js ]]; then
    total_javascript_bytes=$((total_javascript_bytes + bytes))
    if ((bytes > largest_javascript_bytes)); then
      largest_javascript_bytes="$bytes"
      largest_javascript_file="$relative"
    fi
  fi
done

((total_bytes <= total_byte_budget)) ||
  fail "uncompressed bundle is $total_bytes bytes; release budget is $total_byte_budget"
((total_javascript_bytes <= javascript_byte_budget)) ||
  fail "JavaScript is $total_javascript_bytes bytes; release budget is $javascript_byte_budget"
((largest_javascript_bytes <= javascript_chunk_byte_budget)) ||
  fail "$largest_javascript_file is $largest_javascript_bytes bytes; chunk budget is $javascript_chunk_byte_budget"

for forbidden_path in \
  '.env' \
  '.sqlite3' \
  '.gguf' \
  '.onnx' \
  '.safetensors' \
  '.plans' \
  'test-results' \
  'tests/browser'; do
  if find "$dist_root" -type f -path "*$forbidden_path*" -print -quit | grep -q .; then
    fail "bundle contains forbidden path material matching $forbidden_path"
  fi
done

for forbidden_text in \
  '__KOSH_FAKE_BACKEND__' \
  'KOSH_FAKE_BACKEND' \
  '/tmp/kosh-browser-fixture' \
  'fixture-request-1' \
  'VITE_KOSH_BACKEND' \
  'tests/browser'; do
  if LC_ALL=C grep -R -I -F -q -- "$forbidden_text" "$dist_root"; then
    fail "bundle contains test-only marker: $forbidden_text"
  fi
done

mkdir -p "$(dirname "$report")"
temporary="$report.$$.tmp"
jq -n \
  --arg largestFile "$largest_javascript_file" \
  --argjson fileCount "${#files[@]}" \
  --argjson totalBytes "$total_bytes" \
  --argjson javascriptBytes "$total_javascript_bytes" \
  --argjson largestJavascriptBytes "$largest_javascript_bytes" \
  --argjson totalByteBudget "$total_byte_budget" \
  --argjson javascriptByteBudget "$javascript_byte_budget" \
  --argjson javascriptChunkByteBudget "$javascript_chunk_byte_budget" \
  '{
    schemaVersion: 1,
    result: "pass",
    fileCount: $fileCount,
    totalBytes: $totalBytes,
    javascriptBytes: $javascriptBytes,
    largestJavascript: {
      path: $largestFile,
      bytes: $largestJavascriptBytes
    },
    budgets: {
      totalBytes: $totalByteBudget,
      javascriptBytes: $javascriptByteBudget,
      javascriptChunkBytes: $javascriptChunkByteBudget
    }
  }' >"$temporary"
mv "$temporary" "$report"

echo "bundle safety passed: $total_bytes bytes total, $total_javascript_bytes bytes JavaScript"
