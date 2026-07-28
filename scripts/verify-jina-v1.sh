#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
MANIFEST="$ROOT/app/src-tauri/resources/embedding-indexes/jina-v1.json"
FIXTURES="$ROOT/app/src-tauri/resources/embedding-indexes/jina-v1-golden.json"
MODEL_FILE=${1:-"$HOME/Library/Application Support/kosh/models/v5-nano-retrieval-Q8_0.gguf"}
LLAMA_EMBEDDING=${LLAMA_EMBEDDING:-llama-embedding}
LLAMA_SERVER=${LLAMA_SERVER:-llama-server}
LLAMA_DEVICE=${LLAMA_DEVICE:-none}
LLAMA_GPU_LAYERS=${LLAMA_GPU_LAYERS:-0}
LLAMA_REQUIRE_METAL=${LLAMA_REQUIRE_METAL:-0}

case "$LLAMA_REQUIRE_METAL" in
  0 | 1) ;;
  *)
    echo "LLAMA_REQUIRE_METAL must be 0 or 1" >&2
    exit 1
    ;;
esac

for command in curl jq shasum "$LLAMA_EMBEDDING" "$LLAMA_SERVER"; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "required command not found: $command" >&2
    exit 1
  }
done

test -f "$MODEL_FILE" || {
  echo "model not found: $MODEL_FILE" >&2
  echo "download and verify it using the commands in README.md" >&2
  exit 1
}

if test "$LLAMA_REQUIRE_METAL" = 1; then
  case "$LLAMA_DEVICE" in
    MTL[0-9]*) ;;
    *)
      echo "Metal verification requires an explicit MTL device, found: $LLAMA_DEVICE" >&2
      exit 1
      ;;
  esac

  "$LLAMA_SERVER" --list-devices 2>&1 \
    | grep -F "  $LLAMA_DEVICE:" >/dev/null || {
      echo "Metal device is unavailable: $LLAMA_DEVICE" >&2
      exit 1
    }
fi

EXPECTED_FILE=$(jq -r '.config.modelFile' "$MANIFEST")
EXPECTED_SIZE=$(jq -r '.config.modelFileSize' "$MANIFEST")
EXPECTED_SHA=$(jq -r '.modelFileSha256' "$MANIFEST")
FIXTURE_SHA=$(jq -r '.modelFileSha256' "$FIXTURES")
ACTUAL_SIZE=$(wc -c <"$MODEL_FILE" | tr -d ' ')

test "$FIXTURE_SHA" = "$EXPECTED_SHA" || {
  echo "fixture model hash does not match the canonical manifest" >&2
  exit 1
}
test "$ACTUAL_SIZE" = "$EXPECTED_SIZE" || {
  echo "unexpected size for $EXPECTED_FILE: $ACTUAL_SIZE" >&2
  exit 1
}
printf '%s  %s\n' "$EXPECTED_SHA" "$MODEL_FILE" | shasum -a 256 -c -

TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/kosh-jina-v1.XXXXXX")
SERVER_PID=

cleanup() {
  if test -n "$SERVER_PID"; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

verify_output() {
  name=$1
  output=$2
  label=$3

  jq -e --arg name "$name" --slurpfile actual "$output" '
    def vector_norm: map(. * .) | add | sqrt;
    def dot($left; $right):
      reduce range(0; $left | length) as $index
        (0; . + ($left[$index] * $right[$index]));
    def max_absolute_difference($left; $right):
      [range(0; $left | length) as $index
        | (($left[$index] - $right[$index]) | fabs)] | max;

    .tolerance as $tolerance
    | (.cases[] | select(.name == $name) | .embedding) as $expected
    | $actual[0].data[0].embedding as $observed
    | ($observed | length) == 768
      and (($observed | vector_norm) - 1.0 | fabs) <= 0.00001
      and (dot($expected; $observed) >= $tolerance.minimumCosineSimilarity)
      and (max_absolute_difference($expected; $observed) <= $tolerance.maximumAbsoluteDifference)
  ' "$FIXTURES" >/dev/null
  echo "$name fixture passed through $label"
}

verify_metal_offload() {
  log=$1
  label=$2

  test "$LLAMA_REQUIRE_METAL" = 1 || return 0

  grep -F "using device $LLAMA_DEVICE " "$log" >/dev/null || {
    cat "$log" >&2
    echo "$label did not select Metal device $LLAMA_DEVICE" >&2
    exit 1
  }

  grep -F 'ggml_metal_init: found device:' "$log" >/dev/null || {
    cat "$log" >&2
    echo "$label did not initialize Metal" >&2
    exit 1
  }

  offload_counts=$(
    sed -n 's/.*offloaded \([0-9][0-9]*\)\/\([0-9][0-9]*\) layers to GPU.*/\1 \2/p' "$log" \
      | tail -n 1
  )
  offloaded_layers=${offload_counts%% *}
  total_layers=${offload_counts##* }
  if test -z "$offload_counts" \
    || test "$offloaded_layers" -le 0 \
    || test "$offloaded_layers" -ne "$total_layers"; then
    cat "$log" >&2
    echo "$label did not offload every model layer to Metal" >&2
    exit 1
  fi

  echo "$label verified Metal offload on $LLAMA_DEVICE"
}

run_fixture() {
  name=$1
  prompt=$(jq -r --arg name "$name" '.cases[] | select(.name == $name) | .input' "$FIXTURES")
  output="$TMP_DIR/$name.json"
  log="$TMP_DIR/$name.log"

  set -- \
    --model "$MODEL_FILE" \
    --pooling last \
    --embd-normalize 2 \
    --embd-output-format json
  if test "$LLAMA_DEVICE" != auto; then
    set -- "$@" --device "$LLAMA_DEVICE"
  fi
  set -- "$@" \
    --n-gpu-layers "$LLAMA_GPU_LAYERS" \
    --seed 0 \
    --prompt "$prompt"
  if test "$LLAMA_REQUIRE_METAL" = 1; then
    set -- "$@" --verbose
  fi
  if ! "$LLAMA_EMBEDDING" "$@" >"$output" 2>"$log"; then
    cat "$log" >&2
    exit 1
  fi

  verify_metal_offload "$log" "llama-embedding $name"
  verify_output "$name" "$output" llama-embedding
}

run_fixture query
run_fixture document

PORT=${KOSH_JINA_TEST_PORT:-$((40000 + ($$ % 20000)))}
SERVER_URL="http://127.0.0.1:$PORT"
SERVER_LOG="$TMP_DIR/llama-server.log"
set -- \
  --model "$MODEL_FILE" \
  --embedding \
  --pooling last \
  --embd-normalize 2
if test "$LLAMA_DEVICE" != auto; then
  set -- "$@" --device "$LLAMA_DEVICE"
fi
set -- "$@" \
  --n-gpu-layers "$LLAMA_GPU_LAYERS" \
  --parallel 1 \
  --host 127.0.0.1 \
  --port "$PORT"
if test "$LLAMA_REQUIRE_METAL" = 1; then
  set -- "$@" --verbose
fi
"$LLAMA_SERVER" "$@" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

attempt=0
until curl -fsS --connect-timeout 1 "$SERVER_URL/health" >/dev/null 2>&1; do
  if ! kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    cat "$SERVER_LOG" >&2
    echo "llama-server exited before becoming healthy" >&2
    exit 1
  fi
  attempt=$((attempt + 1))
  if test "$attempt" -ge 100; then
    cat "$SERVER_LOG" >&2
    echo "llama-server did not become healthy" >&2
    exit 1
  fi
  sleep 0.1
done

SERVER_PROMPT=$(jq -r '.cases[] | select(.name == "query") | .input' "$FIXTURES")
jq -nc --arg input "$SERVER_PROMPT" '{input: $input}' \
  | curl -fsS \
      -H 'Content-Type: application/json' \
      --data-binary @- \
      "$SERVER_URL/v1/embeddings" \
      >"$TMP_DIR/server-query.json"
verify_metal_offload "$SERVER_LOG" llama-server
verify_output query "$TMP_DIR/server-query.json" llama-server

kill "$SERVER_PID" >/dev/null 2>&1 || true
wait "$SERVER_PID" >/dev/null 2>&1 || true
SERVER_PID=

echo "Jina v1 artifact and embedding fixtures passed"
