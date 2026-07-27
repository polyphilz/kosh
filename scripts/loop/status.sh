#!/usr/bin/env bash
set -euo pipefail

readonly review_bot="${KOSH_CODEX_REVIEW_BOT:-chatgpt-codex-connector[bot]}"
readonly gh_bin="${GH_BIN:-gh}"

repo="${1:-$("$gh_bin" repo view --json nameWithOwner --jq '.nameWithOwner')}"
branch="$(git branch --show-current)"

echo "repository: $repo"
echo "branch: $branch"
git status --short --branch

pr_json="$(
  "$gh_bin" pr view "$branch" --repo "$repo" \
    --json headRefOid,number,state,url 2>/dev/null || true
)"
if [[ -z "$pr_json" ]]; then
  echo "pull request: none"
  exit 0
fi

pr_number="$(jq -r '.number' <<<"$pr_json")"
head_sha="$(jq -r '.headRefOid' <<<"$pr_json")"
echo "pull request: $(jq -r '.url' <<<"$pr_json")"
echo "head: $head_sha"
echo "state: $(jq -r '.state' <<<"$pr_json")"
echo "checks:"
"$gh_bin" pr checks "$pr_number" --repo "$repo" || true

workflow_runs_json="$(
  "$gh_bin" api \
    --paginate \
    --slurp \
    -H "Accept: application/vnd.github+json" \
    "repos/$repo/actions/runs?head_sha=$head_sha&event=pull_request&per_page=100"
)"
head_observed_at="$(
  jq -r \
    --arg head "$head_sha" \
    '[
      .[]
      | .workflow_runs[]?
      | select(.head_sha == $head and .event == "pull_request")
      | .created_at
    ] | max // empty' \
    <<<"$workflow_runs_json"
)"
comments_json="$(
  "$gh_bin" api \
    --paginate \
    --slurp \
    -H "Accept: application/vnd.github+json" \
    "repos/$repo/issues/$pr_number/comments?per_page=100"
)"
review_request="$(
  jq -c \
    --arg bot "$review_bot" \
    --arg observed "$head_observed_at" \
    '[
      .[][]
      | select(
          $observed != ""
          and .user.login != $bot
          and .created_at > $observed
          and (.body | test("^\\s*@codex\\s+review(?:\\s.*)?$"; "i"))
        )
    ] | sort_by(.created_at) | last // empty' \
    <<<"$comments_json"
)"
if [[ -z "$review_request" ]]; then
  echo "review request: none for current head"
  exit 0
fi

request_id="$(jq -r '.id' <<<"$review_request")"
request_created_at="$(jq -r '.created_at' <<<"$review_request")"
head_short="${head_sha:0:10}"
echo "review request: issue comment $request_id"
echo "review-request reactions:"
"$gh_bin" api \
  --paginate \
  --slurp \
  -H "Accept: application/vnd.github+json" \
  "repos/$repo/issues/comments/$request_id/reactions?per_page=100" |
  jq -r '.[][] | "\(.content) \(.user.login) \(.created_at)"'
echo "PR reactions:"
"$gh_bin" api \
  --paginate \
  --slurp \
  -H "Accept: application/vnd.github+json" \
  "repos/$repo/issues/$pr_number/reactions?per_page=100" |
  jq -r '.[][] | "\(.content) \(.user.login) \(.created_at)"'
echo "matching clean completions:"
jq -r \
  --arg bot "$review_bot" \
  --arg requested "$request_created_at" \
  --arg reviewed "$head_short" \
  '
    .[][]
    | select(
        .user.login == $bot
        and .created_at >= $requested
        and (.body | contains("Reviewed commit:"))
        and (.body | contains($reviewed))
        and (
          (.body | test("Didn.t find any major issues"; "i"))
          or (.body | test("found no major issues"; "i"))
        )
      )
    | "clean \(.user.login) \(.created_at) reviewed \($reviewed)"
  ' \
  <<<"$comments_json"
