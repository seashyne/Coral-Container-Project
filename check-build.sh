#!/usr/bin/env bash
# Build + test helper for the Coral workspace.
#
# Why this exists: the repo path contains a space ("data home project"), which makes a bare
# `cd coral_repo` fail from the parent directory and silently produces
# "could not find `Cargo.toml`" from every later cargo command.
#
# Usage:  ./check-build.sh [build|test|e2e|all]
set -euo pipefail

# Resolve the workspace relative to this script, so it works from any cwd.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="${SCRIPT_DIR}/coral_repo"

if [[ ! -f "${REPO}/Cargo.toml" ]]; then
  echo "error: no Cargo.toml at '${REPO}'" >&2
  echo "       expected the coral_repo/ workspace next to this script." >&2
  exit 1
fi

cd "${REPO}"
echo "==> workspace: $(pwd)"
echo "==> cargo:     $(cargo --version)"

mode="${1:-all}"

run_build() {
  echo
  echo "==> cargo build --workspace"
  cargo build --workspace
}

run_test() {
  echo
  echo "==> cargo test -p coral-daemon -p coral-cli"
  cargo test -p coral-daemon -p coral-cli
}

run_e2e() {
  echo
  echo "==> cargo test -p coral-e2e-tests --test real_image_e2e"
  # The two registry tests are #[ignore]d; the Auto-GC test is not and runs by default.
  cargo test -p coral-e2e-tests --test real_image_e2e -- --test-threads=1
  echo
  echo "==> network tests (need Docker Hub access)"
  cargo test -p coral-e2e-tests --test real_image_e2e -- --ignored --test-threads=1
}

case "${mode}" in
  build) run_build ;;
  test)  run_test ;;
  e2e)   run_e2e ;;
  all)   run_build && run_test && run_e2e ;;
  *)     echo "usage: $0 [build|test|e2e|all]" >&2; exit 2 ;;
esac

echo
echo "==> done"
