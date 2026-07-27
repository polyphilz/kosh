#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

secret_pattern='(cfat_[A-Za-z0-9_-]{20,}|gh[opsu]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}|-----BEGIN ([A-Z ]+ )?PRIVATE KEY-----|KOSH_R2_(ACCESS_KEY_ID|SECRET_ACCESS_KEY)[[:space:]]*=[[:space:]]*[^[:space:]<]+)'

find_env_paths() {
  awk '
    /(^|\/)\.env($|\.)/ && $0 !~ /(^|\/)\.env\.example$/ {
      print
    }
  '
}

fail_for_env_paths() {
  local label="$1"
  local paths="$2"
  if [[ -n "$paths" ]]; then
    echo "tracked environment files are forbidden in $label:" >&2
    echo "$paths" >&2
    exit 1
  fi
}

fail_for_secret_paths() {
  local label="$1"
  local paths="$2"
  if [[ -n "$paths" ]]; then
    echo "possible secret material detected in $label:" >&2
    echo "$paths" >&2
    exit 1
  fi
}

index_env="$(git ls-files | find_env_paths)"
fail_for_env_paths "the index" "$index_env"

worktree_secret_paths="$(git grep -l -I -E "$secret_pattern" -- . || true)"
fail_for_secret_paths "the worktree" "$worktree_secret_paths"

index_secret_paths="$(git grep --cached -l -I -E "$secret_pattern" -- . || true)"
fail_for_secret_paths "the index" "$index_secret_paths"

if [[ -n "${KOSH_DIFF_BASE:-}" && ! "$KOSH_DIFF_BASE" =~ ^0+$ ]]; then
  git rev-parse --verify "$KOSH_DIFF_BASE^{commit}" >/dev/null

  while IFS= read -r commit; do
    commit_env="$(
      git ls-tree -r --name-only "$commit" |
        find_env_paths
    )"
    fail_for_env_paths "commit $commit" "$commit_env"

    commit_secret_paths="$(
      git grep -l -I -E "$secret_pattern" "$commit" -- . || true
    )"
    fail_for_secret_paths "commit $commit" "$commit_secret_paths"
  done < <(git rev-list --reverse "$KOSH_DIFF_BASE..HEAD")
fi

echo "secret checks passed"
