#!/usr/bin/env node

// ---------------------------------------------------------------------------
// keel Codex CLI Plugin — bridges Codex lifecycle events to `keel bridge`
//
// This script is invoked by Codex hook commands. It reads event JSON from
// stdin, maps Codex lifecycle events to keel bridge subcommands, and writes
// context text to stdout. Follows the same design principles as the OpenCode
// adapter: resolve binary once, 500ms hard timeout, fire-and-forget for
// observations, degrade gracefully on any error.
//
// Codex plugin structure:
//   .codex-plugin/plugin.json    — manifest pointing at hooks/hooks.json
//   hooks/hooks.json             — lifecycle hook registrations
//   hooks/keel-codex.ts          — this file (the adapter script)
// ---------------------------------------------------------------------------

import * as os from "node:os";
import * as fs from "node:fs";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

// ---------------------------------------------------------------------------
// Types — Codex hook stdin payload (subset we use)
// ---------------------------------------------------------------------------

interface CodexHookInput {
  event: string;
  session_id?: string;
  cwd?: string;
  // SessionStart fields
  start_source?: string; // "startup" | "resume" | "clear" | "compact"
  // PreToolUse / PostToolUse fields
  tool?: string;
  tool_input?: unknown;
  failed?: boolean;
  // UserPromptSubmit fields
  prompt?: string;
  // Stop fields
  turn_data?: Record<string, unknown>;
  // PreCompact / PostCompact fields
  trigger?: string; // "manual" | "auto"
  [key: string]: unknown;
}

// ---------------------------------------------------------------------------
// Binary resolution — resolved once at script init
// ---------------------------------------------------------------------------

const BIN_NAME: string =
  os.platform() === "win32" ? "keel.exe" : "keel";

/**
 * Resolve the bridge binary path.
 * Prefer ~/.claude/<binary>; fall back to bare name (PATH lookup).
 */
function resolveBinary(): string {
  const home = os.homedir();
  const fallback = path.join(home, ".claude", BIN_NAME);
  try {
    if (fs.existsSync(fallback)) return fallback;
  } catch {
    // fs failure — PATH only
  }
  return BIN_NAME;
}

const BRIDGE_BIN: string = resolveBinary();

// ---------------------------------------------------------------------------
// Marker-file helpers — guard session-start to once per session
// ---------------------------------------------------------------------------

const MARKER_DIR = path.join(
  os.homedir(),
  ".claude",
  "state",
  "codex-session-started",
);

function markerPath(sessionID: string): string {
  return path.join(MARKER_DIR, sessionID);
}

function ensureMarkerDir(): void {
  try {
    fs.mkdirSync(MARKER_DIR, { recursive: true });
  } catch {
    /* best-effort */
  }
}

function hasStarted(sessionID: string): boolean {
  ensureMarkerDir();
  try {
    return fs.existsSync(markerPath(sessionID));
  } catch {
    return false;
  }
}

function markStarted(sessionID: string): void {
  ensureMarkerDir();
  try {
    fs.writeFileSync(markerPath(sessionID), "", "utf-8");
  } catch {
    /* best-effort */
  }
}

function clearMarker(sessionID: string): void {
  try {
    fs.rmSync(markerPath(sessionID), { force: true });
  } catch {
    /* best-effort */
  }
}

// ---------------------------------------------------------------------------
// Bridge runner — never throws, 500ms hard timeout via execFileSync
// ---------------------------------------------------------------------------

function runBridge(subcommand: string, args: string[]): string {
  try {
    const result = execFileSync(
      BRIDGE_BIN,
      ["bridge", subcommand, ...args],
      {
        timeout: 500,
        stdio: ["pipe", "pipe", "pipe"],
        encoding: "utf-8",
        windowsHide: true,
      },
    );
    return result ?? "";
  } catch {
    return "";
  }
}

function runBridgeWithStdin(
  subcommand: string,
  args: string[],
  stdin: string,
): string {
  try {
    const result = execFileSync(
      BRIDGE_BIN,
      ["bridge", subcommand, ...args],
      {
        timeout: 500,
        input: stdin,
        stdio: ["pipe", "pipe", "pipe"],
        encoding: "utf-8",
        windowsHide: true,
      },
    );
    return result ?? "";
  } catch {
    return "";
  }
}

// ---------------------------------------------------------------------------
// Event handlers — map Codex events to bridge subcommands
// ---------------------------------------------------------------------------

function handleSessionStart(input: CodexHookInput): string {
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();

  if (hasStarted(sessionID)) return "";

  const text = runBridge("session-start", [
    "--session", sessionID,
    "--cwd", cwd,
  ]);
  markStarted(sessionID);
  return text;
}

function handleUserPromptSubmit(input: CodexHookInput): string {
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();
  const prompt = input.prompt ?? "";

  if (!prompt) return "";

  return runBridge("user-prompt", [
    "--session", sessionID,
    "--cwd", cwd,
    "--prompt", prompt,
  ]);
}

function handlePreToolUse(input: CodexHookInput): string {
  // Observations are fire-and-forget. Codex hooks are synchronous, so we
  // call observe with a short timeout and discard the result.
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();
  const toolName = input.tool ?? "";
  const stdin = input.tool_input != null
    ? JSON.stringify(input.tool_input)
    : "{}";

  const args = [
    "--session", sessionID,
    "--cwd", cwd,
    "--tool", toolName,
  ];
  if (input.failed) args.push("--failed");

  runBridgeWithStdin("observe", args, stdin);
  return "";
}

function handlePreCompact(input: CodexHookInput): string {
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();

  return runBridge("post-compact", [
    "--session", sessionID,
    "--cwd", cwd,
  ]);
}

function handlePostCompact(input: CodexHookInput): string {
  // PostCompact fires after compaction. Record a learning checkpoint.
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();

  runBridge("post-compact", [
    "--session", sessionID,
    "--cwd", cwd,
  ]);
  return "";
}

function handleStop(input: CodexHookInput): string {
  // Stop fires when a turn completes. Run post-compact for the learning
  // checkpoint. Session-end fires on explicit session deletion, not here.
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();

  return runBridge("post-compact", [
    "--session", sessionID,
    "--cwd", cwd,
  ]);
}

function handleSessionEnd(input: CodexHookInput): string {
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();

  runBridge("session-end", [
    "--session", sessionID,
    "--cwd", cwd,
  ]);
  clearMarker(sessionID);
  return "";
}

// ---------------------------------------------------------------------------
// Main — read stdin, dispatch, write stdout
// ---------------------------------------------------------------------------

function main(): void {
  let raw = "";
  try {
    // Read the full stdin payload that Codex provides.
    // Codex hooks receive JSON on stdin with event metadata.
    const chunks: Buffer[] = [];
    const buf = Buffer.alloc(65536);
    // eslint-disable-next-line no-constant-condition
    while (true) {
      const n = fs.readSync(process.stdin.fd, buf, 0, buf.length, null);
      if (n === 0) break;
      chunks.push(buf.subarray(0, n));
    }
    raw = Buffer.concat(chunks).toString("utf-8");
  } catch {
    // stdin read failure — exit silently
    return;
  }

  let input: CodexHookInput;
  try {
    input = JSON.parse(raw);
  } catch {
    // Malformed input — exit silently
    return;
  }

  let contextText = "";

  try {
    switch (input.event) {
      case "SessionStart":
        contextText = handleSessionStart(input);
        break;
      case "UserPromptSubmit":
        contextText = handleUserPromptSubmit(input);
        break;
      case "PreToolUse":
        handlePreToolUse(input);
        break;
      case "PostToolUse":
        // PostToolUse is fire-and-forget for observation recording
        handlePreToolUse(input);
        break;
      case "PreCompact":
        contextText = handlePreCompact(input);
        break;
      case "PostCompact":
        handlePostCompact(input);
        break;
      case "Stop":
        contextText = handleStop(input);
        break;
      case "SessionEnd":
        handleSessionEnd(input);
        break;
      default:
        // Unknown event — exit silently
        break;
    }
  } catch {
    // Degrade gracefully — no context injected
  }

  // Write context to stdout if we have any
  if (contextText) {
    process.stdout.write(contextText);
  }
}

main();
