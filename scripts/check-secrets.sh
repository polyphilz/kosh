#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

tracked_env="$(
  git ls-files |
    awk '
      /(^|\/)\.env($|\.)/ && $0 !~ /(^|\/)\.env\.example$/ {
        print
      }
    '
)"
if [[ -n "$tracked_env" ]]; then
  echo "tracked environment files are forbidden:" >&2
  echo "$tracked_env" >&2
  exit 1
fi

secret_pattern='(cfat_[A-Za-z0-9_-]{20,}|gh[opsu]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}|-----BEGIN ([A-Z ]+ )?PRIVATE KEY-----|KOSH_R2_(ACCESS_KEY_ID|SECRET_ACCESS_KEY)[[:space:]]*=[[:space:]]*[^[:space:]<]+)'
matches="$(git grep -n -I -E "$secret_pattern" -- . || true)"
if [[ -n "$matches" ]]; then
  echo "possible committed secret material detected:" >&2
  echo "$matches" >&2
  exit 1
fi

echo "secret checks passed"
