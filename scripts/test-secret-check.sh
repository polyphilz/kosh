#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
checker="$repo_root/scripts/check-secrets.sh"
temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT

git -C "$temp_dir" init --quiet
git -C "$temp_dir" config user.name "Kosh Tests"
git -C "$temp_dir" config user.email "tests@kosh.invalid"
mkdir -p "$temp_dir/scripts"
cp "$checker" "$temp_dir/scripts/check-secrets.sh"
chmod +x "$temp_dir/scripts/check-secrets.sh"

readonly access_key_name="KOSH_R2_ACCESS_KEY_ID"
readonly secret_key_name="KOSH_R2_SECRET_ACCESS_KEY"
printf '%s=\n%s=\n' \
  "$access_key_name" \
  "$secret_key_name" \
  >"$temp_dir/.env.example"
git -C "$temp_dir" add .env.example scripts/check-secrets.sh
(
  cd "$temp_dir"
  scripts/check-secrets.sh >/dev/null
)

printf '%s=%s\n%s=\n' \
  "$access_key_name" \
  '0123456789abcdef0123456789abcdef' \
  "$secret_key_name" \
  >"$temp_dir/.env.example"
git -C "$temp_dir" add .env.example
if (
  cd "$temp_dir"
  scripts/check-secrets.sh >/dev/null 2>&1
); then
  echo "secret checker accepted a populated .env.example credential" >&2
  exit 1
fi

echo "secret checker tests passed"
