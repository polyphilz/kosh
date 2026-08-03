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
loop_root="${KOSH_LOOP_STATE_ROOT:-$repo_root/.kosh-loop}"
receipt="${2:-$loop_root/runtime-gate.json}"

[[ "$expected_head" =~ ^[0-9a-f]{40}$ ]] ||
  fail "expected head must be a 40-character lowercase Git SHA"
[[ -f "$receipt" && ! -L "$receipt" ]] || fail "receipt is missing or not a regular file"

jq -e \
  --arg head "$expected_head" \
  '
    .schemaVersion == 1
    and .scope == "local"
    and .result == "pass"
    and .headSha == $head
    and .fresh.seed.schemaVersion == 4
    and .fresh.seed.headSha == $head
    and .fresh.seed.buildHeadSha == $head
    and .fresh.seed.expectation == "absent"
    and .fresh.seed.canaryPreexisting == false
    and .fresh.seed.canaryCreated == true
    and .fresh.restart.schemaVersion == 4
    and .fresh.restart.headSha == $head
    and .fresh.restart.buildHeadSha == $head
    and .fresh.restart.expectation == "present"
    and .fresh.restart.canaryPreexisting == true
    and .fresh.restart.canaryCreated == false
    and .fresh.seed.dataDir == .fresh.restart.dataDir
    and .fresh.seed.canary.tidbitId == .fresh.restart.canary.tidbitId
    and .fresh.seed.canary.revisionId == .fresh.restart.canary.revisionId
    and .fresh.seed.canary.passageId == .fresh.restart.canary.passageId
    and (.fresh.seed.windows | sort) == ["main", "quick-add"]
    and (.fresh.restart.windows | sort) == ["main", "quick-add"]
    and all(
      [.fresh.seed, .fresh.restart][];
      . as $launch
      | ([$launch.webviews[].surface] | sort) == ["main", "quick-add"]
      and ($launch.webviews | length) == 2
      and all(
        $launch.webviews[];
        .rendered == true
        and (
          .captureCreated
          == (.surface == "main" and $launch.canaryPreexisting == false)
        )
        and .rootChildCount > 0
        and (.documentReadyState == "interactive" or .documentReadyState == "complete")
        and .frontendOrigin == "http://127.0.0.1:1420"
        and .probeDataDir == $launch.dataDir
        and (.probeRequestId | type) == "string"
        and (.probeRequestId | length) > 0
        and .canary.executionMode == "EXACT"
        and .canary.citationState == "CURRENT"
        and .canary.resultCount == 1
        and .canary.passageId == $launch.canary.passageId
        and .canary.resolvedPassageId == $launch.canary.passageId
        and .canary.revisionId == $launch.canary.revisionId
        and .canary.sourceUrl == $launch.canary.sourceUrl
      )
      and ([$launch.webviews[].probeRequestId] | unique | length) == 2
      and
      $launch.diagnostics.mainJournalMode == "wal"
      and $launch.diagnostics.mediaJournalMode == "wal"
      and $launch.diagnostics.mainForeignKeys == true
      and $launch.diagnostics.mediaForeignKeys == true
      and ($launch.diagnostics.migrationHeads.main | type) == "number"
      and ($launch.diagnostics.migrationHeads.media | type) == "number"
      and $launch.canary.sourceUrl == "https://example.invalid/kosh-progressive-operability"
    )
  ' \
  "$receipt" >/dev/null || fail "receipt contents do not satisfy the fresh/restart contract"

echo "runtime gate passed for $expected_head"
