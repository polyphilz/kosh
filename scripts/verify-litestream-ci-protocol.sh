#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
APP_ROOT="$ROOT/app"
PIN="$APP_ROOT/src-tauri/resources/sidecars/litestream-v1.json"
architecture=$(uname -m)

main() {
for command in curl file jq shasum tar uuidgen; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "required command not found: $command" >&2
    exit 1
  }
done
test "$(uname -s)" = Darwin || {
  echo "the Litestream CI protocol verifier requires macOS" >&2
  exit 1
}
jq -er --arg architecture "$architecture" \
  '.target.architectures | index($architecture) != null' "$PIN" >/dev/null || {
  echo "unsupported CI architecture: $architecture" >&2
  exit 1
}

run_id=$(uuidgen | tr '[:upper:]' '[:lower:]')
run_root="$APP_ROOT/.data/litestream-ci/$run_id"
extract_root="$run_root/extract"
mkdir -p "$extract_root"
chmod 700 "$run_root" "$extract_root"

checksums="$run_root/checksums.txt"
archive_name=$(jq -er --arg architecture "$architecture" \
  '.upstream.assets[$architecture].name' "$PIN")
archive="$run_root/$archive_name"
curl -fL --retry 3 --connect-timeout 15 \
  "$(jq -er '.upstream.checksums.url' "$PIN")" \
  -o "$checksums"
curl -fL --retry 3 --connect-timeout 15 \
  "$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].url' "$PIN")" \
  -o "$archive"

assert_file "$checksums" \
  "$(jq -er '.upstream.checksums.size' "$PIN")" \
  "$(jq -er '.upstream.checksums.sha256' "$PIN")" \
  "official checksums"
asset_size=$(jq -er --arg architecture "$architecture" \
  '.upstream.assets[$architecture].size' "$PIN")
asset_sha256=$(jq -er --arg architecture "$architecture" \
  '.upstream.assets[$architecture].sha256' "$PIN")
assert_file "$archive" "$asset_size" "$asset_sha256" "$architecture archive"
grep -Fx "$asset_sha256  $archive_name" "$checksums" >/dev/null

binary_path=$(jq -er '.binary.archivePath' "$PIN")
tar -xzf "$archive" -C "$extract_root" "$binary_path"
binary="$extract_root/$binary_path"
chmod 755 "$binary"
assert_file "$binary" \
  "$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].binarySize' "$PIN")" \
  "$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].binarySha256' "$PIN")" \
  "$architecture binary"
file "$binary" | grep -q "Mach-O 64-bit executable $architecture"

KOSH_LITESTREAM_BINARY="$binary" \
  "$ROOT/scripts/verify-litestream-local-protocol.sh"

echo "Litestream CI protocol verification passed on $architecture"
}

assert_file() {
  path=$1
  expected_size=$2
  expected_sha256=$3
  label=$4

  actual_size=$(wc -c <"$path" | tr -d ' ')
  test "$actual_size" = "$expected_size" || {
    echo "$label size mismatch" >&2
    exit 1
  }
  actual_sha256=$(shasum -a 256 "$path" | awk '{print $1}')
  test "$actual_sha256" = "$expected_sha256" || {
    echo "$label SHA-256 mismatch" >&2
    exit 1
  }
}

main "$@"
