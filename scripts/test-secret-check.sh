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
git -C "$temp_dir" commit --quiet -m baseline
base_commit="$(git -C "$temp_dir" rev-parse HEAD)"
(
  cd "$temp_dir"
  KOSH_DIFF_BASE="$base_commit" scripts/check-secrets.sh >/dev/null
)

printf '%s=%s\n%s=\n' \
  "$access_key_name" \
  '0123456789abcdef0123456789abcdef' \
  "$secret_key_name" \
  >"$temp_dir/.env.example"
git -C "$temp_dir" add .env.example
if (
  cd "$temp_dir"
  KOSH_DIFF_BASE='' scripts/check-secrets.sh >/dev/null 2>&1
); then
  echo "secret checker accepted a populated .env.example credential" >&2
  exit 1
fi

git -C "$temp_dir" restore --source=HEAD --staged --worktree .env.example
readonly fine_grained_prefix="github_pat_"
printf '%s%s\n' \
  "$fine_grained_prefix" \
  '11AA22BB33CC44DD55EE66FF77GG88HH99II00JJ' \
  >"$temp_dir/credentials.txt"
git -C "$temp_dir" add .env.example credentials.txt
if (
  cd "$temp_dir"
  KOSH_DIFF_BASE='' scripts/check-secrets.sh >/dev/null 2>&1
); then
  echo "secret checker accepted a fine-grained GitHub token" >&2
  exit 1
fi

git -C "$temp_dir" restore --staged credentials.txt
rm "$temp_dir/credentials.txt"

printf '%s%s\n' \
  "$fine_grained_prefix" \
  'AA11BB22CC33DD44EE55FF66GG77HH88II99JJ00' \
  >"$temp_dir/past-credential.txt"
git -C "$temp_dir" add past-credential.txt
git -C "$temp_dir" commit --quiet -m 'add past credential'
git -C "$temp_dir" rm --quiet past-credential.txt
git -C "$temp_dir" commit --quiet -m 'remove past credential'
if (
  cd "$temp_dir"
  KOSH_DIFF_BASE="$base_commit" scripts/check-secrets.sh >/dev/null 2>&1
); then
  echo "secret checker accepted a credential removed later in history" >&2
  exit 1
fi

echo "secret checker tests passed"
