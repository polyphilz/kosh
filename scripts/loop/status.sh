#!/usr/bin/env bash
set -euo pipefail

repo="${1:-$(gh repo view --json nameWithOwner --jq '.nameWithOwner')}"
branch="$(git branch --show-current)"

echo "repository: $repo"
echo "branch: $branch"
git status --short --branch

pr_json="$(gh pr view "$branch" --repo "$repo" --json headRefOid,number,state,url 2>/dev/null || true)"
if [[ -z "$pr_json" ]]; then
  echo "pull request: none"
  exit 0
fi

pr_number="$(jq -r '.number' <<<"$pr_json")"
echo "pull request: $(jq -r '.url' <<<"$pr_json")"
echo "head: $(jq -r '.headRefOid' <<<"$pr_json")"
echo "state: $(jq -r '.state' <<<"$pr_json")"
echo "checks:"
gh pr checks "$pr_number" --repo "$repo" || true
echo "reactions:"
gh api \
  -H "Accept: application/vnd.github+json" \
  "repos/$repo/issues/$pr_number/reactions?per_page=100" \
  --jq '.[] | "\(.content) \(.user.login) \(.created_at)"'
