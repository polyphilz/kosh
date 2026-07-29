#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "hardening report failed: $*" >&2
  exit 1
}

command -v git >/dev/null 2>&1 || fail "git is unavailable"
command -v jq >/dev/null 2>&1 || fail "jq is unavailable"
command -v pnpm >/dev/null 2>&1 || fail "pnpm is unavailable"
command -v cargo >/dev/null 2>&1 || fail "cargo is unavailable"

repo_root="$(git rev-parse --show-toplevel)"
app_root="$repo_root/app"
output="${KOSH_HARDENING_REPORT:-$app_root/.data/hardening/report-v1.json}"
lexical_report="$app_root/.data/relevance/reports/lexical-scale-v1.performance.json"
bundle_report="$app_root/test-results/bundle/report.json"
[[ "$output" == /* ]] || fail "report path must be absolute"
[[ ! -L "$output" ]] || fail "report path must not be a symlink"
mkdir -p "$(dirname "$output")"

head_sha="$(git -C "$repo_root" rev-parse HEAD)"
[[ -z "$(git -C "$repo_root" status --porcelain --untracked-files=normal)" ]] ||
  fail "commit-bound reporting requires a clean worktree"
[[ ! -e "$output" ]] || rm -- "$output"
started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
started_epoch="$(date +%s)"

"$repo_root/scripts/check-repository.sh"
(
  cd "$app_root"
  pnpm exec vitest run tests/security/hardening.test.ts
  pnpm test:browser:hardening
  pnpm test:browser:visual
)
cargo test \
  --locked \
  --manifest-path "$app_root/src-tauri/Cargo.toml" \
  --features test-support \
  mixed_local_workload
cargo test \
  --locked \
  --manifest-path "$app_root/src-tauri/Cargo.toml" \
  --features test-support \
  safety_snapshot
(
  cd "$app_root"
  pnpm relevance:lexical-scale
  pnpm check:bundle
)

[[ -f "$lexical_report" && ! -L "$lexical_report" ]] ||
  fail "lexical performance report is missing"
[[ -f "$bundle_report" && ! -L "$bundle_report" ]] ||
  fail "bundle report is missing"

completed_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
completed_epoch="$(date +%s)"
temporary="$output.$$.tmp"
jq -n \
  --arg headSha "$head_sha" \
  --arg startedAt "$started_at" \
  --arg completedAt "$completed_at" \
  --arg operatingSystem "$(uname -s)" \
  --arg architecture "$(uname -m)" \
  --arg nodeVersion "$(node --version)" \
  --arg pnpmVersion "$(pnpm --version)" \
  --arg rustVersion "$(rustc --version)" \
  --argjson durationSeconds "$((completed_epoch - started_epoch))" \
  --slurpfile lexical "$lexical_report" \
  --slurpfile bundle "$bundle_report" \
  '{
    schemaVersion: 1,
    result: "pass",
    headSha: $headSha,
    startedAt: $startedAt,
    completedAt: $completedAt,
    durationSeconds: $durationSeconds,
    runtime: {
      operatingSystem: $operatingSystem,
      architecture: $architecture,
      node: $nodeVersion,
      pnpm: $pnpmVersion,
      rust: $rustVersion
    },
    gates: {
      repositoryPolicy: "pass",
      securityContracts: "pass",
      browserAccessibilityAndVisual: "pass",
      mixedWorkloadAndRestart: "pass",
      safetySnapshotRestore: "pass",
      lexicalScale: "pass",
      bundleSafety: "pass"
    },
    lexicalScale: $lexical[0],
    bundle: $bundle[0]
  }' >"$temporary"
mv "$temporary" "$output"

echo "hardening report passed for $head_sha: $output"
