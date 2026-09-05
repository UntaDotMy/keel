#!/usr/bin/env bash
# keel:managed-host-file (remove this line before customizing to opt out of upgrades)
# ---------------------------------------------------------------------------
# keel Cursor adapter — bridges Cursor hook events to `keel bridge`.
#
# Cursor's hooks (preToolUse, postToolUse, preCompact, stop, sessionEnd) call
# this script with JSON on stdin. The hook_event_name field routes dispatch:
#
#   preToolUse  -> session-start (once) + Iron Law gate + Shell compaction reroute
#   postToolUse -> bridge observe (observation capture, fire-and-forget)
#   preCompact  -> bridge pre-compact (learning checkpoint before window rewrite)
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
# Keep the shell declaration in parity with Rust and bridge-core. Multi-word
# forms are documented here so the parity test cannot silently lose them.
KEEL_RESEARCH_SUBCOMMANDS="system-map system_map recall doctor code-search code_search skill-route skill_route skill-list skill_list skill-get skill_get context-brief context_brief"
# Multi-word research forms: memory status, memory recall, memory system-map,
# memory scope, anvil prefix-check, anvil sieve.

# Resolve the keel binary: KEEL_HOME → ~/.keel → ~/.claude → PATH.
if [ -n "${KEEL_HOME:-}" ] && [ -x "$KEEL_HOME/keel" ]; then
  KEEL_BIN="$KEEL_HOME/keel"
elif [ -n "${KEEL_HOME:-}" ] && [ -x "$KEEL_HOME/keel.exe" ]; then
  KEEL_BIN="$KEEL_HOME/keel.exe"
elif [ -x "$HOME/.keel/keel" ]; then
  KEEL_BIN="$HOME/.keel/keel"
elif [ -x "$HOME/.keel/keel.exe" ]; then
  KEEL_BIN="$HOME/.keel/keel.exe"
elif [ -x "$HOME/.claude/keel" ]; then
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
TOOL_PATH=$(printf '%s' "$INPUT" | "$JQ_BIN" -r '.tool_input.path // .tool_input.file_path // .tool_input.filePath // .path // empty' 2>/dev/null) || TOOL_PATH=""
CWD=$(printf '%s' "$INPUT" | "$JQ_BIN" -r '.cwd // empty' 2>/dev/null) || CWD=""
[ -z "$CWD" ] && CWD="$PWD"
# conversation_id is the stable session identifier across a Cursor conversation.
SESSION_ID=$(printf '%s' "$INPUT" | "$JQ_BIN" -r '.conversation_id // empty' 2>/dev/null) || SESSION_ID=""
[ -z "$SESSION_ID" ] && SESSION_ID="default"

# --- Iron Law marker dir (SHARED with Rust: iron-law-satisfied). ---
# Session key matches Rust sanitize_memory_key (lowercase alnum, else '-').
# why: `s/-\+/-/g` is GNU-only; BSD sed (macOS) never collapsed the dash runs, so
# this adapter and the Rust gate read different marker files. `tr -s` is POSIX.
SESSION_KEY=$(printf '%s' "$SESSION_ID" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g' | tr -s '-' | sed 's/^-//; s/-$//')
# Rust's sanitize_memory_key falls back to "workspace" when nothing survives.
[ -z "$SESSION_KEY" ] && SESSION_KEY="workspace"
keel_state_root() {
  if [ -n "${KEEL_HOME:-}" ]; then echo "$KEEL_HOME/state"; return; fi
  if [ -d "$HOME/.keel" ]; then echo "$HOME/.keel/state"; return; fi
  echo "$HOME/.claude/state"
}
STATE_ROOT="$(keel_state_root)"
MARKER_DIR="$STATE_ROOT/iron-law-satisfied"
mkdir -p "$MARKER_DIR" 2>/dev/null || true
MARKER="$MARKER_DIR/$SESSION_KEY"
STARTED_DIR="$STATE_ROOT/cursor-session-started"
mkdir -p "$STARTED_DIR" 2>/dev/null || true
STARTED_MARKER="$STARTED_DIR/$SESSION_KEY"

is_edit_class_tool() {
  case "$1" in
    Write|Edit|Delete|StrReplace|MultiEdit|NotebookEdit|ApplyPatch|Patch|SearchReplace) return 0 ;;
    *) return 1 ;;
  esac
}

canonical_keel_tool() {
  case "$1" in
    StrReplace) echo "str_replace" ;;
    SearchReplace) echo "search_replace" ;;
    ApplyPatch) echo "apply_patch" ;;
    *) echo "$1" ;;
  esac
}

is_shell_tool() {
  case "$1" in
    Shell|Bash|PowerShell|Command|Terminal) return 0 ;;
    *) return 1 ;;
  esac
}
is_compound_command() {
  # Keep the local fail-closed guard aligned with Rust's `[&|;`\n]` parser.
  case "$1" in
    *'&&'*|*'||'*|*';'*|*'|'*|*'`'*|*'$('*|*$'\n'*) return 0 ;;
    *) return 1 ;;
  esac
}


# --- Lifecycle events: dispatch to keel bridge, no output needed. ---
case "$HOOK_EVENT" in
  preCompact)
    # Learning checkpoint before the working window is rewritten.
    "$KEEL_BIN" bridge pre-compact --session "$SESSION_ID" --cwd "$CWD" >/dev/null 2>&1 || true
    echo '{}'
    exit 0
    ;;
  stop)
    # Stop fires on every turn end. Do not run post-compact here to avoid loops.
    # Learning checkpoints belong on preCompact and sessionEnd.
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
    printf '%s' "$INPUT" | "$KEEL_BIN" bridge observe --session "$SESSION_ID" --cwd "$CWD" --tool "$TOOL_NAME" --phase post >/dev/null 2>&1 || true
    echo '{}'
    exit 0
    ;;
esac

# --- preToolUse: Iron Law enforcement first, then compaction reroute. ---

# Session-start (once per session): bootstrap + digest + MCP self-heal.
if [ ! -f "$STARTED_MARKER" ]; then
  if "$KEEL_BIN" bridge session-start --session "$SESSION_ID" --cwd "$CWD" >/dev/null 2>&1; then
    : > "$STARTED_MARKER" 2>/dev/null || true
  fi
fi
if is_shell_tool "$TOOL_NAME" && is_compound_command "$CMD"; then
  "$JQ_BIN" -n --arg msg "Compound shell commands are denied by the keel gate." '{
    "permission": "deny",
    "user_message": $msg,
    "agent_message": $msg
  }'
  exit 0
fi


if is_shell_tool "$TOOL_NAME"; then
  CANON_TOOL=$(canonical_keel_tool "$TOOL_NAME")
  GATE_ARGS=(--session "$SESSION_ID" --cwd "$CWD" --tool "$CANON_TOOL")
  [ -n "$CMD" ] && GATE_ARGS+=(--command "$CMD")
  GATE=$("$KEEL_BIN" bridge pre-tool-use "${GATE_ARGS[@]}" 2>/dev/null) || GATE=""
  case "$GATE" in
    KEEL_GATE_DENY*)
      REASON=$(printf '%s' "$GATE" | sed '1d')
      [ -z "$REASON" ] && REASON="IRON LAW shell gate denied this command."
      "$JQ_BIN" -n --arg msg "$REASON" '{
        "permission": "deny",
        "user_message": $msg,
        "agent_message": $msg
      }'
      exit 0
      ;;
    KEEL_GATE_ALLOW*) ;;
    *)
      "$JQ_BIN" -n --arg msg "IRON LAW shell gate could not be evaluated." '{
        "permission": "deny",
        "user_message": $msg,
        "agent_message": $msg
      }'
      exit 0
      ;;
  esac
fi
# Iron Law: deny edit-class tools via Rust core (evidence-based; no ack-on-deny).
if is_edit_class_tool "$TOOL_NAME"; then
  # Map the Cursor tool name to the canonical name so the Rust gate recognizes
  # it (e.g. StrReplace -> str_replace); without this the gate answered ALLOW.
  CANON_TOOL=$(canonical_keel_tool "$TOOL_NAME")
  GATE_ARGS=(--session "$SESSION_ID" --cwd "$CWD" --tool "$CANON_TOOL")
  [ -n "$TOOL_PATH" ] && GATE_ARGS+=(--path "$TOOL_PATH")
  GATE=$("$KEEL_BIN" bridge pre-tool-use "${GATE_ARGS[@]}" 2>/dev/null) || GATE=""
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
  # Local-marker fallback (mirrors the Pi adapter). The Rust gated set does not
  # include Delete, so `bridge pre-tool-use` returns ALLOW for it. Gate any
  # edit-class tool locally when this session has not yet cleared the Iron Law
  # marker, so a Delete (or any tool name the Rust core does not recognize) is
  # not silently allowed to run before research.
  if [ ! -f "$MARKER" ]; then
    REASON="IRON LAW ENFORCED (STRICT): Use a keel tool first (MCP system_map, recall, context_brief, skill_route, or keel doctor / code-search). Plain Read does not clear the gate."
    "$JQ_BIN" -n --arg msg "$REASON" '{
      "permission": "deny",
      "user_message": $msg,
      "agent_message": $msg
    }'
    exit 0
  fi
fi

# --- Compaction reroute for Shell tools. ---
if [ "$TOOL_NAME" = "Shell" ] && [ -n "$CMD" ]; then
  # Already wrapped — skip.
  case "$CMD" in
    keel\ run\ --*) echo '{}'; exit 0 ;;
  esac

  # Ask keel to rewrite the command (stdin = command, stdout = "KEEL_REWRITE <cmd>").
  # why: --tool is required; without it bridge rewrite sees an empty tool name,
  # fails the shell-tool check, and returns nothing, so no command is ever rerouted.
  REWRITE=$(printf '%s' "$CMD" | "$KEEL_BIN" bridge rewrite --tool "$TOOL_NAME" 2>/dev/null) || REWRITE=""

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
