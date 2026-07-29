#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 [--bootstrap-persistent | --ci]" >&2
}

fail() {
  echo "runtime gate failed: $*" >&2
  exit 1
}

mode="local"
bootstrap_persistent="false"
if (($# == 1)); then
  case "$1" in
    --bootstrap-persistent)
      bootstrap_persistent="true"
      ;;
    --ci)
      mode="ci"
      ;;
    *)
      usage
      exit 2
      ;;
  esac
elif (($# > 1)); then
  usage
  exit 2
fi

[[ "$(uname -s)" == "Darwin" ]] || fail "real Tauri startup verification requires macOS"
command -v cargo >/dev/null 2>&1 || fail "cargo is unavailable"
command -v jq >/dev/null 2>&1 || fail "jq is unavailable"

repo_root="$(git rev-parse --show-toplevel)"
head_sha="$(git -C "$repo_root" rev-parse HEAD)"
[[ "$head_sha" =~ ^[0-9a-f]{40}$ ]] || fail "could not resolve the current Git head"
[[ -z "$(git -C "$repo_root" status --porcelain --untracked-files=normal)" ]] ||
  fail "tracked and untracked source changes must be committed before runtime verification"
[[ ! -L "$repo_root/.kosh-loop" ]] ||
  fail "the ignored loop-state root must not be a symlink"

runtime_root="$repo_root/.kosh-loop/runtime"
launch_root="$runtime_root/launches/$head_sha"
persistent_root="$repo_root/.kosh-loop/progressive-profile"
persistent_data="$persistent_root/data"
persistent_marker="$persistent_root/established.json"
aggregate_receipt="$repo_root/.kosh-loop/runtime-gate.json"
[[ ! -L "$runtime_root" && ! -L "$persistent_root" && ! -L "$persistent_data" ]] ||
  fail "runtime gate directories must not be symlinks"
[[ ! -L "$persistent_marker" ]] ||
  fail "the preserved-profile marker must not be a symlink"
mkdir -p "$launch_root" "$persistent_root"

fresh_data="$(mktemp -d "$runtime_root/fresh.XXXXXX")"
cleanup() {
  case "$fresh_data" in
    "$runtime_root"/fresh.*)
      rm -rf "$fresh_data"
      ;;
    *)
      echo "refusing to remove unexpected runtime gate path: $fresh_data" >&2
      ;;
  esac
}
trap cleanup EXIT

run_launch() {
  local data_dir="$1"
  local receipt="$2"
  local expectation="$3"
  local log_file="$4"

  rm -f "$receipt" "$log_file"
  mkdir -p "$data_dir" "$(dirname "$receipt")"
  if ! env \
    KOSH_DATA_DIR="$data_dir" \
    KOSH_STARTUP_SMOKE_RECEIPT="$receipt" \
    KOSH_STARTUP_SMOKE_HEAD="$head_sha" \
    KOSH_STARTUP_SMOKE_EXPECT="$expectation" \
    cargo run \
      --quiet \
      --locked \
      --manifest-path "$repo_root/app/src-tauri/Cargo.toml" \
      --no-default-features \
      --bin kosh >"$log_file" 2>&1; then
    sed -n '1,240p' "$log_file" >&2
    fail "the real Tauri $expectation launch failed"
  fi
  [[ -f "$receipt" && ! -L "$receipt" ]] ||
    fail "the real Tauri $expectation launch produced no regular receipt"
  local canonical_data
  canonical_data="$(cd "$data_dir" && pwd -P)"
  jq -e \
    --arg head "$head_sha" \
    --arg expectation "$expectation" \
    --arg data "$canonical_data" \
    '
      .schemaVersion == 1
      and .headSha == $head
      and .expectation == $expectation
      and .dataDir == $data
      and (.windows | sort) == ["main", "quick-add"]
      and .diagnostics.mainJournalMode == "wal"
      and .diagnostics.mediaJournalMode == "wal"
      and .diagnostics.mainForeignKeys == true
      and .diagnostics.mediaForeignKeys == true
      and (.diagnostics.migrationHeads.main | type) == "number"
      and (.diagnostics.migrationHeads.media | type) == "number"
      and .canary.sourceUrl == "https://example.invalid/kosh-progressive-operability"
      and (
        ($expectation == "absent" and .canaryPreexisting == false and .canaryCreated == true)
        or
        ($expectation == "present" and .canaryPreexisting == true and .canaryCreated == false)
      )
    ' \
    "$receipt" >/dev/null || fail "the real Tauri $expectation receipt is invalid"
}

fresh_seed_receipt="$launch_root/fresh-seed.json"
fresh_restart_receipt="$launch_root/fresh-restart.json"
run_launch "$fresh_data" "$fresh_seed_receipt" "absent" "$launch_root/fresh-seed.log"
run_launch "$fresh_data" "$fresh_restart_receipt" "present" "$launch_root/fresh-restart.log"

if [[ "$mode" == "ci" ]]; then
  temporary="$aggregate_receipt.$$.tmp"
  jq -n \
    --arg head "$head_sha" \
    --slurpfile seed "$fresh_seed_receipt" \
    --slurpfile restart "$fresh_restart_receipt" \
    '{
      schemaVersion: 1,
      scope: "ci",
      result: "pass",
      headSha: $head,
      fresh: {seed: $seed[0], restart: $restart[0]}
    }' >"$temporary"
  mv "$temporary" "$aggregate_receipt"
  echo "CI runtime gate passed for $head_sha"
  exit 0
fi

persistent_expectation="present"
if [[ ! -f "$persistent_marker" ]]; then
  [[ "$bootstrap_persistent" == "true" ]] ||
    fail "the preserved profile is not established; run once with --bootstrap-persistent"
  [[ ! -e "$persistent_data" ]] ||
    fail "unmarked preserved-profile data already exists and will not be overwritten"
  persistent_expectation="absent"
elif [[ "$bootstrap_persistent" == "true" ]]; then
  fail "the preserved profile is already established; bootstrap cannot run again"
fi

persistent_receipt="$launch_root/persistent.json"
run_launch \
  "$persistent_data" \
  "$persistent_receipt" \
  "$persistent_expectation" \
  "$launch_root/persistent.log"

if [[ "$persistent_expectation" == "absent" ]]; then
  marker_temporary="$persistent_marker.$$.tmp"
  jq -n \
    --arg head "$head_sha" \
    --arg data "$(cd "$persistent_data" && pwd -P)" \
    --arg tidbit "$(jq -r '.canary.tidbitId' "$persistent_receipt")" \
    --arg revision "$(jq -r '.canary.revisionId' "$persistent_receipt")" \
    '{
      schemaVersion: 1,
      establishedAtHead: $head,
      dataDir: $data,
      canaryTidbitId: $tidbit,
      canaryRevisionId: $revision
    }' >"$marker_temporary"
  mv "$marker_temporary" "$persistent_marker"
fi

temporary="$aggregate_receipt.$$.tmp"
jq -n \
  --arg head "$head_sha" \
  --arg expectation "$persistent_expectation" \
  --argjson bootstrap "$bootstrap_persistent" \
  --slurpfile seed "$fresh_seed_receipt" \
  --slurpfile restart "$fresh_restart_receipt" \
  --slurpfile persistent "$persistent_receipt" \
  '{
    schemaVersion: 1,
    scope: "local",
    result: "pass",
    headSha: $head,
    fresh: {seed: $seed[0], restart: $restart[0]},
    persistent: {
      bootstrap: $bootstrap,
      expectation: $expectation,
      receipt: $persistent[0]
    }
  }' >"$temporary"
mv "$temporary" "$aggregate_receipt"

"$repo_root/scripts/loop/verify-runtime-gate.sh" "$head_sha" "$aggregate_receipt"
