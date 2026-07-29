#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
runtime_gate="$repo_root/scripts/loop/runtime-gate.sh"
temp_dir="$(mktemp -d)"
temp_dir="$(cd "$temp_dir" && pwd -P)"
trap 'rm -rf "$temp_dir"' EXIT

readonly head_sha="0123456789abcdef0123456789abcdef01234567"

cat >"$temp_dir/git" <<'FAKE_GIT'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "-C" ]]; then
  shift 2
fi
case "${1:-}" in
  "rev-parse")
    if [[ "${2:-}" == "--show-toplevel" ]]; then
      echo "$FAKE_REPO_ROOT"
    else
      echo "$FAKE_HEAD_SHA"
    fi
    ;;
  "status")
    ;;
  *)
    echo "unexpected fake git invocation: $*" >&2
    exit 2
    ;;
esac
FAKE_GIT
chmod +x "$temp_dir/git"

cat >"$temp_dir/cargo" <<'FAKE_CARGO'
#!/usr/bin/env bash
set -euo pipefail

data="$KOSH_DATA_DIR"
receipt="$KOSH_STARTUP_SMOKE_RECEIPT"
expectation="$KOSH_STARTUP_SMOKE_EXPECT"
canary="$data/.fake-canary"
mkdir -p "$data" "$(dirname "$receipt")"

if [[ -n "${FAKE_FAIL_STAGE_MARKER:-}" \
  && "$data" == */bootstrap-incomplete \
  && ! -f "$FAKE_FAIL_STAGE_MARKER" ]]; then
  : >"$data/partial-bootstrap"
  : >"$FAKE_FAIL_STAGE_MARKER"
  exit 17
fi
if [[ -n "${FAKE_FAIL_PROMOTED_MARKER:-}" \
  && "$data" == */progressive-profile/data \
  && "$expectation" == "present" \
  && ! -f "$FAKE_FAIL_PROMOTED_MARKER" ]]; then
  : >"$FAKE_FAIL_PROMOTED_MARKER"
  exit 18
fi

preexisting=false
created=false
if [[ -f "$canary" ]]; then
  preexisting=true
  [[ "$expectation" != "absent" ]] || exit 19
else
  [[ "$expectation" != "present" ]] || exit 20
  : >"$canary"
  created=true
fi
: >"$data/kosh.sqlite3"
: >"$data/media.sqlite3"
canonical_data="$(cd "$data" && pwd -P)"

jq -n \
  --arg head "$KOSH_STARTUP_SMOKE_HEAD" \
  --arg expectation "$expectation" \
  --arg data "$canonical_data" \
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
    webviews: [
      {
        surface: "main",
        rendered: true,
        documentReadyState: "complete",
        rootChildCount: 1,
        frontendOrigin: "http://127.0.0.1:1420",
        probeDataDir: $data,
        probeRequestId: "00000000-0000-7000-8000-000000000004"
      },
      {
        surface: "quick-add",
        rendered: true,
        documentReadyState: "complete",
        rootChildCount: 1,
        frontendOrigin: "http://127.0.0.1:1420",
        probeDataDir: $data,
        probeRequestId: "00000000-0000-7000-8000-000000000005"
      }
    ],
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
      tidbitId: "00000000-0000-7000-8000-000000000001",
      revisionId: "00000000-0000-7000-8000-000000000002",
      passageId: "00000000-0000-7000-8000-000000000003",
      sourceUrl: "https://example.invalid/kosh-progressive-operability"
    }
  }' >"$receipt"
FAKE_CARGO
chmod +x "$temp_dir/cargo"

export GIT_BIN="$temp_dir/git"
export CARGO_BIN="$temp_dir/cargo"
export KOSH_RUNTIME_GATE_PLATFORM="Darwin"
export KOSH_RUNTIME_GATE_TESTING="true"
export FAKE_REPO_ROOT="$repo_root"
export FAKE_HEAD_SHA="$head_sha"

run_gate() {
  KOSH_LOOP_STATE_ROOT="$1" "$runtime_gate" --bootstrap-persistent
}

run_normal_gate() {
  KOSH_LOOP_STATE_ROOT="$1" "$runtime_gate"
}

stage_loop="$temp_dir/stage-interruption"
export FAKE_FAIL_STAGE_MARKER="$temp_dir/failed-stage-once"
if run_gate "$stage_loop" >"$temp_dir/stage-first.log" 2>&1; then
  echo "expected the first staged bootstrap to fail" >&2
  exit 1
fi
if [[ ! -f "$stage_loop/progressive-profile/bootstrap-owned.json" ]]; then
  sed -n '1,200p' "$temp_dir/stage-first.log" >&2
  echo "interrupted bootstrap lost its ownership record" >&2
  exit 1
fi
[[ -d "$stage_loop/progressive-profile/bootstrap-incomplete" ]] ||
  { echo "interrupted bootstrap lost its recoverable staging profile" >&2; exit 1; }
[[ ! -e "$stage_loop/progressive-profile/data" ]] ||
  { echo "failed staged bootstrap was promoted" >&2; exit 1; }
run_gate "$stage_loop" >/dev/null
[[ -f "$stage_loop/progressive-profile/established.json" ]] ||
  { echo "staged bootstrap retry was not established" >&2; exit 1; }
[[ -d "$stage_loop/progressive-profile/data" ]] ||
  { echo "staged bootstrap retry was not promoted" >&2; exit 1; }
[[ ! -e "$stage_loop/progressive-profile/bootstrap-owned.json" ]] ||
  { echo "successful staged bootstrap retained its owner record" >&2; exit 1; }
unset FAKE_FAIL_STAGE_MARKER

promoted_loop="$temp_dir/promoted-interruption"
export FAKE_FAIL_PROMOTED_MARKER="$temp_dir/failed-promoted-once"
if run_gate "$promoted_loop" >"$temp_dir/promoted-first.log" 2>&1; then
  echo "expected the first promoted-profile verification to fail" >&2
  exit 1
fi
if [[ ! -f "$promoted_loop/progressive-profile/bootstrap-owned.json" ]]; then
  sed -n '1,200p' "$temp_dir/promoted-first.log" >&2
  echo "promoted interruption lost its ownership record" >&2
  exit 1
fi
[[ -d "$promoted_loop/progressive-profile/data" ]] ||
  { echo "promoted interruption lost its canary profile" >&2; exit 1; }
[[ ! -e "$promoted_loop/progressive-profile/established.json" ]] ||
  { echo "failed promoted verification was marked established" >&2; exit 1; }
run_gate "$promoted_loop" >/dev/null
[[ -f "$promoted_loop/progressive-profile/established.json" ]] ||
  { echo "promoted-profile retry was not established" >&2; exit 1; }
[[ ! -e "$promoted_loop/progressive-profile/bootstrap-owned.json" ]] ||
  { echo "successful promoted retry retained its owner record" >&2; exit 1; }

promoted_marker="$promoted_loop/progressive-profile/established.json"
jq -e \
  '
    .schemaVersion == 2
    and (.canaryPassageId | type) == "string"
    and (.canaryPassageId | length) > 0
  ' \
  "$promoted_marker" >/dev/null ||
  { echo "new bootstrap did not preserve the canary passage" >&2; exit 1; }
jq \
  'del(.citationBaselineAtHead, .canaryPassageId) | .schemaVersion = 1' \
  "$promoted_marker" >"$promoted_marker.legacy"
mv "$promoted_marker.legacy" "$promoted_marker"
run_normal_gate "$promoted_loop" >/dev/null
jq -e \
  '
    .schemaVersion == 2
    and (.citationBaselineAtHead | type) == "string"
    and (.canaryPassageId | type) == "string"
  ' \
  "$promoted_marker" >/dev/null ||
  { echo "legacy marker did not upgrade without replacing its profile" >&2; exit 1; }

echo "runtime bootstrap recovery tests passed"
