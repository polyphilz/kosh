#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
verifier="$repo_root/scripts/loop/verify-runtime-gate.sh"
temp_dir="$(mktemp -d)"
trap 'rm -rf "$temp_dir"' EXIT

readonly head_sha="0123456789abcdef0123456789abcdef01234567"
profile_root="$temp_dir/profile"
persistent_data="$profile_root/data"
marker="$profile_root/established.json"
receipt="$temp_dir/runtime-gate.json"
mkdir -p "$persistent_data"
: >"$persistent_data/kosh.sqlite3"
: >"$persistent_data/media.sqlite3"

launch_receipt() {
  local expectation="$1"
  local preexisting="$2"
  local created="$3"
  local data_dir="$4"
  local tidbit="$5"
  local revision="$6"
  local passage="$7"

  jq -n \
    --arg head "$head_sha" \
    --arg expectation "$expectation" \
    --arg data "$data_dir" \
    --arg tidbit "$tidbit" \
    --arg revision "$revision" \
    --arg passage "$passage" \
    --argjson preexisting "$preexisting" \
    --argjson created "$created" \
    '{
      schemaVersion: 1,
      headSha: $head,
      expectation: $expectation,
      dataDir: $data,
      processId: 123,
      completedAtMs: 456,
      windows: ["main", "quick-add"],
      diagnostics: {
        migrationHeads: {main: 11, media: 2},
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
        sourceUrl: "https://example.invalid/kosh-progressive-operability"
      }
    }'
}

fresh_data="$temp_dir/fresh"
seed="$(
  launch_receipt \
    absent false true "$fresh_data" \
    00000000-0000-7000-8000-000000000001 \
    00000000-0000-7000-8000-000000000002 \
    00000000-0000-7000-8000-000000000003
)"
restart="$(
  launch_receipt \
    present true false "$fresh_data" \
    00000000-0000-7000-8000-000000000001 \
    00000000-0000-7000-8000-000000000002 \
    00000000-0000-7000-8000-000000000003
)"
persistent="$(
  launch_receipt \
    present true false "$persistent_data" \
    00000000-0000-7000-8000-000000000011 \
    00000000-0000-7000-8000-000000000012 \
    00000000-0000-7000-8000-000000000013
)"

jq -n \
  --arg data "$persistent_data" \
  '{
    schemaVersion: 1,
    establishedAtHead: "previous",
    dataDir: $data,
    canaryTidbitId: "00000000-0000-7000-8000-000000000011",
    canaryRevisionId: "00000000-0000-7000-8000-000000000012"
  }' >"$marker"

write_aggregate() {
  local scope="$1"
  local aggregate_head="$2"
  local bootstrap="$3"
  local expectation="$4"
  local persistent_json="$5"
  jq -n \
    --arg scope "$scope" \
    --arg head "$aggregate_head" \
    --arg expectation "$expectation" \
    --argjson bootstrap "$bootstrap" \
    --argjson seed "$seed" \
    --argjson restart "$restart" \
    --argjson persistent "$persistent_json" \
    '{
      schemaVersion: 1,
      scope: $scope,
      result: "pass",
      headSha: $head,
      fresh: {seed: $seed, restart: $restart},
      persistent: {
        bootstrap: $bootstrap,
        expectation: $expectation,
        receipt: $persistent
      }
    }' >"$receipt"
}

expect_blocked() {
  local label="$1"
  if KOSH_PROGRESSIVE_PROFILE_ROOT="$profile_root" \
    "$verifier" "$head_sha" "$receipt" >/dev/null 2>&1; then
    echo "expected runtime receipt to be rejected: $label" >&2
    exit 1
  fi
}

write_aggregate local "$head_sha" true present "$persistent"
KOSH_PROGRESSIVE_PROFILE_ROOT="$profile_root" \
  "$verifier" "$head_sha" "$receipt" >/dev/null

write_aggregate local ffffffffffffffffffffffffffffffffffffffff true present "$persistent"
expect_blocked "receipt names another head"

write_aggregate ci "$head_sha" true present "$persistent"
expect_blocked "CI-only receipt used for a local merge"

bad_restart="$(jq '.canary.revisionId = "00000000-0000-7000-8000-000000000099"' <<<"$restart")"
original_restart="$restart"
restart="$bad_restart"
write_aggregate local "$head_sha" true present "$persistent"
expect_blocked "fresh restart silently retargets the citation"
restart="$original_restart"

rm "$persistent_data/media.sqlite3"
write_aggregate local "$head_sha" true present "$persistent"
expect_blocked "preserved database pair is incomplete"
: >"$persistent_data/media.sqlite3"

write_aggregate local "$head_sha" false present "$persistent"
KOSH_PROGRESSIVE_PROFILE_ROOT="$profile_root" \
  "$verifier" "$head_sha" "$receipt" >/dev/null

jq '.canaryRevisionId = "00000000-0000-7000-8000-000000000099"' \
  "$marker" >"$marker.invalid"
mv "$marker.invalid" "$marker"
expect_blocked "preserved marker silently retargets the canary"

echo "runtime gate tests passed"
