#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
gate="$repo_root/scripts/loop/gate.sh"
status="$repo_root/scripts/loop/status.sh"
temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT

cat >"$temp_dir/gh" <<'FAKE_GH'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
  "repo")
    echo "polyphilz/kosh"
    ;;
  "pr")
    if [[ "${2:-}" == "view" && " $* " == *" --jq "* ]]; then
      echo "$FAKE_HEAD_SHA"
    elif [[ "${2:-}" == "view" ]]; then
      echo "$FAKE_PR_JSON"
    elif [[ "${2:-}" == "checks" ]]; then
      echo "$FAKE_CHECKS_JSON"
    elif [[ "${2:-}" == "merge" ]]; then
      [[ " $* " == *" --match-head-commit $FAKE_HEAD_SHA "* ]] || {
        echo "merge was not bound to expected head" >&2
        exit 2
      }
      : >"$FAKE_MERGE_MARKER"
    else
      echo "unexpected fake gh PR invocation: $*" >&2
      exit 2
    fi
    ;;
  "api")
    [[ " $* " == *" --paginate "* && " $* " == *" --slurp "* ]] || {
      echo "review evidence request was not paginated and slurped" >&2
      exit 2
    }
    request="${*: -1}"
    if [[ "$request" == *"/issues/comments/77/reactions?"* ]]; then
      jq -cn --argjson page "$FAKE_REQUEST_REACTIONS_JSON" '[$page]'
    elif [[ "$request" == *"/issues/1/reactions?"* ]]; then
      jq -cn --argjson page "$FAKE_PR_REACTIONS_JSON" '[$page]'
    elif [[ "$request" == *"/issues/"*"/comments?"* ]]; then
      jq -cn --argjson page "$FAKE_COMMENTS_JSON" '[$page]'
    elif [[ "$request" == *"/actions/runs?"* ]]; then
      jq -cn --argjson page "$FAKE_RUNS_JSON" '[$page]'
    else
      echo "unexpected API request: $request" >&2
      exit 2
    fi
    ;;
  *)
    echo "unexpected fake gh invocation: $*" >&2
    exit 2
    ;;
esac
FAKE_GH
chmod +x "$temp_dir/gh"

readonly head_sha="0123456789abcdef0123456789abcdef01234567"
readonly bot="chatgpt-codex-connector[bot]"

export GH_BIN="$temp_dir/gh"
export FAKE_HEAD_SHA="$head_sha"
export FAKE_MERGE_MARKER="$temp_dir/merged"
FAKE_PR_JSON="$(
  jq -cn \
    --arg head "$head_sha" \
    '{
      baseRefName: "main",
      headRefOid: $head,
      isDraft: false,
      mergeStateStatus: "CLEAN",
      mergeable: "MERGEABLE",
      number: 1,
      state: "OPEN",
      url: "https://github.com/polyphilz/kosh/pull/1"
    }'
)"
export FAKE_PR_JSON
export FAKE_CHECKS_JSON='[{"bucket":"pass","link":"","name":"check","state":"SUCCESS","workflow":"check"}]'
FAKE_RUNS_JSON="$(
  jq -cn \
    --arg head "$head_sha" \
    '{
      workflow_runs: [{
        created_at: "2026-07-27T18:01:00Z",
        event: "pull_request",
        head_sha: $head
      }]
    }'
)"
export FAKE_RUNS_JSON
export FAKE_REQUEST_REACTIONS_JSON='[]'
export FAKE_PR_REACTIONS_JSON='[]'
FAKE_COMMENTS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[
      {
        id: 77,
        user: {login: "polyphilz"},
        created_at: "2026-07-27T18:02:00Z",
        body: "@codex review"
      },
      {
        id: 78,
        user: {login: $bot},
        created_at: "2026-07-27T18:05:00Z",
        body: "Codex Review: Didn'\''t find any major issues.\n\n**Reviewed commit:** `0123456789`"
      }
    ]'
)"
export FAKE_COMMENTS_JSON

# A matching clean completion is sufficient even if GitHub exposes no +1.
"$gate" 1 polyphilz/kosh >/dev/null
"$repo_root/scripts/loop/merge.sh" 1 polyphilz/kosh >/dev/null
[[ -f "$FAKE_MERGE_MARKER" ]] || {
  echo "merge wrapper did not invoke a guarded merge" >&2
  exit 1
}
status_output="$("$status" polyphilz/kosh)"
grep -F "review request: issue comment 77" <<<"$status_output" >/dev/null
grep -F "PR reactions:" <<<"$status_output" >/dev/null
grep -F "matching clean completions:" <<<"$status_output" >/dev/null
grep -F "clean $bot 2026-07-27T18:05:00Z reviewed 0123456789" \
  <<<"$status_output" >/dev/null

# A +1 is likewise sufficient when Codex does not post a clean comment.
FAKE_REQUEST_REACTIONS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      content: "+1",
      created_at: "2026-07-27T18:05:00Z"
    }]'
)"
export FAKE_REQUEST_REACTIONS_JSON
FAKE_COMMENTS_JSON="$(
  jq -cn \
    '[
      {
        id: 77,
        user: {login: "polyphilz"},
        created_at: "2026-07-27T18:02:00Z",
        body: "@codex review"
      }
    ]'
)"
export FAKE_COMMENTS_JSON
"$gate" 1 polyphilz/kosh >/dev/null
export FAKE_REQUEST_REACTIONS_JSON='[]'

# GitHub may place the +1 on the PR body rather than the request comment.
FAKE_PR_REACTIONS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      content: "+1",
      created_at: "2026-07-27T18:05:00Z"
    }]'
)"
export FAKE_PR_REACTIONS_JSON
"$gate" 1 polyphilz/kosh >/dev/null
export FAKE_PR_REACTIONS_JSON='[]'

expect_blocked() {
  local label="$1"
  if "$gate" 1 polyphilz/kosh >/dev/null 2>&1; then
    echo "expected merge gate to block: $label" >&2
    exit 1
  fi
}

export FAKE_CHECKS_JSON='[{"bucket":"fail","link":"","name":"check","state":"FAILURE","workflow":"check"}]'
expect_blocked "failed CI"
export FAKE_CHECKS_JSON='[{"bucket":"pass","link":"","name":"check","state":"SUCCESS","workflow":"check"}]'

# With neither clean signal, the gate stays closed.
expect_blocked "missing clean review signal"

FAKE_PR_REACTIONS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      content: "+1",
      created_at: "2026-07-27T17:59:59Z"
    }]'
)"
export FAKE_PR_REACTIONS_JSON
expect_blocked "stale reaction"

FAKE_COMMENTS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[
      {
        id: 77,
        user: {login: "polyphilz"},
        created_at: "2026-07-27T18:02:00Z",
        body: "@codex review"
      },
      {
        id: 78,
        user: {login: $bot},
        created_at: "2026-07-27T18:05:00Z",
        body: "Codex Review: Didn'\''t find any major issues.\n\n**Reviewed commit:** `ffffffffff`"
      }
    ]'
)"
export FAKE_COMMENTS_JSON
expect_blocked "clean review names another head"
FAKE_COMMENTS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[
      {
        id: 77,
        user: {login: "polyphilz"},
        created_at: "2026-07-27T18:02:00Z",
        body: "@codex review"
      },
      {
        id: 78,
        user: {login: $bot},
        created_at: "2026-07-27T18:05:00Z",
        body: "Codex Review: Didn'\''t find any major issues.\n\n**Reviewed commit:** `0123456789`"
      }
    ]'
)"
export FAKE_COMMENTS_JSON

FAKE_RUNS_JSON="$(
  jq -cn \
    --arg head "$head_sha" \
    '{
      workflow_runs: [
        {
          created_at: "2026-07-27T18:00:00Z",
          event: "pull_request",
          head_sha: $head
        },
        {
          created_at: "2026-07-27T18:02:00Z",
          event: "pull_request",
          head_sha: $head
        }
      ]
    }'
)"
export FAKE_RUNS_JSON
expect_blocked "review request is not newer than latest transition to current head"

echo "merge gate tests passed"
