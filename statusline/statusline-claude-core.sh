#!/usr/bin/env bash
# Purpose: claude-core statusline. Renders model + context usage, and appends a
#   compaction-savings badge (caveman/RTK-style ROI surface) sourced from
#   `claude-skills gain --json` when the binary is reachable.
# Caller: Claude Code statusLine command
#   (settings.json: {"type":"command","command":"~/.claude/statusline-claude-core.sh"}).
# Input: Claude Code pipes session JSON on stdin (model, context_window, cwd, ...).
# Output: one status line on stdout. Degrades gracefully — if the binary or gain
#   data is unavailable the savings badge is omitted, and the line never errors
#   (a non-zero exit or empty output blanks the Claude Code statusline).
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

# Locate the installed claude-skills binary (best-effort, never required).
resolve_binary() {
  local home_dir="${HOME:-${USERPROFILE:-}}"
  for candidate in \
    "$home_dir/.claude/claude-skills" \
    "$home_dir/.claude/claude-skills.exe"; do
    if [ -x "$candidate" ]; then printf '%s' "$candidate"; return 0; fi
  done
  if command -v claude-skills >/dev/null 2>&1; then printf '%s' "claude-skills"; return 0; fi
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
[ -z "$line" ] && line="claude-core"

printf '%s\n' "$line"
