#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 [--ci]" >&2
}

fail() {
  echo "runtime gate failed: $*" >&2
  exit 1
}

mode="local"
if (($# == 1)); then
  case "$1" in
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
pnpm_bin="${PNPM_BIN:-pnpm}"
curl_bin="${CURL_BIN:-curl}"
testing="${KOSH_RUNTIME_GATE_TESTING:-false}"
[[ "$platform" == "Darwin" ]] || fail "real Tauri startup verification requires macOS"
command -v "$cargo_bin" >/dev/null 2>&1 || fail "cargo is unavailable"
command -v "$git_bin" >/dev/null 2>&1 || fail "git is unavailable"
command -v jq >/dev/null 2>&1 || fail "jq is unavailable"
if [[ "$testing" != "true" ]]; then
  command -v "$pnpm_bin" >/dev/null 2>&1 || fail "pnpm is unavailable"
  command -v "$curl_bin" >/dev/null 2>&1 || fail "curl is unavailable"
fi

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
aggregate_receipt="$loop_root/runtime-gate.json"
[[ ! -L "$runtime_root" ]] || fail "the runtime gate directory must not be a symlink"
mkdir -p "$launch_root"

fresh_data="$(mktemp -d "$runtime_root/fresh.XXXXXX")"
frontend_pid=""
cleanup() {
  if [[ -n "$frontend_pid" ]]; then
    kill "$frontend_pid" >/dev/null 2>&1 || true
    wait "$frontend_pid" >/dev/null 2>&1 || true
  fi
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

if [[ "$testing" != "true" ]]; then
  frontend_log="$launch_root/frontend.log"
  if "$curl_bin" \
    --fail \
    --silent \
    --show-error \
    --connect-timeout 1 \
    --max-time 1 \
    http://127.0.0.1:1420/ >/dev/null 2>&1; then
    fail "port 1420 is already serving HTTP; stop the existing frontend before verification"
  fi
  rm -f "$frontend_log"
  (
    cd "$repo_root/app"
    exec "$pnpm_bin" exec vite --host 127.0.0.1 --port 1420 --strictPort
  ) >"$frontend_log" 2>&1 &
  frontend_pid=$!

  frontend_ready="false"
  for _ in {1..80}; do
    if ! kill -0 "$frontend_pid" >/dev/null 2>&1; then
      sed -n '1,160p' "$frontend_log" >&2
      fail "the exact-head frontend server exited before becoming ready"
    fi
    if "$curl_bin" \
      --fail \
      --silent \
      --show-error \
      --connect-timeout 1 \
      --max-time 1 \
      http://127.0.0.1:1420/ >/dev/null 2>&1; then
      frontend_ready="true"
      break
    fi
    sleep 0.25
  done
  [[ "$frontend_ready" == "true" ]] || {
    sed -n '1,160p' "$frontend_log" >&2
    fail "the exact-head frontend server did not become ready"
  }
  kill -0 "$frontend_pid" >/dev/null 2>&1 ||
    fail "the exact-head frontend server exited after its readiness check"
fi

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
      . as $launch
      | .schemaVersion == 4
      and .headSha == $head
      and .buildHeadSha == $head
      and .expectation == $expectation
      and .dataDir == $data
      and (.windows | sort) == ["main", "quick-add"]
      and ([.webviews[].surface] | sort) == ["main", "quick-add"]
      and (.webviews | length) == 2
      and all(
        .webviews[];
        .rendered == true
        and (
          .captureCreated
          == (.surface == "main" and $launch.canaryPreexisting == false)
        )
        and .rootChildCount > 0
        and (.documentReadyState == "interactive" or .documentReadyState == "complete")
        and .frontendOrigin == "http://127.0.0.1:1420"
        and .probeDataDir == $data
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
      and ([.webviews[].probeRequestId] | unique | length) == 2
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
jq -e -n \
  --slurpfile seed "$fresh_seed_receipt" \
  --slurpfile restart "$fresh_restart_receipt" \
  '
    $seed[0].dataDir == $restart[0].dataDir
    and $seed[0].canary.tidbitId == $restart[0].canary.tidbitId
    and $seed[0].canary.revisionId == $restart[0].canary.revisionId
    and $seed[0].canary.passageId == $restart[0].canary.passageId
    and $seed[0].canary.sourceUrl == $restart[0].canary.sourceUrl
  ' >/dev/null ||
  fail "the fresh restart silently retargeted the startup canary citation"

temporary="$aggregate_receipt.$$.tmp"
jq -n \
  --arg head "$head_sha" \
  --arg scope "$mode" \
  --slurpfile seed "$fresh_seed_receipt" \
  --slurpfile restart "$fresh_restart_receipt" \
  '{
    schemaVersion: 1,
    scope: $scope,
    result: "pass",
    headSha: $head,
    fresh: {seed: $seed[0], restart: $restart[0]}
  }' >"$temporary"
mv "$temporary" "$aggregate_receipt"

if [[ "$mode" == "ci" ]]; then
  echo "CI runtime gate passed for $head_sha"
else
  "$repo_root/scripts/loop/verify-runtime-gate.sh" "$head_sha" "$aggregate_receipt"
fi
