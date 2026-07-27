#!/usr/bin/env bash
set -euo pipefail

if (($# < 1 || $# > 2)); then
  echo "usage: $0 <pull-request-number> [owner/repository]" >&2
  exit 2
fi

gh_bin="${GH_BIN:-gh}"
repo="${2:-$("$gh_bin" repo view --json nameWithOwner --jq '.nameWithOwner')}"
pr_number="$1"
head_sha="$(
  "$gh_bin" pr view "$pr_number" --repo "$repo" \
    --json headRefOid --jq '.headRefOid'
)"

"$(dirname "$0")/gate.sh" "$pr_number" "$repo"
"$gh_bin" pr merge "$pr_number" --repo "$repo" --squash --delete-branch \
  --match-head-commit "$head_sha"
