#!/usr/bin/env bash
# Purpose: keel statusline. Renders model + context usage, and appends a
#   compaction-savings badge (caveman/RTK-style ROI surface) sourced from
#   `keel gain --json` when the binary is reachable.
# Caller: the harness statusLine command
#   (settings.json: {"type":"command","command":"~/.claude/statusline-keel.sh"}).
# Input: the harness pipes session JSON on stdin (model, context_window, cwd, ...).
# Output: one status line on stdout. Degrades gracefully — if the binary or gain
#   data is unavailable the savings badge is omitted, and the line never errors
#   (a non-zero exit or empty output blanks the harness statusline).
# Dependencies: POSIX shell + grep/sed only. No python required.
set -u

input="$(cat 2>/dev/null || true)"

# Extract the first JSON string/number value for a key, tolerating whitespace.
# Pure shell (grep/sed): works in minimal statusline shells without python/jq.
json_value() {
  # $1 = key name
  printf '%s' "$input" \
    | grep -o "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" \
    | head -1 | sed 's/.*:[[:space:]]*"//; s/"$//'
}
json_number() {
  # $1 = key name; returns the numeric token after the key
  printf '%s' "$input" \
    | grep -o "\"$1\"[[:space:]]*:[[:space:]]*[0-9][0-9.]*" \
    | head -1 | sed 's/.*:[[:space:]]*//'
}

model="$(json_value display_name)"
used_pct="$(json_number used_percentage)"

# Locate the installed keel binary (best-effort, never required).
resolve_binary() {
  local home_dir="${HOME:-${USERPROFILE:-}}"
  for candidate in \
    "$home_dir/.claude/keel" \
    "$home_dir/.claude/keel.exe"; do
    if [ -x "$candidate" ]; then printf '%s' "$candidate"; return 0; fi
  done
  if command -v keel >/dev/null 2>&1; then printf '%s' "keel"; return 0; fi
  return 1
}

# Savings badge from `gain --json` (optional). Pinned to the real schema:
# top-level integer field 'tokensSaved'. Parsed with the same pure-shell helper.
savings_badge=""
if bin="$(resolve_binary)"; then
  gain_json="$("$bin" gain --since today --json 2>/dev/null || true)"
  if [ -n "$gain_json" ]; then
    saved="$(printf '%s' "$gain_json" \
      | grep -o "\"tokensSaved\"[[:space:]]*:[[:space:]]*[0-9]*" \
      | head -1 | sed 's/.*:[[:space:]]*//')"
    if [ -n "${saved:-}" ] && [ "${saved:-0}" -gt 0 ] 2>/dev/null; then
      savings_badge="saved ${saved} tok"
    fi
  fi
fi

# Compose the line; omit empty segments cleanly.
line=""
[ -n "$model" ] && line="$model"
if [ -n "$used_pct" ]; then
  if [ -n "$line" ]; then line="$line | ctx ${used_pct}%"; else line="ctx ${used_pct}%"; fi
fi
if [ -n "$savings_badge" ]; then
  if [ -n "$line" ]; then line="$line | $savings_badge"; else line="$savings_badge"; fi
fi
[ -z "$line" ] && line="keel"

printf '%s\n' "$line"
