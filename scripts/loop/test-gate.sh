#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
gate="$repo_root/scripts/loop/gate.sh"
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
    if [[ "${2:-}" == "repos/polyphilz/kosh/commits/"* ]]; then
      echo "$FAKE_COMMITTED_AT"
    else
      [[ " $* " == *" --paginate "* && " $* " == *" --slurp "* ]] || {
        echo "review evidence request was not paginated and slurped" >&2
        exit 2
      }
      request="${*: -1}"
      if [[ "$request" == *"/issues/comments/77/reactions?"* ]]; then
        jq -cn --argjson page "$FAKE_REQUEST_REACTIONS_JSON" '[$page]'
      elif [[ "$request" == *"/reactions?"* ]]; then
        jq -cn --argjson page "$FAKE_REACTIONS_JSON" '[$page]'
      elif [[ "$request" == *"/comments?"* ]]; then
        jq -cn --argjson page "$FAKE_COMMENTS_JSON" '[$page]'
      else
        echo "unexpected API request: $request" >&2
        exit 2
      fi
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
readonly committed_at="2026-07-27T18:00:00Z"
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
      state: "OPEN",
      url: "https://github.com/polyphilz/kosh/pull/1"
    }'
)"
export FAKE_PR_JSON
export FAKE_CHECKS_JSON='[{"bucket":"pass","link":"","name":"check","state":"SUCCESS","workflow":"check"}]'
export FAKE_COMMITTED_AT="$committed_at"
FAKE_REACTIONS_JSON="$(
  jq -cn '[]'
)"
export FAKE_REACTIONS_JSON
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

"$gate" 1 polyphilz/kosh >/dev/null
"$repo_root/scripts/loop/merge.sh" 1 polyphilz/kosh >/dev/null
[[ -f "$FAKE_MERGE_MARKER" ]] || {
  echo "merge wrapper did not invoke a guarded merge" >&2
  exit 1
}

export FAKE_REQUEST_REACTIONS_JSON='[]'
FAKE_REACTIONS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      content: "+1",
      created_at: "2026-07-27T18:05:00Z"
    }]'
)"
export FAKE_REACTIONS_JSON
"$gate" 1 polyphilz/kosh >/dev/null
export FAKE_REACTIONS_JSON='[]'
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

FAKE_REQUEST_REACTIONS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      content: "+1",
      created_at: "2026-07-27T17:59:59Z"
    }]'
)"
export FAKE_REQUEST_REACTIONS_JSON
expect_blocked "stale reaction"
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
        body: "Codex Review: Didn'\''t find any major issues.\n\n**Reviewed commit:** `fffffffff0`"
      }
    ]'
)"
export FAKE_COMMENTS_JSON
expect_blocked "reviewed commit mismatch"

echo "merge gate tests passed"
