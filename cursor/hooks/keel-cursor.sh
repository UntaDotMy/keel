#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# keel Cursor adapter — bridges Cursor hook events to `keel bridge`.
#
# Cursor's hooks (preToolUse, postToolUse, preCompact, stop, sessionEnd) call
# this script with JSON on stdin. The hook_event_name field routes dispatch:
#
#   preToolUse  -> session-start (once) + Iron Law gate + Shell compaction reroute
#   postToolUse -> bridge observe (observation capture, fire-and-forget)
#   preCompact  -> bridge pre-compact (pre-compaction checkpoint)
#   stop        -> bridge post-compact (turn-end checkpoint)
#   sessionEnd  -> bridge session-end (learning + capture + marker cleanup)
#
# Cursor tool names are capitalized: Shell, Read, Write, Edit, Grep, Delete,
# Task, MCP:<name> (per cursor.com/docs/hooks). The preToolUse matcher filters
# by tool NAME (regex); edit-class tools are Write/Edit/Delete.
#
# Output contract (Cursor preToolUse, verified vs cursor.com/docs/hooks):
#   Deny:    {"permission":"deny","user_message":"...","agent_message":"..."}
#   Rewrite: {"permission":"allow","updated_input":{"command":"<rewritten>"}}
#   Pass:    {}
#   Exit code 2 also blocks (equivalent to permission:deny).
#
# Fail-open: any error outputs {} so the original tool runs unchanged. Every
# keel bridge call is capped at a short timeout.
# ---------------------------------------------------------------------------

set -euo pipefail

# Resolve the keel binary: prefer ~/.claude/keel(.exe), fall back to PATH.
if [ -x "$HOME/.claude/keel" ]; then
  KEEL_BIN="$HOME/.claude/keel"
elif [ -x "$HOME/.claude/keel.exe" ]; then
  KEEL_BIN="$HOME/.claude/keel.exe"
else
  KEEL_BIN="keel"
fi

# jq is required for safe JSON parsing and output escaping (the rewrite case
# embeds an arbitrary command string). If absent, fail open: emit {} so the
# tool runs unchanged and surface a one-line stderr notice. Do NOT attempt
# half-parsed logic that could misclassify tools. Resolve jq or jq.exe (Windows).
JQ_BIN=""
if command -v jq >/dev/null 2>&1; then
  JQ_BIN="jq"
elif command -v jq.exe >/dev/null 2>&1; then
  JQ_BIN="jq.exe"
fi
if [ -z "$JQ_BIN" ]; then
  echo "keel-cursor: jq not found — skipping keel gate/rewrite" >&2
  echo '{}'
  exit 0
fi

INPUT=$(cat)

# --- Parse fields from stdin (jq, fail-open on parse error). ---
HOOK_EVENT=$(printf '%s' "$INPUT" | "$JQ_BIN" -r '.hook_event_name // empty' 2>/dev/null) || HOOK_EVENT=""
TOOL_NAME=$(printf '%s' "$INPUT" | "$JQ_BIN" -r '.tool_name // empty' 2>/dev/null) || TOOL_NAME=""
CMD=$(printf '%s' "$INPUT" | "$JQ_BIN" -r '.tool_input.command // empty' 2>/dev/null) || CMD=""
CWD=$(printf '%s' "$INPUT" | "$JQ_BIN" -r '.cwd // empty' 2>/dev/null) || CWD=""
[ -z "$CWD" ] && CWD="$PWD"
# conversation_id is the stable session identifier across a Cursor conversation.
SESSION_ID=$(printf '%s' "$INPUT" | "$JQ_BIN" -r '.conversation_id // empty' 2>/dev/null) || SESSION_ID=""
[ -z "$SESSION_ID" ] && SESSION_ID="default"

# --- Iron Law marker dir (SHARED with Rust: iron-law-satisfied). ---
# Session key matches Rust sanitize_memory_key (lowercase alnum, else '-').
SESSION_KEY=$(printf '%s' "$SESSION_ID" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g; s/-\+/-/g; s/^-//; s/-$//')
[ -z "$SESSION_KEY" ] && SESSION_KEY="default"
MARKER_DIR="$HOME/.claude/state/iron-law-satisfied"
mkdir -p "$MARKER_DIR" 2>/dev/null || true
MARKER="$MARKER_DIR/$SESSION_KEY"
STARTED_DIR="$HOME/.claude/state/cursor-session-started"
mkdir -p "$STARTED_DIR" 2>/dev/null || true
STARTED_MARKER="$STARTED_DIR/$SESSION_ID"

is_edit_class_tool() {
  case "$1" in
    Write|Edit|Delete|StrReplace|MultiEdit|NotebookEdit) return 0 ;;
    *) return 1 ;;
  esac
}

is_keel_research_tool() {
  case "$1" in
    *keel*|*KEEL*) return 0 ;;
    *) return 1 ;;
  esac
}

is_keel_reading_command() {
  case "$1" in
    "keel system-map"*|"keel recall"*|"keel doctor"*|"keel code-search"*|"keel observe"*|"keel memory"*|"keel workflow cockpit"*|"keel workflow status"*|"keel run -- keel "*) return 0 ;;
    *) return 1 ;;
  esac
}

# --- Lifecycle events: dispatch to keel bridge, no output needed. ---
case "$HOOK_EVENT" in
  preCompact)
    "$KEEL_BIN" bridge pre-compact --session "$SESSION_ID" --cwd "$CWD" >/dev/null 2>&1 || true
    echo '{}'
    exit 0
    ;;
  stop)
    "$KEEL_BIN" bridge post-compact --session "$SESSION_ID" --cwd "$CWD" >/dev/null 2>&1 || true
    echo '{}'
    exit 0
    ;;
  sessionEnd)
    "$KEEL_BIN" bridge session-end --session "$SESSION_ID" --cwd "$CWD" >/dev/null 2>&1 || true
    rm -f "$MARKER" "$STARTED_MARKER" 2>/dev/null || true
    echo '{}'
    exit 0
    ;;
  postToolUse)
    # Observation capture (fire-and-forget). stdin payload to bridge observe.
    printf '%s' "$INPUT" | "$KEEL_BIN" bridge observe --session "$SESSION_ID" --cwd "$CWD" --tool "$TOOL_NAME" >/dev/null 2>&1 || true
    echo '{}'
    exit 0
    ;;
esac

# --- preToolUse: Iron Law enforcement first, then compaction reroute. ---

# Session-start (once per session): bootstrap + digest + MCP self-heal.
if [ ! -f "$STARTED_MARKER" ]; then
  "$KEEL_BIN" bridge session-start --session "$SESSION_ID" --cwd "$CWD" >/dev/null 2>&1 || true
  : > "$STARTED_MARKER" 2>/dev/null || true
fi

# STRICT: only keel research tools clear the shared marker (not plain Read).
if is_keel_research_tool "$TOOL_NAME"; then
  printf 'satisfied' > "$MARKER" 2>/dev/null || true
fi

# Shell tools running keel reading commands also satisfy the gate.
if [ "$TOOL_NAME" = "Shell" ] && [ -n "$CMD" ]; then
  if is_keel_reading_command "$CMD"; then
    printf 'satisfied' > "$MARKER" 2>/dev/null || true
  fi
fi

# Iron Law: deny edit-class tools via Rust core (evidence-based; no ack-on-deny).
if is_edit_class_tool "$TOOL_NAME"; then
  GATE=$("$KEEL_BIN" bridge pre-tool-use --session "$SESSION_ID" --cwd "$CWD" --tool "$TOOL_NAME" 2>/dev/null) || GATE=""
  case "$GATE" in
    KEEL_GATE_DENY*)
      REASON=$(printf '%s' "$GATE" | sed '1d')
      [ -z "$REASON" ] && REASON="IRON LAW ENFORCED (STRICT): Use a keel tool first (MCP system_map, recall, context_brief, skill_route, or keel doctor / code-search). Plain Read does not clear the gate."
      "$JQ_BIN" -n --arg msg "$REASON" '{
        "permission": "deny",
        "user_message": $msg,
        "agent_message": $msg
      }'
      exit 0
      ;;
  esac
fi

# --- Compaction reroute for Shell tools. ---
if [ "$TOOL_NAME" = "Shell" ] && [ -n "$CMD" ]; then
  # Already wrapped — skip.
  case "$CMD" in
    keel\ run\ --*) echo '{}'; exit 0 ;;
  esac

  # Ask keel to rewrite the command (stdin = command, stdout = "KEEL_REWRITE <cmd>").
  REWRITE=$(printf '%s' "$CMD" | "$KEEL_BIN" bridge rewrite 2>/dev/null) || REWRITE=""

  case "$REWRITE" in
    KEEL_REWRITE\ *)
      REWRITTEN="${REWRITE#KEEL_REWRITE }"
      REWRITTEN="${REWRITTEN# }"
      if [ -n "$REWRITTEN" ] && [ "$REWRITTEN" != "$CMD" ]; then
        "$JQ_BIN" -n --arg cmd "$REWRITTEN" '{
          "permission": "allow",
          "updated_input": { "command": $cmd }
        }'
        exit 0
      fi
      ;;
  esac
fi

# Default: pass through unchanged.
echo '{}'
