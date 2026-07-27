#!/usr/bin/env bash
set -euo pipefail

readonly review_bot="${KOSH_CODEX_REVIEW_BOT:-chatgpt-codex-connector[bot]}"
readonly gh_bin="${GH_BIN:-gh}"

usage() {
  echo "usage: $0 <pull-request-number> [owner/repository]" >&2
}

fail() {
  echo "merge gate blocked: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command is unavailable: $1"
}

if (($# < 1 || $# > 2)); then
  usage
  exit 2
fi

require_command "$gh_bin"
require_command jq

pr_number="$1"
[[ "$pr_number" =~ ^[1-9][0-9]*$ ]] || fail "pull request number must be a positive integer"

repo="${2:-}"
if [[ -z "$repo" ]]; then
  repo="$("$gh_bin" repo view --json nameWithOwner --jq '.nameWithOwner')"
fi
[[ "$repo" =~ ^[^/]+/[^/]+$ ]] || fail "repository must use owner/name form"

pr_json="$(
  "$gh_bin" pr view "$pr_number" --repo "$repo" \
    --json baseRefName,headRefOid,isDraft,mergeStateStatus,mergeable,state,url
)"

state="$(jq -r '.state' <<<"$pr_json")"
is_draft="$(jq -r '.isDraft' <<<"$pr_json")"
base_ref="$(jq -r '.baseRefName' <<<"$pr_json")"
head_sha="$(jq -r '.headRefOid' <<<"$pr_json")"
mergeable="$(jq -r '.mergeable' <<<"$pr_json")"
merge_state="$(jq -r '.mergeStateStatus' <<<"$pr_json")"
pr_url="$(jq -r '.url' <<<"$pr_json")"

[[ "$state" == "OPEN" ]] || fail "pull request is not open"
[[ "$is_draft" == "false" ]] || fail "pull request is still a draft"
[[ "$base_ref" == "main" ]] || fail "pull request targets $base_ref instead of main"
[[ "$head_sha" =~ ^[0-9a-f]{40}$ ]] || fail "pull request head SHA is invalid"
[[ "$mergeable" == "MERGEABLE" ]] || fail "GitHub reports mergeable=$mergeable"
case "$merge_state" in
  CLEAN | HAS_HOOKS | UNSTABLE) ;;
  *) fail "GitHub reports mergeStateStatus=$merge_state" ;;
esac

checks_json="$(
  "$gh_bin" pr checks "$pr_number" --repo "$repo" \
    --json bucket,link,name,state,workflow
)"
check_count="$(jq 'length' <<<"$checks_json")"
((check_count > 0)) || fail "no CI checks are reported"

blocking_checks="$(
  jq -r '
    .[]
    | select(.bucket != "pass" and .bucket != "skipping")
    | "\(.name): \(.state) [\(.bucket)]"
  ' <<<"$checks_json"
)"
if [[ -n "$blocking_checks" ]]; then
  echo "$blocking_checks" >&2
  fail "CI checks are not all complete and successful"
fi

comments_json="$(
  "$gh_bin" api \
    --paginate \
    --slurp \
    -H "Accept: application/vnd.github+json" \
    "repos/$repo/issues/$pr_number/comments?per_page=100"
)"
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
    ] | min // empty' \
    <<<"$workflow_runs_json"
)"
[[ -n "$head_observed_at" ]] ||
  fail "no GitHub Actions run records when the current head reached GitHub"

review_request="$(
  jq -c \
    --arg bot "$review_bot" \
    --arg observed "$head_observed_at" \
    '[
      .[][]
      | select(
          .user.login != $bot
          and .created_at >= $observed
          and (.body | test("^\\s*@codex\\s+review(?:\\s.*)?$"; "i"))
        )
    ] | sort_by(.created_at) | last // empty' \
    <<<"$comments_json"
)"
[[ -n "$review_request" ]] ||
  fail "no Codex review request follows GitHub's observation of the current head"

request_id="$(jq -r '.id' <<<"$review_request")"
request_created_at="$(jq -r '.created_at' <<<"$review_request")"
[[ "$request_id" =~ ^[1-9][0-9]*$ ]] ||
  fail "latest Codex review request has an invalid comment ID"
request_reactions_json="$(
  "$gh_bin" api \
    --paginate \
    --slurp \
    -H "Accept: application/vnd.github+json" \
    "repos/$repo/issues/comments/$request_id/reactions?per_page=100"
)"
request_approval_count="$(
  jq \
    --arg bot "$review_bot" \
    --arg requested "$request_created_at" \
    '[
      .[][]
      | select(
          .user.login == $bot
          and .content == "+1"
          and .created_at >= $requested
        )
    ] | length' \
    <<<"$request_reactions_json"
)"
((request_approval_count > 0)) ||
  fail "no clean +1 from $review_bot on the current-head review request"

echo "merge gate passed for $pr_url at $head_sha"
