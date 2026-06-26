#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# keel Cursor adapter — compaction reroute for shell commands.
#
# Cursor's preToolUse hook (matcher: Shell) calls this script with JSON on
# stdin containing .tool_input.command. The script delegates to
# `keel bridge rewrite` and returns Cursor's updated_input JSON to reroute
# noisy commands through `keel run --` for output compaction.
#
# Output contract (Cursor preToolUse):
#   Rewrite: {"permission":"allow","updated_input":{"command":"<rewritten>"}}
#   Pass-through: {}
#
# Fail-open: any error outputs {} so the original command runs unchanged.
# ---------------------------------------------------------------------------

set -euo pipefail

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null) || {
  echo '{}'
  exit 0
}

# No command to rewrite — pass through.
if [ -z "$CMD" ]; then
  echo '{}'
  exit 0
fi

# Already wrapped — skip.
case "$CMD" in
  keel\ run\ --*) echo '{}'; exit 0 ;;
esac

# Resolve the keel binary: prefer ~/.claude/keel(.exe), fall back to PATH.
if [ -x "$HOME/.claude/keel" ]; then
  KEEL_BIN="$HOME/.claude/keel"
elif [ -x "$HOME/.claude/keel.exe" ]; then
  KEEL_BIN="$HOME/.claude/keel.exe"
else
  KEEL_BIN="keel"
fi

# Ask keel to rewrite the command.
# bridge rewrite reads the command from stdin and outputs:
#   "KEEL_REWRITE <rewritten_command>" for noisy commands
#   empty for non-noisy / non-shell commands
REWRITE=$(echo "$CMD" | "$KEEL_BIN" bridge rewrite 2>/dev/null) || {
  echo '{}'
  exit 0
}

# Check if keel returned a rewrite.
if [ -z "$REWRITE" ]; then
  echo '{}'
  exit 0
fi

case "$REWRITE" in
  KEEL_REWRITE\ *)
    REWRITTEN="${REWRITE#KEEL_REWRITE }"
    REWRITTEN="${REWRITTEN# }"
    ;;
  *)
    echo '{}'
    exit 0
    ;;
esac

# No change — pass through.
if [ "$REWRITTEN" = "$CMD" ] || [ -z "$REWRITTEN" ]; then
  echo '{}'
  exit 0
fi

# Build the response JSON with jq (never heredoc-interpolate the command —
# unescaped quotes/newlines would produce malformed JSON and Cursor fails open).
jq -n --arg cmd "$REWRITTEN" '{
  "permission": "allow",
  "updated_input": { "command": $cmd }
}'
