#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
APP_ROOT="$ROOT/app"
PIN="$APP_ROOT/src-tauri/resources/sidecars/litestream-v1.json"
BINARY=${KOSH_LITESTREAM_BINARY:-"$APP_ROOT/src-tauri/resources/release/bin/litestream"}
VERIFICATION_ROOT="$APP_ROOT/.data/litestream-protocol"

main() {
for command in jq sqlite3 stat uuidgen; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "required command not found: $command" >&2
    exit 1
  }
done

test "$(uname -s)" = Darwin || {
  echo "the Litestream protocol verifier requires macOS" >&2
  exit 1
}
test -x "$BINARY" || {
  echo "staged Litestream binary not found; run pnpm release:stage-litestream" >&2
  exit 1
}

expected_version=$(jq -er --arg architecture "$(uname -m)" \
  '.binary.versionOutputByArchitecture[$architecture]' "$PIN")
actual_version=$("$BINARY" version)
test "$actual_version" = "$expected_version" || {
  echo "staged Litestream version mismatch" >&2
  exit 1
}

run_id=$(uuidgen | tr '[:upper:]' '[:lower:]')
short_id=${run_id%%-*}
run_root="$VERIFICATION_ROOT/runs/$run_id"
runtime_root="$VERIFICATION_ROOT/rt/$short_id"
database="$run_root/kosh.sqlite3"
config="$run_root/litestream.yml"
socket="$runtime_root/ls.sock"
replica="$run_root/replica"
log="$run_root/litestream.log"
daemon_pid=
expiry_daemon_pid=

mkdir -p "$run_root/restores" "$runtime_root" "$replica"
chmod 700 "$run_root" "$run_root/restores" "$runtime_root" "$replica"

stop_process() {
  process_id=$1
  if test -n "$process_id" && kill -0 "$process_id" 2>/dev/null; then
    kill -TERM "$process_id" 2>/dev/null || true
    wait "$process_id" 2>/dev/null || true
  fi
}

cleanup() {
  stop_process "$daemon_pid"
  stop_process "$expiry_daemon_pid"
}
trap cleanup EXIT INT TERM

render_file_config "$config" "$socket" "$database" "$replica" "720h" "1m"
chmod 600 "$config"

sqlite3 "$database" \
  "PRAGMA journal_mode=WAL;
   PRAGMA synchronous=FULL;
   CREATE TABLE fence_events(sequence INTEGER PRIMARY KEY, marker TEXT NOT NULL UNIQUE);
   INSERT INTO fence_events(marker) VALUES('baseline');" \
  >/dev/null

"$BINARY" replicate -config "$config" >"$log" 2>&1 &
daemon_pid=$!
wait_for_socket "$daemon_pid" "$socket" "$log"

sqlite_write "$database" "INSERT INTO fence_events(marker) VALUES('inside-fence');"
local_sync=$("$BINARY" sync -json -socket "$socket" "$database")
local_txid=$(printf '%s' "$local_sync" | jq -er \
  --arg database "$database" \
  'select(.db_path == $database and (.replica_txid == null)) | .txid')
fenced_txid=$(printf '%016x' "$local_txid")

sqlite_write "$database" "INSERT INTO fence_events(marker) VALUES('after-fence');"
remote_sync=$("$BINARY" sync -wait -timeout 60 -json -socket "$socket" "$database")
replica_txid=$(printf '%s' "$remote_sync" | jq -er \
  --arg database "$database" \
  'select(.db_path == $database and .replica_txid == .txid) | .replica_txid')
test "$replica_txid" -ge "$local_txid" || {
  echo "remote replica did not reach the fenced transaction" >&2
  exit 1
}

"$BINARY" restore \
  -config "$config" \
  -txid "$fenced_txid" \
  -dry-run \
  -json \
  -o "$run_root/restores/pre-compaction.sqlite3" \
  "$database" \
  >"$run_root/restores/pre-compaction-plan.json"

"$BINARY" restore \
  -config "$config" \
  -txid "$fenced_txid" \
  -json \
  -integrity-check full \
  -o "$run_root/restores/pre-compaction.sqlite3" \
  "$database" \
  >"$run_root/restores/pre-compaction-result.json"

assert_exact_fence_restore "$run_root/restores/pre-compaction.sqlite3"
assert_restore_result "$run_root/restores/pre-compaction-result.json" "$fenced_txid"

compaction_deadline=$((SECONDS + 90))
while :; do
  ltx_json=$("$BINARY" ltx -config "$config" -level all -json "$database")
  if printf '%s' "$ltx_json" |
    jq -e --arg txid "$fenced_txid" \
      'any(.[]; .level >= 1 and .min_txid <= $txid and .max_txid >= $txid)' \
      >/dev/null; then
    break
  fi
  test "$SECONDS" -lt "$compaction_deadline" || {
    echo "timed out waiting for ordinary Litestream compaction" >&2
    exit 1
  }
  sleep 2
done

"$BINARY" restore \
  -config "$config" \
  -txid "$fenced_txid" \
  -json \
  -integrity-check full \
  -o "$run_root/restores/post-compaction.sqlite3" \
  "$database" \
  >"$run_root/restores/post-compaction-result.json"
assert_exact_fence_restore "$run_root/restores/post-compaction.sqlite3"
assert_restore_result "$run_root/restores/post-compaction-result.json" "$fenced_txid"

sqlite_write "$database" "INSERT INTO fence_events(marker) VALUES('shutdown-only-sync');"
kill -TERM "$daemon_pid"
wait "$daemon_pid"
stopped_pid=$daemon_pid
daemon_pid=
if kill -0 "$stopped_pid" 2>/dev/null; then
  echo "Litestream child survived graceful shutdown" >&2
  exit 1
fi

"$BINARY" restore \
  -config "$config" \
  -json \
  -integrity-check full \
  -o "$run_root/restores/shutdown-latest.sqlite3" \
  "$database" \
  >"$run_root/restores/shutdown-result.json"
test "$(sqlite3 "$run_root/restores/shutdown-latest.sqlite3" \
  "SELECT COUNT(*) FROM fence_events WHERE marker='shutdown-only-sync'")" = 1

expiry_root="$run_root/l0-expiry"
expiry_runtime="$VERIFICATION_ROOT/rt/${short_id}e"
expiry_database="$expiry_root/kosh.sqlite3"
expiry_config="$expiry_root/litestream.yml"
expiry_socket="$expiry_runtime/ls.sock"
expiry_replica="$expiry_root/replica"
expiry_log="$expiry_root/litestream.log"
mkdir -p "$expiry_root/restores" "$expiry_runtime" "$expiry_replica"
chmod 700 "$expiry_root" "$expiry_root/restores" "$expiry_runtime" "$expiry_replica"
render_file_config "$expiry_config" "$expiry_socket" "$expiry_database" \
  "$expiry_replica" "1s" "1s"
chmod 600 "$expiry_config"

sqlite3 "$expiry_database" \
  "PRAGMA journal_mode=WAL;
   PRAGMA synchronous=FULL;
   CREATE TABLE expiry_events(sequence INTEGER PRIMARY KEY, marker TEXT NOT NULL UNIQUE);
   INSERT INTO expiry_events(marker) VALUES('baseline');" \
  >/dev/null
"$BINARY" replicate -config "$expiry_config" >"$expiry_log" 2>&1 &
expiry_daemon_pid=$!
wait_for_socket "$expiry_daemon_pid" "$expiry_socket" "$expiry_log"

interior_txid=
sequence=1
while test "$sequence" -le 16; do
  sqlite_write "$expiry_database" \
    "INSERT INTO expiry_events(marker) VALUES('event-$sequence');"
  sync_json=$("$BINARY" sync -json -socket "$expiry_socket" "$expiry_database")
  current_txid=$(printf '%s' "$sync_json" | jq -er '.txid')
  if test "$sequence" -eq 5; then
    interior_txid=$(printf '%016x' "$current_txid")
  fi
  sequence=$((sequence + 1))
done
"$BINARY" sync -wait -timeout 60 -json -socket "$expiry_socket" \
  "$expiry_database" >/dev/null

expiry_deadline=$((SECONDS + 120))
while :; do
  expiry_ltx=$("$BINARY" ltx -config "$expiry_config" -level all -json "$expiry_database")
  has_interior_compaction=$(
    printf '%s' "$expiry_ltx" |
      jq -r --arg txid "$interior_txid" \
        'any(.[]; .level >= 1 and .min_txid < $txid and .max_txid > $txid)'
  )
  has_exact_l0=$(
    printf '%s' "$expiry_ltx" |
      jq -r --arg txid "$interior_txid" \
        'any(.[]; .level == 0 and .min_txid <= $txid and .max_txid >= $txid)'
  )
  if test "$has_interior_compaction" = true && test "$has_exact_l0" = false; then
    break
  fi
  test "$SECONDS" -lt "$expiry_deadline" || {
    echo "timed out proving accelerated L0 expiry" >&2
    exit 1
  }
  sleep 2
done

if "$BINARY" restore \
  -config "$expiry_config" \
  -txid "$interior_txid" \
  -json \
  -integrity-check full \
  -o "$expiry_root/restores/interior.sqlite3" \
  "$expiry_database" \
  >"$expiry_root/restores/interior-result.json" \
  2>"$expiry_root/restores/interior-error.log"; then
  echo "Litestream unexpectedly restored an interior TXID after its L0 expired" >&2
  exit 1
fi

kill -TERM "$expiry_daemon_pid"
wait "$expiry_daemon_pid"
expiry_stopped_pid=$expiry_daemon_pid
expiry_daemon_pid=
if kill -0 "$expiry_stopped_pid" 2>/dev/null; then
  echo "accelerated-expiry Litestream child survived shutdown" >&2
  exit 1
fi

jq -n \
  --arg runId "$run_id" \
  --arg fencedTxid "$fenced_txid" \
  --argjson remoteTxid "$replica_txid" \
  --arg interiorExpiredTxid "$interior_txid" \
  --arg binarySha256 "$(shasum -a 256 "$BINARY" | awk '{print $1}')" \
  '{
    schemaVersion: 1,
    runId: $runId,
    replicaType: "file",
    fencedTxid: $fencedTxid,
    remoteConfirmedTxid: $remoteTxid,
    exactFenceRestore: "PASSED",
    postCompactionExactRestore: "PASSED",
    defaultL0ExpiryInteriorTxidFailureObserved: "PASSED",
    interiorExpiredTxid: $interiorExpiredTxid,
    requiredL0Retention: "720h",
    gracefulShutdownFinalSync: "PASSED",
    orphanProcess: false,
    binarySha256: $binarySha256
  }' >"$run_root/report.json"
chmod 600 "$run_root/report.json"

echo "Litestream local protocol verification passed"
echo "evidence: $run_root/report.json"
}

render_file_config() {
  output=$1
  socket_path=$2
  database_path=$3
  replica_path=$4
  l0_retention=$5
  l0_retention_check_interval=$6

  jq -n \
    --arg socket "$socket_path" \
    --arg database "$database_path" \
    --arg replica "$replica_path" \
    --arg retention "$l0_retention" \
    --arg retentionCheck "$l0_retention_check_interval" \
    '{
      logging: {level: "info", type: "json", stderr: true},
      socket: {enabled: true, path: $socket, permissions: 384},
      "sync-interval": "1s",
      "verify-compaction": true,
      "auto-recover": false,
      "l0-retention": $retention,
      "l0-retention-check-interval": $retentionCheck,
      "shutdown-sync-timeout": "30s",
      "shutdown-sync-interval": "500ms",
      snapshot: {interval: "6h", retention: "720h"},
      validation: {interval: "6h"},
      dbs: [{
        path: $database,
        "monitor-interval": "1s",
        "checkpoint-interval": "1m",
        replica: {
          type: "file",
          path: $replica,
          "sync-interval": "1s"
        }
      }]
    }' >"$output"
}

sqlite_write() {
  database_path=$1
  statement=$2
  sqlite3 -cmd '.timeout 30000' "$database_path" "$statement"
}

wait_for_socket() {
  process_id=$1
  socket_path=$2
  process_log=$3
  socket_deadline=$((SECONDS + 20))
  while test ! -S "$socket_path"; do
    kill -0 "$process_id" 2>/dev/null || {
      echo "Litestream exited before creating its control socket; see $process_log" >&2
      exit 1
    }
    test "$SECONDS" -lt "$socket_deadline" || {
      echo "timed out waiting for Litestream control socket; see $process_log" >&2
      exit 1
    }
    sleep 1
  done
  test "$(stat -f '%Lp' "$socket_path")" = 600 || {
    echo "Litestream control socket is not mode 0600" >&2
    exit 1
  }
}

assert_exact_fence_restore() {
  restored_database=$1
  test "$(sqlite3 "$restored_database" \
    "SELECT COUNT(*) FROM fence_events WHERE marker='inside-fence'")" = 1
  test "$(sqlite3 "$restored_database" \
    "SELECT COUNT(*) FROM fence_events WHERE marker='after-fence'")" = 0
}

assert_restore_result() {
  result_path=$1
  expected_txid=$2
  jq -e --arg txid "$expected_txid" \
    'select(.replica == "file" and .txid == $txid and .integrity_check == "full")' \
    "$result_path" >/dev/null
}

main "$@"
