#!/usr/bin/env bash
# Enforce the project-wide source file size limit.
#
# Rule: no first-party source file may exceed 400 lines.
#   - Enforced: Rust (.rs), JavaScript (.js), CSS (.css) in the Coral workspace.
#   - Reported only: HTML (.html). Splitting the two large HTML documents needs a
#     templating/build-step decision, so they are listed as warnings, not failures.
#
# Usage:  ./check-line-limit.sh
# Exit code 0 when every enforced file is within the limit.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
LIMIT="${LIMIT:-400}"

collect() {
  # $1 = root path, $2 = include pattern
  find "$1" \
    -path '*/target' -prune -o \
    -path '*/node_modules' -prune -o \
    -path '*/.git' -prune -o \
    -type f -name "$2" -print0 2>/dev/null \
    | xargs -0 -r wc -l 2>/dev/null || true
}

REPORT="$(mktemp)"
trap 'rm -f "$REPORT"' EXIT

add_rows() {
  # $1 = "1" for enforced rows, "0" for report-only rows
  local enforced="$1" count file
  while read -r count file; do
    [[ "$file" == "total" || -z "$file" ]] && continue
    [[ "$count" -le "$LIMIT" ]] && continue
    if [[ "$enforced" == "1" ]]; then
      printf 'FAIL  %6s  %s\n' "$count" "$file" >>"$REPORT"
    else
      printf 'WARN  %6s  %s\n' "$count" "$file" >>"$REPORT"
    fi
  done
}

# --- Enforced: Rust -------------------------------------------------------
{
  collect "$SCRIPT_DIR/coral_repo" '*.rs'
  collect "$SCRIPT_DIR/desktop/src-tauri/src" '*.rs'
} | add_rows 1

# --- Enforced: JavaScript / CSS ------------------------------------------
{
  collect "$SCRIPT_DIR/desktop/src" '*.js'
  collect "$SCRIPT_DIR/desktop/src" '*.css'
  collect "$SCRIPT_DIR/docs" '*.js'
  collect "$SCRIPT_DIR/docs" '*.css'
} | add_rows 1

# --- Reported only: HTML --------------------------------------------------
{
  collect "$SCRIPT_DIR/desktop/src" '*.html'
  collect "$SCRIPT_DIR/docs" '*.html'
} | add_rows 0

if [[ -s "$REPORT" ]]; then
  echo "Files above the ${LIMIT}-line limit:"
  sort -k2 "$REPORT"
  echo
fi

if grep -q '^FAIL' "$REPORT"; then
  echo "error: ${LIMIT}-line limit violated (see FAIL rows above)" >&2
  exit 1
fi

if grep -q '^WARN' "$REPORT"; then
  echo "note: HTML documents above the limit are reported but not enforced yet." >&2
fi

echo "ok: every enforced source file is within ${LIMIT} lines."
