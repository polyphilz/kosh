#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
PIN="$ROOT/app/src-tauri/resources/sidecars/llama-server-v1.json"
EMBEDDING_MANIFEST="$ROOT/app/src-tauri/resources/embedding-indexes/jina-v1.json"
GOLDEN_FIXTURES="$ROOT/app/src-tauri/resources/embedding-indexes/jina-v1-golden.json"
VERIFY="$ROOT/scripts/verify-jina-v1.sh"
STAGE="$ROOT/app/src-tauri/resources/release"
MODEL_FILE=${1:-"$ROOT/models/v5-nano-retrieval-Q8_0.gguf"}

for command in arch cmake codesign curl git jq lipo otool shasum; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "required command not found: $command" >&2
    exit 1
  }
done

test "$(uname -s)" = Darwin || {
  echo "the v1 llama-server release build requires macOS" >&2
  exit 1
}

HOST_ARCHITECTURE=$(uname -m)
test "$HOST_ARCHITECTURE" = arm64 || {
  echo "the universal release must be built on Apple Silicon with Rosetta installed" >&2
  exit 1
}

test -f "$MODEL_FILE" || {
  echo "model not found: $MODEL_FILE" >&2
  echo "download the pinned model using the instructions in README.md" >&2
  exit 1
}

UPSTREAM_REPOSITORY=$(jq -er '.upstream.repository' "$PIN")
UPSTREAM_REVISION=$(jq -er '.upstream.revision' "$PIN")
BINARY_DESTINATION=$(jq -er '.resourceDestinations.binary' "$PIN")
BINARY_STAGING_PATH=$(jq -er '.stagingPaths.binary' "$PIN")
MANIFEST_STAGING_PATH=$(jq -er '.stagingPaths.releaseManifest' "$PIN")
LICENSE_SOURCE=$(jq -er '.licenseNotices[0].sourcePath' "$PIN")
LICENSE_STAGING_PATH=$(jq -er '.stagingPaths.license' "$PIN")

BUILD_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/kosh-llama-release.XXXXXX")
SOURCE="$BUILD_ROOT/llama.cpp"
UNIVERSAL="$BUILD_ROOT/universal"
STAGE_TEMP=

cleanup() {
  rm -rf "$BUILD_ROOT"
  if test -n "$STAGE_TEMP"; then
    rm -rf "$STAGE_TEMP"
  fi
}
trap cleanup EXIT INT TERM

git init -q "$SOURCE"
git -C "$SOURCE" remote add origin "$UPSTREAM_REPOSITORY"
git -C "$SOURCE" fetch -q --depth 1 origin "$UPSTREAM_REVISION"
git -C "$SOURCE" checkout -q --detach FETCH_HEAD

ACTUAL_REVISION=$(git -C "$SOURCE" rev-parse HEAD)
test "$ACTUAL_REVISION" = "$UPSTREAM_REVISION" || {
  echo "checked out $ACTUAL_REVISION instead of $UPSTREAM_REVISION" >&2
  exit 1
}

CMAKE_ARGUMENTS="$BUILD_ROOT/cmake-arguments.txt"
jq -er '.cmake.flags[]' "$PIN" >"$CMAKE_ARGUMENTS"
BUILD_JOBS=$(sysctl -n hw.logicalcpu 2>/dev/null || printf '1')

for architecture in $(jq -er '.target.architectures[]' "$PIN"); do
  build="$BUILD_ROOT/build-$architecture"
  set -- \
    -S "$SOURCE" \
    -B "$build" \
    "-DCMAKE_OSX_ARCHITECTURES=$architecture"
  while IFS= read -r argument; do
    set -- "$@" "$argument"
  done <"$CMAKE_ARGUMENTS"
  cmake "$@"
  cmake --build "$build" --config Release --parallel "$BUILD_JOBS" \
    --target llama-server llama-embedding
  test -x "$build/bin/llama-server"
  test -x "$build/bin/llama-embedding"
done

mkdir -p "$UNIVERSAL"
lipo -create \
  "$BUILD_ROOT/build-arm64/bin/llama-server" \
  "$BUILD_ROOT/build-x86_64/bin/llama-server" \
  -output "$UNIVERSAL/llama-server"
lipo -create \
  "$BUILD_ROOT/build-arm64/bin/llama-embedding" \
  "$BUILD_ROOT/build-x86_64/bin/llama-embedding" \
  -output "$UNIVERSAL/llama-embedding"
chmod 755 "$UNIVERSAL/llama-server" "$UNIVERSAL/llama-embedding"

LLAMA_SERVER="$UNIVERSAL/llama-server"
LLAMA_EMBEDDING="$UNIVERSAL/llama-embedding"
test -x "$LLAMA_SERVER"
test -x "$LLAMA_EMBEDDING"

for architecture in $(jq -er '.target.architectures[]' "$PIN"); do
  for binary in "$LLAMA_SERVER" "$LLAMA_EMBEDDING"; do
    lipo -archs "$binary" | tr ' ' '\n' | grep -Fx "$architecture" >/dev/null || {
      lipo -info "$binary" >&2
      echo "$(basename "$binary") is missing the $architecture slice" >&2
      exit 1
    }
  done
done

for architecture in $(jq -er '.target.architectures[]' "$PIN"); do
  if otool -arch "$architecture" -L "$LLAMA_SERVER" \
    | tail -n +2 \
    | grep -Ev '^[[:space:]]+(/System/Library/|/usr/lib/)' >/dev/null; then
    otool -arch "$architecture" -L "$LLAMA_SERVER" >&2
    echo "llama-server $architecture has a non-system dynamic dependency" >&2
    exit 1
  fi
done

VERSION_OUTPUTS="$BUILD_ROOT/version-outputs.json"
printf '{}\n' >"$VERSION_OUTPUTS"
METAL_DEVICE=MTL0
for architecture in $(jq -er '.target.architectures[]' "$PIN"); do
  if ! version_output=$(arch "-$architecture" "$LLAMA_SERVER" --version 2>&1); then
    echo "$version_output" >&2
    echo "could not execute the $architecture llama-server slice; install Rosetta if needed" >&2
    exit 1
  fi
  next_version_outputs="$VERSION_OUTPUTS.tmp"
  jq \
    --arg architecture "$architecture" \
    --arg output "$version_output" \
    '. + {($architecture): $output}' \
    "$VERSION_OUTPUTS" >"$next_version_outputs"
  mv "$next_version_outputs" "$VERSION_OUTPUTS"

  LLAMA_ARCHITECTURE="$architecture" \
  LLAMA_DEVICE=none \
  LLAMA_GPU_LAYERS=0 \
  LLAMA_EMBEDDING="$LLAMA_EMBEDDING" \
  LLAMA_SERVER="$LLAMA_SERVER" \
    "$VERIFY" "$MODEL_FILE"

  LLAMA_ARCHITECTURE="$architecture" \
  LLAMA_DEVICE="$METAL_DEVICE" \
  LLAMA_GPU_LAYERS=all \
  LLAMA_REQUIRE_METAL=1 \
  LLAMA_EMBEDDING="$LLAMA_EMBEDDING" \
  LLAMA_SERVER="$LLAMA_SERVER" \
    "$VERIFY" "$MODEL_FILE"
done

codesign --force --sign - --timestamp=none "$LLAMA_SERVER"
codesign --verify --strict --verbose=2 "$LLAMA_SERVER"

BINARY_SHA256=$(shasum -a 256 "$LLAMA_SERVER" | awk '{print $1}')
BINARY_SIZE=$(wc -c <"$LLAMA_SERVER" | tr -d ' ')
MANIFEST_SHA256=$(shasum -a 256 "$EMBEDDING_MANIFEST" | awk '{print $1}')
GOLDEN_SHA256=$(shasum -a 256 "$GOLDEN_FIXTURES" | awk '{print $1}')
LICENSE_SHA256=$(shasum -a 256 "$SOURCE/$LICENSE_SOURCE" | awk '{print $1}')

STAGE_TEMP=$(mktemp -d "$ROOT/app/src-tauri/resources/.release.XXXXXX")
mkdir -p \
  "$STAGE_TEMP/$(dirname "$BINARY_STAGING_PATH")" \
  "$STAGE_TEMP/$(dirname "$MANIFEST_STAGING_PATH")" \
  "$STAGE_TEMP/$(dirname "$LICENSE_STAGING_PATH")"

install -m 755 "$LLAMA_SERVER" "$STAGE_TEMP/$BINARY_STAGING_PATH"
install -m 644 "$SOURCE/$LICENSE_SOURCE" "$STAGE_TEMP/$LICENSE_STAGING_PATH"

jq \
  --arg binarySha256 "$BINARY_SHA256" \
  --argjson binarySize "$BINARY_SIZE" \
  --slurpfile versionOutputs "$VERSION_OUTPUTS" \
  --arg embeddingManifestSha256 "$MANIFEST_SHA256" \
  --arg goldenFixturesSha256 "$GOLDEN_SHA256" \
  --arg licenseSha256 "$LICENSE_SHA256" \
  '. + {
    binary: {
      bundlePath: .resourceDestinations.binary,
      sha256: $binarySha256,
      size: $binarySize,
      versionOutputByArchitecture: $versionOutputs[0]
    },
    verification: {
      modelBundled: false,
      embeddingManifest: {
        bundlePath: "embedding-indexes/jina-v1.json",
        sha256: $embeddingManifestSha256
      },
      goldenFixtures: {
        bundlePath: "embedding-indexes/jina-v1-golden.json",
        sha256: $goldenFixturesSha256
      },
      architectureChecks: (
        .target.architectures
        | map({
            architecture: .,
            cpuPassed: true,
            metalPassed: true
          })
      )
    },
    licenseNotices: (
      .licenseNotices
      | map(. + {sha256: $licenseSha256})
    )
  }' "$PIN" >"$STAGE_TEMP/$MANIFEST_STAGING_PATH"
chmod 644 "$STAGE_TEMP/$MANIFEST_STAGING_PATH"

mkdir -p \
  "$STAGE/$(dirname "$BINARY_STAGING_PATH")" \
  "$STAGE/$(dirname "$MANIFEST_STAGING_PATH")" \
  "$STAGE/$(dirname "$LICENSE_STAGING_PATH")"

install -m 755 "$STAGE_TEMP/$BINARY_STAGING_PATH" "$STAGE/$BINARY_STAGING_PATH"
install -m 644 "$STAGE_TEMP/$MANIFEST_STAGING_PATH" "$STAGE/$MANIFEST_STAGING_PATH"
install -m 644 "$STAGE_TEMP/$LICENSE_STAGING_PATH" "$STAGE/$LICENSE_STAGING_PATH"

rm -rf "$STAGE_TEMP"
STAGE_TEMP=

echo "staged $BINARY_DESTINATION"
echo "sha256 $BINARY_SHA256"
jq -r 'to_entries[] | "\(.key): \(.value)"' "$VERSION_OUTPUTS"
