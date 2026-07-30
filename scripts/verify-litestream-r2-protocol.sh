#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
APP_ROOT="$ROOT/app"
PIN="$APP_ROOT/src-tauri/resources/sidecars/litestream-v1.json"
BINARY=${KOSH_LITESTREAM_BINARY:-"$APP_ROOT/src-tauri/resources/release/bin/litestream"}
ENV_FILE=${KOSH_LITESTREAM_ENV_FILE:-"$APP_ROOT/.env"}
VERIFICATION_ROOT="$APP_ROOT/.data/litestream-r2-protocol"
daemon_pid=
remote_created=0
remote_path=
endpoint=
run_root=

main() {
  for command in aws grep jq shasum sqlite3 stat uuidgen; do
    command -v "$command" >/dev/null 2>&1 || {
      echo "required command not found: $command" >&2
      exit 1
    }
  done
  test "$(uname -s)" = Darwin || {
    echo "the Litestream R2 protocol verifier requires macOS" >&2
    exit 1
  }
  test -x "$BINARY" || {
    echo "staged Litestream binary not found; run pnpm release:stage-litestream" >&2
    exit 1
  }
  test -r "$ENV_FILE" || {
    echo "Litestream environment file not found; copy app/.env.example to app/.env" >&2
    exit 1
  }
  test "$(stat -f '%Lp' "$ENV_FILE")" = 600 || {
    echo "Litestream environment file must be mode 0600" >&2
    exit 1
  }

  set -a
  # shellcheck disable=SC1090
  . "$ENV_FILE"
  set +a
  validate_environment
  endpoint=$(r2_endpoint)

  expected_version=$(jq -er --arg architecture "$(uname -m)" \
    '.binary.versionOutputByArchitecture[$architecture]' "$PIN")
  test "$("$BINARY" version)" = "$expected_version" || {
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
  log="$run_root/litestream.log"
  remote_path="${KOSH_LITESTREAM_R2_PREFIX%/}/runs/$run_id/kosh.sqlite3"

  mkdir -p "$run_root/restores" "$runtime_root"
  chmod 700 "$run_root" "$run_root/restores" "$runtime_root"
  render_r2_config "$config" "$socket" "$database" "$remote_path" "$endpoint"
  chmod 600 "$config"

  sqlite3 "$database" \
    "PRAGMA journal_mode=WAL;
     PRAGMA synchronous=FULL;
     CREATE TABLE fence_events(sequence INTEGER PRIMARY KEY, marker TEXT NOT NULL UNIQUE);
     INSERT INTO fence_events(marker) VALUES('baseline');" \
    >/dev/null

  remote_created=1
  "$BINARY" replicate -config "$config" >"$log" 2>&1 &
  daemon_pid=$!
  wait_for_socket "$daemon_pid" "$socket" "$log"

  sqlite3 "$database" "INSERT INTO fence_events(marker) VALUES('inside-fence');"
  local_sync=$("$BINARY" sync -json -socket "$socket" "$database")
  local_txid=$(printf '%s' "$local_sync" | jq -er \
    --arg database "$database" \
    'select(.db_path == $database and (.replica_txid == null)) | .txid')
  fenced_txid=$(printf '%016x' "$local_txid")

  sqlite3 "$database" "INSERT INTO fence_events(marker) VALUES('after-fence');"
  remote_sync=$("$BINARY" sync -wait -timeout 60 -json -socket "$socket" "$database")
  replica_txid=$(printf '%s' "$remote_sync" | jq -er \
    --arg database "$database" \
    'select(.db_path == $database and .replica_txid == .txid) | .replica_txid')
  test "$replica_txid" -ge "$local_txid" || {
    echo "R2 replica did not reach the fenced transaction" >&2
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

  compaction_deadline=$((SECONDS + 120))
  while :; do
    ltx_json=$("$BINARY" ltx -config "$config" -level all -json "$database")
    if printf '%s' "$ltx_json" |
      jq -e --arg txid "$fenced_txid" \
        'any(.[]; .level >= 1 and .min_txid <= $txid and .max_txid >= $txid)' \
        >/dev/null; then
      break
    fi
    test "$SECONDS" -lt "$compaction_deadline" || {
      echo "timed out waiting for ordinary R2 compaction" >&2
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

  sqlite3 "$database" "INSERT INTO fence_events(marker) VALUES('shutdown-only-sync');"
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

  cleanup_remote
  remote_created=0

  jq -n \
    --arg runId "$run_id" \
    --arg remotePath "$remote_path" \
    --arg fencedTxid "$fenced_txid" \
    --argjson remoteTxid "$replica_txid" \
    --arg binarySha256 "$(shasum -a 256 "$BINARY" | awk '{print $1}')" \
    '{
      schemaVersion: 1,
      runId: $runId,
      replicaType: "r2",
      remotePath: $remotePath,
      fencedTxid: $fencedTxid,
      remoteConfirmedTxid: $remoteTxid,
      exactFenceRestore: "PASSED",
      postCompactionExactRestore: "PASSED",
      requiredL0Retention: "720h",
      gracefulShutdownFinalSync: "PASSED",
      orphanProcess: false,
      remoteResidueObjects: 0,
      binarySha256: $binarySha256
    }' >"$run_root/report.json"
  chmod 600 "$run_root/report.json"

  echo "Litestream R2 protocol verification passed"
  echo "evidence: $run_root/report.json"
  echo "remote test prefix removed"
}

cleanup() {
  exit_status=$?
  trap - EXIT INT TERM
  stop_process "$daemon_pid"
  if test "$remote_created" = 1; then
    cleanup_remote || {
      echo "warning: could not remove the unique R2 protocol prefix" >&2
      test "$exit_status" -ne 0 || exit_status=1
    }
  fi
  exit "$exit_status"
}

stop_process() {
  process_id=$1
  if test -n "$process_id" && kill -0 "$process_id" 2>/dev/null; then
    kill -TERM "$process_id" 2>/dev/null || true
    wait "$process_id" 2>/dev/null || true
  fi
}

validate_environment() {
  for variable in \
    KOSH_LITESTREAM_R2_ACCOUNT_ID \
    KOSH_LITESTREAM_R2_JURISDICTION \
    KOSH_LITESTREAM_R2_BUCKET \
    KOSH_LITESTREAM_R2_PREFIX \
    KOSH_LITESTREAM_R2_ACCESS_KEY_ID \
    KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY
  do
    eval "value=\${$variable:-}"
    test -n "$value" || {
      echo "missing $variable in the ignored Litestream environment file" >&2
      exit 1
    }
  done

  printf '%s' "$KOSH_LITESTREAM_R2_ACCOUNT_ID" |
    grep -Eq '^[a-f0-9]{32}$' || {
    echo "invalid Cloudflare account ID" >&2
    exit 1
  }
  printf '%s' "$KOSH_LITESTREAM_R2_BUCKET" |
    grep -Eq '^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$' || {
    echo "invalid R2 bucket name" >&2
    exit 1
  }
  case "$KOSH_LITESTREAM_R2_PREFIX" in
    /* | */ | *'..'* | *//* | *\\* | *\?* | *\#*)
      echo "invalid confined R2 prefix" >&2
      exit 1
      ;;
  esac
  printf '%s' "$KOSH_LITESTREAM_R2_PREFIX" |
    grep -Eq '^[A-Za-z0-9._/-]{1,255}$' || {
    echo "invalid confined R2 prefix" >&2
    exit 1
  }
}

r2_endpoint() {
  case "$KOSH_LITESTREAM_R2_JURISDICTION" in
    DEFAULT)
      printf 'https://%s.r2.cloudflarestorage.com\n' \
        "$KOSH_LITESTREAM_R2_ACCOUNT_ID"
      ;;
    EU)
      printf 'https://%s.eu.r2.cloudflarestorage.com\n' \
        "$KOSH_LITESTREAM_R2_ACCOUNT_ID"
      ;;
    FEDRAMP)
      printf 'https://%s.fedramp.r2.cloudflarestorage.com\n' \
        "$KOSH_LITESTREAM_R2_ACCOUNT_ID"
      ;;
    *)
      echo "unsupported R2 jurisdiction" >&2
      exit 1
      ;;
  esac
}

render_r2_config() {
  output=$1
  socket_path=$2
  database_path=$3
  replica_path=$4
  endpoint_url=$5

  jq -n \
    --arg socket "$socket_path" \
    --arg database "$database_path" \
    --arg bucket "$KOSH_LITESTREAM_R2_BUCKET" \
    --arg replicaPath "$replica_path" \
    --arg endpoint "$endpoint_url" \
    '{
      logging: {level: "info", type: "json", stderr: true},
      socket: {enabled: true, path: $socket, permissions: 384},
      "sync-interval": "5s",
      "verify-compaction": true,
      "auto-recover": false,
      "l0-retention": "720h",
      "l0-retention-check-interval": "1m",
      "shutdown-sync-timeout": "30s",
      "shutdown-sync-interval": "500ms",
      snapshot: {interval: "6h", retention: "720h"},
      validation: {interval: "6h"},
      dbs: [{
        path: $database,
        "monitor-interval": "1s",
        "checkpoint-interval": "1m",
        replica: {
          type: "s3",
          bucket: $bucket,
          path: $replicaPath,
          endpoint: $endpoint,
          region: "auto",
          "access-key-id": "${KOSH_LITESTREAM_R2_ACCESS_KEY_ID}",
          "secret-access-key": "${KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY}",
          "force-path-style": false,
          "sync-interval": "5s"
        }
      }]
    }' >"$output"
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
    'select(.replica == "s3" and .txid == $txid and .integrity_check == "full")' \
    "$result_path" >/dev/null
}

cleanup_remote() {
  test -n "$remote_path" || return 0
  AWS_ACCESS_KEY_ID="$KOSH_LITESTREAM_R2_ACCESS_KEY_ID" \
  AWS_SECRET_ACCESS_KEY="$KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY" \
  AWS_DEFAULT_REGION=auto \
    aws --endpoint-url "$endpoint" s3 rm \
      "s3://$KOSH_LITESTREAM_R2_BUCKET/$remote_path" \
      --recursive --only-show-errors
  remaining=$(
    AWS_ACCESS_KEY_ID="$KOSH_LITESTREAM_R2_ACCESS_KEY_ID" \
    AWS_SECRET_ACCESS_KEY="$KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY" \
    AWS_DEFAULT_REGION=auto \
      aws --endpoint-url "$endpoint" s3api list-objects-v2 \
        --bucket "$KOSH_LITESTREAM_R2_BUCKET" \
        --prefix "$remote_path" \
        --query 'Contents[].Key' \
        --output text
  )
  test -z "$remaining" || test "$remaining" = None || {
    echo "the unique R2 protocol prefix still contains objects" >&2
    return 1
  }
}

trap cleanup EXIT INT TERM
main "$@"
