#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
gate="$repo_root/scripts/loop/gate.sh"
temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT

cat >"$temp_dir/gh" <<'FAKE_GH'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-} ${2:-}" in
  "repo view")
    echo "polyphilz/kosh"
    ;;
  "pr view")
    echo "$FAKE_PR_JSON"
    ;;
  "pr checks")
    echo "$FAKE_CHECKS_JSON"
    ;;
  "api repos/polyphilz/kosh/commits/"*)
    echo "$FAKE_COMMITTED_AT"
    ;;
  "api -H")
    request="${4:-}"
    if [[ "$request" == *"/reactions?"* ]]; then
      echo "$FAKE_REACTIONS_JSON"
    elif [[ "$request" == *"/comments?"* ]]; then
      echo "$FAKE_COMMENTS_JSON"
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
readonly committed_at="2026-07-27T18:00:00Z"
readonly bot="chatgpt-codex-connector[bot]"

export GH_BIN="$temp_dir/gh"
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
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      content: "+1",
      created_at: "2026-07-27T18:05:00Z"
    }]'
)"
export FAKE_REACTIONS_JSON
FAKE_COMMENTS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      created_at: "2026-07-27T18:05:00Z",
      body: "Codex Review: Didn'\''t find any major issues.\n\n**Reviewed commit:** `0123456789`"
    }]'
)"
export FAKE_COMMENTS_JSON

"$gate" 1 polyphilz/kosh >/dev/null

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

FAKE_REACTIONS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      content: "+1",
      created_at: "2026-07-27T17:59:59Z"
    }]'
)"
export FAKE_REACTIONS_JSON
expect_blocked "stale reaction"
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

FAKE_COMMENTS_JSON="$(
  jq -cn \
    --arg bot "$bot" \
    '[{
      user: {login: $bot},
      created_at: "2026-07-27T18:05:00Z",
      body: "Codex Review: Didn'\''t find any major issues.\n\n**Reviewed commit:** `fffffffff0`"
    }]'
)"
export FAKE_COMMENTS_JSON
expect_blocked "reviewed commit mismatch"

echo "merge gate tests passed"
