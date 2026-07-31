#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

shell_scripts=()
while IFS= read -r shell_script; do
  shell_scripts+=("$shell_script")
done < <(find scripts -type f -name '*.sh' -print | sort)
if ((${#shell_scripts[@]} == 0)); then
  echo "no tracked shell scripts found" >&2
  exit 1
fi

bash -n "${shell_scripts[@]}"
scripts/check-secrets.sh
scripts/test-secret-check.sh
scripts/test-bundle-check.sh
node app/scripts/test-release-source.mjs
node app/scripts/check-backup-fault-matrix.mjs
(
  cd app
  node scripts/check-litestream-release-contracts.mjs
)
git diff --check
git diff --cached --check
if [[ -n "${KOSH_DIFF_BASE:-}" && ! "$KOSH_DIFF_BASE" =~ ^0+$ ]]; then
  git rev-parse --verify "$KOSH_DIFF_BASE^{commit}" >/dev/null
  git diff --check "$KOSH_DIFF_BASE"...HEAD
fi

echo "repository checks passed"
