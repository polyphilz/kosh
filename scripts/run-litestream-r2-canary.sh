#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
app_root="$repo_root/app"
environment_file="${KOSH_LITESTREAM_ENV_FILE:-$app_root/.env}"
staged_binary="$app_root/src-tauri/resources/release/bin/litestream"
packaged_app="${KOSH_R2_CANARY_PACKAGED_APP:-$app_root/src-tauri/target/universal-apple-darwin/release/bundle/macos/Kosh.app}"
packaged_executable="$packaged_app/Contents/MacOS/kosh"
require_packaged="${KOSH_R2_CANARY_REQUIRE_PACKAGED:-0}"

fail() {
  echo "Kosh real-R2 canary failed: $*" >&2
  exit 1
}

for command in cargo jq shasum uuidgen; do
  command -v "$command" >/dev/null 2>&1 || fail "required command not found: $command"
done
[[ "$(uname -s)" == "Darwin" ]] || fail "the real-R2 canary requires macOS"

if [[ -z "${KOSH_LITESTREAM_R2_ACCOUNT_ID:-}" ]]; then
  [[ -r "$environment_file" ]] ||
    fail "credential file not found; copy app/.env.example to app/.env"
  [[ "$(stat -f '%Lp' "$environment_file")" == "600" ]] ||
    fail "the ignored credential file must be mode 0600"
  set -a
  # shellcheck disable=SC1090
  source "$environment_file"
  set +a
fi

for variable in \
  KOSH_LITESTREAM_R2_ACCOUNT_ID \
  KOSH_LITESTREAM_R2_JURISDICTION \
  KOSH_LITESTREAM_R2_BUCKET \
  KOSH_LITESTREAM_R2_ACCESS_KEY_ID \
  KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY; do
  [[ -n "${!variable:-}" ]] || fail "missing required variable: $variable"
done

if [[ ! -x "$staged_binary" ]]; then
  "$repo_root/scripts/stage-litestream-sidecar.sh" >/dev/null
fi
[[ -x "$staged_binary" ]] || fail "the pinned Litestream binary was not staged"

run_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
evidence_parent="$app_root/.data/r2-canary"
evidence_root="$evidence_parent/$run_id"
mkdir -p "$evidence_parent"
source_head="$(git -C "$repo_root" rev-parse HEAD)"
if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=normal)" ]]; then
  source_tree_state="DIRTY"
else
  source_tree_state="CLEAN"
fi
if [[ "$require_packaged" == "1" && "$source_tree_state" != "CLEAN" ]]; then
  fail "packaged recovery acceptance requires a clean exact-HEAD worktree"
fi

export KOSH_RUN_R2_CANARY=1
export KOSH_LITESTREAM_PATH="$staged_binary"
export KOSH_R2_CANARY_DATA_DIR="$evidence_root"
export KOSH_R2_CANARY_HEAD="$source_head"
export KOSH_R2_CANARY_TREE_STATE="$source_tree_state"

if [[ "$require_packaged" == "1" ]]; then
  [[ -x "$packaged_executable" ]] ||
    fail "packaged Kosh executable not found; run pnpm release:build:app"
  packaged_home="$evidence_root/packaged-home"
  packaged_data_parent="$packaged_home/Library/Application Support"
  export KOSH_R2_CANARY_PACKAGED_EXECUTABLE="$packaged_executable"
  export KOSH_R2_CANARY_PACKAGED_DATA_DIR="$packaged_data_parent/com.rohan.kosh"
fi

(
  cd "$app_root"
  cargo test \
    --locked \
    --manifest-path src-tauri/Cargo.toml \
    --features test-support \
    --lib \
    backup::canary::live_r2_canary_restores_complete_kosh_library_and_cleans_unique_backup_set \
    -- \
    --ignored \
    --exact \
    --nocapture \
    --test-threads=1
)

report="$evidence_root/canary-report-v1.json"
[[ -f "$report" ]] || fail "the canary did not produce its bounded report"
jq -e \
  --arg executionMode "$([[ "$require_packaged" == "1" ]] && printf PACKAGED || printf LIBRARY)" \
  '
    .schemaVersion == 1
    and .result == "PASSED"
    and .executionMode == $executionMode
    and (.sourceHead | test("^[0-9a-f]{40}$"; "i"))
    and (.sourceTreeState == "CLEAN" or .sourceTreeState == "DIRTY")
    and .interruptedReplicationRetry == "PASSED"
    and .immutableManifestPublishedLast == "PASSED"
    and .nonMutatingDrill == "PASSED"
    and .cleanDirectoryRestore == "PASSED"
    and .normalDatabaseReopen == "PASSED"
    and .searchRebuild == "PASSED"
    and .citationResolution == "PASSED"
    and .researchCitationResolution == "PASSED"
    and .restored.activeTidbits >= 2
    and .restored.revisions >= 3
    and .restored.attachments >= 1
    and .restored.mediaBlobs >= 1
    and .restored.researchCitations >= 1
    and .restored.historicalResearchCitations >= 1
    and .restored.interruptedReplicationDrafts == 1
    and .removedRemoteObjects > 0
    and .remoteResidueObjects == 0
  ' "$report" >/dev/null || fail "the redacted canary report is incomplete"

if [[ "$require_packaged" == "1" ]]; then
  jq -e '.packagedRecoveryCommand == "PASSED"' "$report" >/dev/null ||
    fail "the packaged recovery command was not proved"
  node "$app_root/scripts/run-recovered-package-smoke.mjs" \
    "$packaged_app" \
    "$packaged_home" \
    "$evidence_root/packaged-recovery-smoke-v1.json"
fi

echo "Kosh real-R2 recovery canary passed"
echo "redacted evidence: $evidence_root"
echo "the unique backup-set prefix was removed and verified empty"
