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

platform="${KOSH_RUNTIME_GATE_PLATFORM:-$(uname -s)}"
cargo_bin="${CARGO_BIN:-cargo}"
git_bin="${GIT_BIN:-git}"
[[ "$platform" == "Darwin" ]] || fail "real Tauri startup verification requires macOS"
command -v "$cargo_bin" >/dev/null 2>&1 || fail "cargo is unavailable"
command -v "$git_bin" >/dev/null 2>&1 || fail "git is unavailable"
command -v jq >/dev/null 2>&1 || fail "jq is unavailable"

repo_root="$("$git_bin" rev-parse --show-toplevel)"
head_sha="$("$git_bin" -C "$repo_root" rev-parse HEAD)"
[[ "$head_sha" =~ ^[0-9a-f]{40}$ ]] || fail "could not resolve the current Git head"
[[ -z "$("$git_bin" -C "$repo_root" status --porcelain --untracked-files=normal)" ]] ||
  fail "tracked and untracked source changes must be committed before runtime verification"
loop_root="${KOSH_LOOP_STATE_ROOT:-$repo_root/.kosh-loop}"
[[ "$loop_root" == /* ]] || fail "the loop-state root must be absolute"
[[ ! -L "$loop_root" ]] ||
  fail "the ignored loop-state root must not be a symlink"

runtime_root="$loop_root/runtime"
launch_root="$runtime_root/launches/$head_sha"
persistent_root="$loop_root/progressive-profile"
persistent_data="$persistent_root/data"
persistent_marker="$persistent_root/established.json"
aggregate_receipt="$loop_root/runtime-gate.json"
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
    "$cargo_bin" run \
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
        or
        (
          $expectation == "ensure"
          and (
            (.canaryPreexisting == false and .canaryCreated == true)
            or
            (.canaryPreexisting == true and .canaryCreated == false)
          )
        )
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

if [[ ! -f "$persistent_marker" ]]; then
  [[ "$bootstrap_persistent" == "true" ]] ||
    fail "the preserved profile is not established; run once with --bootstrap-persistent"
  bootstrap_staging="$persistent_root/bootstrap-incomplete"
  bootstrap_owner="$persistent_root/bootstrap-owned.json"
  [[ ! -L "$bootstrap_staging" && ! -L "$bootstrap_owner" ]] ||
    fail "bootstrap recovery state must not use symlinks"
  if [[ ! -f "$bootstrap_owner" ]]; then
    [[ ! -e "$bootstrap_staging" && ! -e "$persistent_data" ]] ||
      fail "unowned preserved-profile bootstrap data already exists"
    owner_temporary="$bootstrap_owner.$$.tmp"
    jq -n \
      --arg head "$head_sha" \
      --arg staging "$bootstrap_staging" \
      --arg data "$persistent_data" \
      '{
        schemaVersion: 1,
        establishedAtHead: $head,
        stagingDir: $staging,
        dataDir: $data
      }' >"$owner_temporary"
    mv "$owner_temporary" "$bootstrap_owner"
  fi
  [[ -f "$bootstrap_owner" && ! -L "$bootstrap_owner" ]] ||
    fail "the preserved-profile bootstrap owner is invalid"
  jq -e \
    --arg staging "$bootstrap_staging" \
    --arg data "$persistent_data" \
    '
      .schemaVersion == 1
      and .stagingDir == $staging
      and .dataDir == $data
    ' \
    "$bootstrap_owner" >/dev/null ||
    fail "the preserved-profile bootstrap owner does not match this repository"
  [[ ! ( -e "$bootstrap_staging" && -e "$persistent_data" ) ]] ||
    fail "both staged and promoted bootstrap profiles exist"

  if [[ ! -e "$persistent_data" ]]; then
    bootstrap_receipt="$launch_root/persistent-bootstrap.json"
    run_launch \
      "$bootstrap_staging" \
      "$bootstrap_receipt" \
      "ensure" \
      "$launch_root/persistent-bootstrap.log"
    mv "$bootstrap_staging" "$persistent_data"
  fi
elif [[ "$bootstrap_persistent" == "true" ]]; then
  fail "the preserved profile is already established; bootstrap cannot run again"
fi

persistent_receipt="$launch_root/persistent.json"
run_launch \
  "$persistent_data" \
  "$persistent_receipt" \
  "present" \
  "$launch_root/persistent.log"

if [[ ! -f "$persistent_marker" ]]; then
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
  unlink "$bootstrap_owner"
fi

temporary="$aggregate_receipt.$$.tmp"
jq -n \
  --arg head "$head_sha" \
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
      expectation: "present",
      receipt: $persistent[0]
    }
  }' >"$temporary"
mv "$temporary" "$aggregate_receipt"

"$repo_root/scripts/loop/verify-runtime-gate.sh" "$head_sha" "$aggregate_receipt"
