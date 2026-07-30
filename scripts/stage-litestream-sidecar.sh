#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
PIN="$ROOT/app/src-tauri/resources/sidecars/litestream-v1.json"
NOTICE_SOURCE="$ROOT/app/src-tauri/resources/sidecars/litestream-NOTICE"
STAGE="$ROOT/app/src-tauri/resources/release"
ARM64_ARCHIVE_OVERRIDE=${1:-}
X86_64_ARCHIVE_OVERRIDE=${2:-}
CHECKSUMS_OVERRIDE=${3:-}

assert_file_size_and_sha256() {
  file_path=$1
  expected_size=$2
  expected_sha256=$3
  label=$4

  actual_size=$(wc -c <"$file_path" | tr -d ' ')
  test "$actual_size" = "$expected_size" || {
    echo "$label size mismatch" >&2
    exit 1
  }
  actual_sha256=$(shasum -a 256 "$file_path" | awk '{print $1}')
  test "$actual_sha256" = "$expected_sha256" || {
    echo "$label SHA-256 mismatch" >&2
    exit 1
  }
}

for command in arch codesign curl file jq lipo otool shasum tar; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "required command not found: $command" >&2
    exit 1
  }
done

test "$(uname -s)" = Darwin || {
  echo "the Litestream release stage requires macOS" >&2
  exit 1
}

host_architecture=$(uname -m)
jq -er --arg architecture "$host_architecture" \
  '.target.architectures | index($architecture) != null' "$PIN" >/dev/null || {
  echo "unsupported build host architecture: $host_architecture" >&2
  exit 1
}

work_root=$(mktemp -d "${TMPDIR:-/tmp}/kosh-litestream-release.XXXXXX")
extract_root="$work_root/extract"
universal_root="$work_root/universal"
stage_temp="$work_root/stage"
mkdir -p "$extract_root" "$universal_root"

cleanup() {
  rm -rf "$work_root"
}
trap cleanup EXIT INT TERM

checksums_name=$(jq -er '.upstream.checksums.name' "$PIN")
checksums_url=$(jq -er '.upstream.checksums.url' "$PIN")
checksums_size=$(jq -er '.upstream.checksums.size' "$PIN")
checksums_sha256=$(jq -er '.upstream.checksums.sha256' "$PIN")
if test -n "$CHECKSUMS_OVERRIDE"; then
  test -f "$CHECKSUMS_OVERRIDE" || {
    echo "Litestream checksums file not found" >&2
    exit 1
  }
  checksums="$CHECKSUMS_OVERRIDE"
else
  checksums="$work_root/$checksums_name"
  curl -fL --retry 3 --connect-timeout 15 "$checksums_url" -o "$checksums"
fi
assert_file_size_and_sha256 "$checksums" "$checksums_size" "$checksums_sha256" \
  "Litestream checksums"

for architecture in $(jq -er '.target.architectures[]' "$PIN"); do
  override=
  case "$architecture" in
    arm64) override="$ARM64_ARCHIVE_OVERRIDE" ;;
    x86_64) override="$X86_64_ARCHIVE_OVERRIDE" ;;
    *)
      echo "unsupported Litestream architecture in pin: $architecture" >&2
      exit 1
      ;;
  esac

  asset_name=$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].name' "$PIN")
  asset_url=$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].url' "$PIN")
  asset_size=$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].size' "$PIN")
  asset_sha256=$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].sha256' "$PIN")

  if test -n "$override"; then
    test -f "$override" || {
      echo "Litestream $architecture archive not found" >&2
      exit 1
    }
    archive="$override"
  else
    archive="$work_root/$asset_name"
    curl -fL --retry 3 --connect-timeout 15 "$asset_url" -o "$archive"
  fi
  assert_file_size_and_sha256 "$archive" "$asset_size" "$asset_sha256" \
    "Litestream $architecture archive"
  grep -Fx "$asset_sha256  $asset_name" "$checksums" >/dev/null || {
    echo "Litestream $architecture archive is absent from the pinned official checksums" >&2
    exit 1
  }

  architecture_root="$extract_root/$architecture"
  mkdir -p "$architecture_root"
  archive_binary_path=$(jq -er '.binary.archivePath' "$PIN")
  license_source_path=$(jq -er '.licenseNotices[0].sourcePath' "$PIN")
  tar -xzf "$archive" -C "$architecture_root" \
    "$archive_binary_path" \
    "$license_source_path"

  binary="$architecture_root/$archive_binary_path"
  license="$architecture_root/$license_source_path"
  test -f "$binary" && test ! -L "$binary" || {
    echo "Litestream $architecture binary is not a regular file" >&2
    exit 1
  }
  test -f "$license" && test ! -L "$license" || {
    echo "Litestream $architecture license is not a regular file" >&2
    exit 1
  }
  chmod 755 "$binary"

  binary_size=$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].binarySize' "$PIN")
  binary_sha256=$(jq -er --arg architecture "$architecture" \
    '.upstream.assets[$architecture].binarySha256' "$PIN")
  assert_file_size_and_sha256 "$binary" "$binary_size" "$binary_sha256" \
    "Litestream $architecture binary"

  file "$binary" | grep -q "Mach-O 64-bit executable $architecture" || {
    file "$binary" >&2
    echo "Litestream binary has the wrong architecture" >&2
    exit 1
  }
  if otool -L "$binary" | tail -n +2 |
    grep -Ev '^[[:space:]]+(/System/Library/|/usr/lib/)' >/dev/null; then
    otool -L "$binary" >&2
    echo "Litestream $architecture has a non-system dynamic dependency" >&2
    exit 1
  fi

  minimum_system_version=$(jq -er --arg architecture "$architecture" \
    '.target.binaryMinimumSystemVersionByArchitecture[$architecture]' "$PIN")
  actual_minimum_system_version=$(
    otool -l "$binary" |
      awk '
        /LC_BUILD_VERSION/ { in_build_version = 1; next }
        in_build_version && $1 == "minos" { print $2; exit }
      '
  )
  test "$actual_minimum_system_version" = "$minimum_system_version" || {
    echo "Litestream $architecture deployment target mismatch" >&2
    exit 1
  }

  license_sha256=$(jq -er '.licenseNotices[0].sha256' "$PIN")
  actual_license_sha256=$(shasum -a 256 "$license" | awk '{print $1}')
  test "$actual_license_sha256" = "$license_sha256" || {
    echo "Litestream $architecture license SHA-256 mismatch" >&2
    exit 1
  }
done

universal="$universal_root/litestream"
lipo -create \
  "$extract_root/arm64/$(jq -er '.binary.archivePath' "$PIN")" \
  "$extract_root/x86_64/$(jq -er '.binary.archivePath' "$PIN")" \
  -output "$universal"
chmod 755 "$universal"

for architecture in $(jq -er '.target.architectures[]' "$PIN"); do
  lipo -archs "$universal" | tr ' ' '\n' | grep -Fx "$architecture" >/dev/null || {
    lipo -info "$universal" >&2
    echo "universal Litestream binary is missing $architecture" >&2
    exit 1
  }
  if otool -arch "$architecture" -L "$universal" | tail -n +2 |
    grep -Ev '^[[:space:]]+(/System/Library/|/usr/lib/)' >/dev/null; then
    otool -arch "$architecture" -L "$universal" >&2
    echo "universal Litestream $architecture has a non-system dependency" >&2
    exit 1
  fi
done

version_arguments=$(jq -er '.binary.versionArguments[]' "$PIN")
for architecture in $(jq -er '.target.architectures[]' "$PIN"); do
  expected_version=$(jq -er --arg architecture "$architecture" \
    '.binary.versionOutputByArchitecture[$architecture]' "$PIN")
  if ! actual_version=$(arch "-$architecture" "$universal" "$version_arguments" 2>&1); then
    echo "$actual_version" >&2
    echo "could not execute the $architecture Litestream slice; install Rosetta if needed" >&2
    exit 1
  fi
  test "$actual_version" = "$expected_version" || {
    echo "Litestream $architecture version output mismatch" >&2
    exit 1
  }
done

codesign --force --sign - --timestamp=none \
  --identifier "$(jq -er '.binary.universal.codeSignatureIdentifier' "$PIN")" \
  "$universal"
codesign --verify --strict --verbose=2 "$universal"

universal_sha256=$(shasum -a 256 "$universal" | awk '{print $1}')
universal_size=$(wc -c <"$universal" | tr -d ' ')
expected_universal_sha256=$(jq -er '.binary.universal.sha256' "$PIN")
expected_universal_size=$(jq -er '.binary.universal.size' "$PIN")
test "$universal_sha256" = "$expected_universal_sha256" || {
  echo "universal Litestream binary SHA-256 mismatch" >&2
  exit 1
}
test "$universal_size" = "$expected_universal_size" || {
  echo "universal Litestream binary size mismatch" >&2
  exit 1
}
binary_staging_path=$(jq -er '.stagingPaths.binary' "$PIN")
manifest_staging_path=$(jq -er '.stagingPaths.releaseManifest' "$PIN")
license_staging_path=$(jq -er '.stagingPaths.license' "$PIN")
notice_staging_path=$(jq -er '.stagingPaths.notice' "$PIN")

mkdir -p \
  "$stage_temp/$(dirname "$binary_staging_path")" \
  "$stage_temp/$(dirname "$manifest_staging_path")" \
  "$stage_temp/$(dirname "$license_staging_path")" \
  "$stage_temp/$(dirname "$notice_staging_path")"
install -m 755 "$universal" "$stage_temp/$binary_staging_path"
install -m 644 "$extract_root/arm64/$(jq -er '.licenseNotices[0].sourcePath' "$PIN")" \
  "$stage_temp/$license_staging_path"
install -m 644 "$NOTICE_SOURCE" "$stage_temp/$notice_staging_path"

jq \
  --arg sha256 "$universal_sha256" \
  --argjson size "$universal_size" \
  '. + {
    stagedBinary: {
      bundlePath: .resourceDestinations.binary,
      sha256: $sha256,
      size: $size,
      architectures: .target.architectures,
      versionOutputByArchitecture: .binary.versionOutputByArchitecture
    },
    verification: (
      .verification + {
        architectureChecks: (
          .target.architectures
          | map({
              architecture: .,
              executable: true,
              systemLibrariesOnly: true
            })
        )
      }
    )
  }' "$PIN" >"$stage_temp/$manifest_staging_path"
chmod 644 "$stage_temp/$manifest_staging_path"

mkdir -p \
  "$STAGE/$(dirname "$binary_staging_path")" \
  "$STAGE/$(dirname "$manifest_staging_path")" \
  "$STAGE/$(dirname "$license_staging_path")" \
  "$STAGE/$(dirname "$notice_staging_path")"
install -m 755 "$stage_temp/$binary_staging_path" "$STAGE/$binary_staging_path"
install -m 644 "$stage_temp/$manifest_staging_path" "$STAGE/$manifest_staging_path"
install -m 644 "$stage_temp/$license_staging_path" "$STAGE/$license_staging_path"
install -m 644 "$stage_temp/$notice_staging_path" "$STAGE/$notice_staging_path"

echo "staged $(jq -er '.resourceDestinations.binary' "$PIN")"
echo "sha256 $universal_sha256"
echo "architectures $(jq -er '.target.architectures | join("+")' "$PIN")"
