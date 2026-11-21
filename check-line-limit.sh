#!/usr/bin/env bash
# Enforce the project-wide source file size limit.
#
# Rule: no first-party source file may exceed 400 lines.
#   - Enforced: Rust (.rs), JavaScript (.js), CSS (.css), HTML (.html) in the
#     Coral workspace.
#   - Skipped: files ignored by git (build outputs and generated artifacts such
#     as the assembled desktop/docs frontends).
#
# Usage:  ./check-line-limit.sh
# Exit code 0 when every enforced file is within the limit.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
LIMIT="${LIMIT:-400}"

list_files() {
  # $1 = root path, $2 = include pattern
  find "$1" \
    -path '*/target' -prune -o \
    -path '*/node_modules' -prune -o \
    -path '*/.git' -prune -o \
    -type f -name "$2" -print 2>/dev/null || true
}

# Drops files that git ignores (generated frontends, build outputs).
drop_ignored() {
  local file
  while read -r file; do
    [[ -z "$file" ]] && continue
    if git -C "$SCRIPT_DIR" check-ignore --no-index -q "$file" 2>/dev/null; then
      continue
    fi
    printf '%s\n' "$file"
  done
}

REPORT="$(mktemp)"
trap 'rm -f "$REPORT"' EXIT

add_rows() {
  local count file
  while read -r count file; do
    [[ -z "$count" || -z "$file" ]] && continue
    [[ "$count" -le "$LIMIT" ]] && continue
    printf 'FAIL  %6s  %s\n' "$count" "$file" >>"$REPORT"
  done
}

{
  list_files "$SCRIPT_DIR/coral_repo" '*.rs'
  list_files "$SCRIPT_DIR/desktop/src-tauri/src" '*.rs'
  list_files "$SCRIPT_DIR/desktop/src" '*.js'
  list_files "$SCRIPT_DIR/desktop/src" '*.css'
  list_files "$SCRIPT_DIR/desktop/src" '*.html'
  list_files "$SCRIPT_DIR/docs" '*.js'
  list_files "$SCRIPT_DIR/docs" '*.css'
  list_files "$SCRIPT_DIR/docs" '*.html'
} | sort -u | drop_ignored | while read -r file; do
  printf '%s %s\n' "$(wc -l <"$file")" "$file"
done | add_rows

if [[ -s "$REPORT" ]]; then
  echo "Files above the ${LIMIT}-line limit:"
  sort -k2 "$REPORT"
  echo
  echo "error: ${LIMIT}-line limit violated (see rows above)" >&2
  exit 1
fi

echo "ok: every enforced source file is within ${LIMIT} lines."
