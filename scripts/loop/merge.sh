#!/usr/bin/env bash
set -euo pipefail

if (($# < 1 || $# > 2)); then
  echo "usage: $0 <pull-request-number> [owner/repository]" >&2
  exit 2
fi

repo="${2:-$(gh repo view --json nameWithOwner --jq '.nameWithOwner')}"
pr_number="$1"

"$(dirname "$0")/gate.sh" "$pr_number" "$repo"
gh pr merge "$pr_number" --repo "$repo" --squash --delete-branch
