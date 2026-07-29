#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "runtime gate invalid: $*" >&2
  exit 1
}

if (($# < 1 || $# > 2)); then
  echo "usage: $0 <expected-head-sha> [receipt-path]" >&2
  exit 2
fi

repo_root="$(git rev-parse --show-toplevel)"
expected_head="$1"
receipt="${2:-$repo_root/.kosh-loop/runtime-gate.json}"
profile_root="${KOSH_PROGRESSIVE_PROFILE_ROOT:-$repo_root/.kosh-loop/progressive-profile}"
marker="$profile_root/established.json"

[[ "$expected_head" =~ ^[0-9a-f]{40}$ ]] ||
  fail "expected head must be a 40-character lowercase Git SHA"
[[ -f "$receipt" && ! -L "$receipt" ]] || fail "receipt is missing or not a regular file"
[[ -f "$marker" && ! -L "$marker" ]] ||
  fail "the preserved-profile marker is missing or not a regular file"

jq -e \
  --arg head "$expected_head" \
  '
    .schemaVersion == 1
    and .scope == "local"
    and .result == "pass"
    and .headSha == $head
    and .fresh.seed.schemaVersion == 1
    and .fresh.seed.headSha == $head
    and .fresh.seed.expectation == "absent"
    and .fresh.seed.canaryPreexisting == false
    and .fresh.seed.canaryCreated == true
    and .fresh.restart.schemaVersion == 1
    and .fresh.restart.headSha == $head
    and .fresh.restart.expectation == "present"
    and .fresh.restart.canaryPreexisting == true
    and .fresh.restart.canaryCreated == false
    and .fresh.seed.dataDir == .fresh.restart.dataDir
    and .fresh.seed.canary.tidbitId == .fresh.restart.canary.tidbitId
    and .fresh.seed.canary.revisionId == .fresh.restart.canary.revisionId
    and .fresh.seed.canary.passageId == .fresh.restart.canary.passageId
    and .persistent.receipt.schemaVersion == 1
    and .persistent.receipt.headSha == $head
    and .persistent.receipt.canaryPreexisting == (.persistent.expectation == "present")
    and .persistent.receipt.canaryCreated == (.persistent.expectation == "absent")
    and (
      (.persistent.bootstrap == true and .persistent.expectation == "absent")
      or
      (.persistent.bootstrap == false and .persistent.expectation == "present")
    )
    and (.fresh.seed.windows | sort) == ["main", "quick-add"]
    and (.fresh.restart.windows | sort) == ["main", "quick-add"]
    and (.persistent.receipt.windows | sort) == ["main", "quick-add"]
    and all(
      [.fresh.seed, .fresh.restart, .persistent.receipt][];
      .diagnostics.mainJournalMode == "wal"
      and .diagnostics.mediaJournalMode == "wal"
      and .diagnostics.mainForeignKeys == true
      and .diagnostics.mediaForeignKeys == true
      and (.diagnostics.migrationHeads.main | type) == "number"
      and (.diagnostics.migrationHeads.media | type) == "number"
      and .canary.sourceUrl == "https://example.invalid/kosh-progressive-operability"
    )
  ' \
  "$receipt" >/dev/null || fail "receipt contents do not satisfy the progressive launch contract"

persistent_data="$(
  jq -er '.persistent.receipt.dataDir | select(type == "string" and length > 0)' "$receipt"
)" || fail "receipt has no preserved-profile data directory"
[[ "$persistent_data" == "$profile_root/data" ]] ||
  fail "receipt points at an unexpected preserved-profile directory"
[[ -d "$persistent_data" && ! -L "$persistent_data" ]] ||
  fail "the preserved-profile data directory is missing or is a symlink"
[[ -f "$persistent_data/kosh.sqlite3" && -f "$persistent_data/media.sqlite3" ]] ||
  fail "the preserved-profile database pair is missing"

jq -e \
  --arg data "$persistent_data" \
  --arg tidbit "$(jq -r '.persistent.receipt.canary.tidbitId' "$receipt")" \
  --arg revision "$(jq -r '.persistent.receipt.canary.revisionId' "$receipt")" \
  '
    .schemaVersion == 1
    and .dataDir == $data
    and .canaryTidbitId == $tidbit
    and .canaryRevisionId == $revision
  ' \
  "$marker" >/dev/null || fail "the preserved-profile marker does not match the live receipt"

echo "runtime gate passed for $expected_head"
