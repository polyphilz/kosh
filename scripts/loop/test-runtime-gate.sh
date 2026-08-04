#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
verifier="$repo_root/scripts/loop/verify-runtime-gate.sh"
temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT

readonly head_sha="0123456789abcdef0123456789abcdef01234567"
receipt="$temp_dir/runtime-gate.json"

launch_receipt() {
  local expectation="$1"
  local preexisting="$2"
  local created="$3"
  local tidbit="$4"
  local revision="$5"
  local passage="$6"

  jq -n \
    --arg head "$head_sha" \
    --arg expectation "$expectation" \
    --arg tidbit "$tidbit" \
    --arg revision "$revision" \
    --arg passage "$passage" \
    --argjson preexisting "$preexisting" \
    --argjson created "$created" \
    '{
      schemaVersion: 4,
      headSha: $head,
      buildHeadSha: $head,
      expectation: $expectation,
      dataDir: "/tmp/kosh-fresh",
      processId: 123,
      completedAtMs: 456,
      windows: ["main"],
      webviews: [
        {
          surface: "main",
          rendered: true,
          captureCreated: $created,
          documentReadyState: "complete",
          rootChildCount: 1,
          frontendOrigin: "http://127.0.0.1:1420",
          probeDataDir: "/tmp/kosh-fresh",
          probeRequestId: "00000000-0000-7000-8000-000000000004",
          canary: {
            executionMode: "EXACT",
            citationState: "CURRENT",
            resultCount: 1,
            passageId: $passage,
            resolvedPassageId: $passage,
            revisionId: $revision,
            canaryUrl: "https://example.invalid/kosh-progressive-operability"
          }
        }
      ],
      diagnostics: {
        migrationHeads: {main: 1, media: 1},
        mainJournalMode: "wal",
        mediaJournalMode: "wal",
        mainForeignKeys: true,
        mediaForeignKeys: true
      },
      canaryPreexisting: $preexisting,
      canaryCreated: $created,
      canary: {
        tidbitId: $tidbit,
        revisionId: $revision,
        passageId: $passage,
        canaryUrl: "https://example.invalid/kosh-progressive-operability"
      }
    }'
}

seed="$(
  launch_receipt \
    absent false true \
    00000000-0000-7000-8000-000000000001 \
    00000000-0000-7000-8000-000000000002 \
    00000000-0000-7000-8000-000000000003
)"
restart="$(
  launch_receipt \
    present true false \
    00000000-0000-7000-8000-000000000001 \
    00000000-0000-7000-8000-000000000002 \
    00000000-0000-7000-8000-000000000003
)"

write_aggregate() {
  local scope="$1"
  local aggregate_head="$2"
  jq -n \
    --arg scope "$scope" \
    --arg head "$aggregate_head" \
    --argjson seed "$seed" \
    --argjson restart "$restart" \
    '{
      schemaVersion: 1,
      scope: $scope,
      result: "pass",
      headSha: $head,
      fresh: {seed: $seed, restart: $restart}
    }' >"$receipt"
}

expect_blocked() {
  local label="$1"
  if "$verifier" "$head_sha" "$receipt" >/dev/null 2>&1; then
    echo "expected runtime receipt to be rejected: $label" >&2
    exit 1
  fi
}

write_aggregate local "$head_sha"
"$verifier" "$head_sha" "$receipt" >/dev/null

write_aggregate local ffffffffffffffffffffffffffffffffffffffff
expect_blocked "receipt names another head"

write_aggregate ci "$head_sha"
expect_blocked "CI-only receipt used for a local merge"

original_restart="$restart"
restart="$(jq '.canary.revisionId = "00000000-0000-7000-8000-000000000099"' <<<"$restart")"
write_aggregate local "$head_sha"
expect_blocked "restart silently retargets the citation"

restart="$(jq '.webviews[0].rendered = false' <<<"$original_restart")"
write_aggregate local "$head_sha"
expect_blocked "a webview did not render its React root"

restart="$(jq '.webviews[0].probeRequestId = ""' <<<"$original_restart")"
write_aggregate local "$head_sha"
expect_blocked "a webview has no backend IPC evidence"

restart="$(
  jq '.webviews[0].canary.resolvedPassageId = "00000000-0000-7000-8000-000000000099"' \
    <<<"$original_restart"
)"
write_aggregate local "$head_sha"
expect_blocked "a citation resolves to a different passage"

restart="$(jq '.webviews[0].frontendOrigin = "http://localhost:1420"' <<<"$original_restart")"
write_aggregate local "$head_sha"
expect_blocked "a webview loaded from an unowned frontend origin"

echo "runtime gate tests passed"
