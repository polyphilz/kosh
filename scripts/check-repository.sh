#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

mapfile -t shell_scripts < <(find scripts -type f -name '*.sh' -print | sort)
if ((${#shell_scripts[@]} == 0)); then
  echo "no tracked shell scripts found" >&2
  exit 1
fi

bash -n "${shell_scripts[@]}"
scripts/check-secrets.sh
scripts/test-secret-check.sh
git diff --check

echo "repository checks passed"
