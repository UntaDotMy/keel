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
// Iron Law markers — track per-session reading-before-editing satisfaction
// (mirrors the OpenCode adapter's iron-law enforcement via tool.execute.before)
// ---------------------------------------------------------------------------

const IRONLAW_DIR = path.join(
  os.homedir(),
  ".claude",
  "state",
  "codex-iron-law-satisfied",
);

function ironLawMarkerPath(sessionID: string): string {
  return path.join(IRONLAW_DIR, sessionID);
}

function ensureIronLawDir(): void {
  try {
    fs.mkdirSync(IRONLAW_DIR, { recursive: true });
  } catch {
    /* best-effort */
  }
}

function ironLawSatisfied(sessionID: string): boolean {
  ensureIronLawDir();
  try {
    return fs.existsSync(ironLawMarkerPath(sessionID));
  } catch {
    return false;
  }
}

function markIronLawSatisfied(sessionID: string): void {
  ensureIronLawDir();
  try {
    fs.writeFileSync(ironLawMarkerPath(sessionID), "", "utf-8");
  } catch {
    /* best-effort */
  }
}

function clearIronLawMarker(sessionID: string): void {
  try {
    fs.rmSync(ironLawMarkerPath(sessionID), { force: true });
  } catch {
    /* best-effort */
  }
}

// ---------------------------------------------------------------------------
// Tool classification — Iron Law gate. Kept in sync with opencode/keel.ts so
// the gate fires identically across hosts. Edit this set in both files.
// ---------------------------------------------------------------------------

const READING_TOOL_NAMES = new Set([
  "read", "glob", "grep",
  "lsp", "lsp_diagnostics", "lsp_goto_definition",
  "lsp_find_references", "lsp_symbols", "lsp_prepare_rename",
]);

const EDIT_CLASS_TOOL_NAMES = new Set([
  "edit", "write", "multiedit", "notebookedit",
  "apply_patch", "str_replace", "patch",
]);

const KEEL_READING_COMMANDS = [
  "keel system-map",
  "keel recall",
  "keel doctor",
  "keel code-search",
  "keel observe",
  "keel workflow cockpit",
  "keel workflow status",
];

function isReadingTool(toolName: string): boolean {
  return READING_TOOL_NAMES.has(toolName.toLowerCase());
}

function isEditClassTool(toolName: string): boolean {
  return EDIT_CLASS_TOOL_NAMES.has(toolName.toLowerCase());
}

function isKeelReadingCommand(command: string): boolean {
  const c = command.trim();
  return KEEL_READING_COMMANDS.some((k) => c.startsWith(k));
}

// Codex PreToolUse deny output: a hookSpecificOutput with permissionDecision
// "deny" blocks the tool call and surfaces the reason to the model. Per the
// official Codex hooks spec, PreToolUse may return allow/deny/ask.
function denyOutput(reason: string): string {
  return JSON.stringify({
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: reason,
    },
  });
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

const SHELL_TOOL_NAMES = new Set([
  "bash", "shell", "sh", "zsh", "fish", "powershell", "pwsh", "cmd",
]);

function isShellTool(toolName: string): boolean {
  return SHELL_TOOL_NAMES.has(toolName.toLowerCase());
}

function extractCommand(toolInput: unknown): string {
  if (toolInput && typeof toolInput === "object" && "command" in toolInput) {
    const cmd = (toolInput as { command?: unknown }).command;
    return typeof cmd === "string" ? cmd : "";
  }
  return "";
}

function handlePreToolUse(input: CodexHookInput, isPre: boolean): string {
  // PostToolUse (isPre === false) only records observations — no gate, no rewrite.
  if (!isPre) {
    const sessionID = input.session_id ?? "unknown";
    const cwd = input.cwd ?? process.cwd();
    const toolName = input.tool ?? "";
    const stdin = input.tool_input != null
      ? JSON.stringify(input.tool_input)
      : "{}";
    const args = ["--session", sessionID, "--cwd", cwd, "--tool", toolName];
    if (input.failed) args.push("--failed");
    runBridgeWithStdin("observe", args, stdin);
    return "";
  }

  // --- PreToolUse: Iron Law enforcement first, then observe + rewrite. ---
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();
  const toolName = input.tool ?? "";

  // Reading tools satisfy the Iron Law gate and are always allowed.
  if (isReadingTool(toolName)) {
    markIronLawSatisfied(sessionID);
  }

  // Shell tools running keel reading commands also satisfy the gate.
  if (isShellTool(toolName)) {
    const command = extractCommand(input.tool_input);
    if (command && isKeelReadingCommand(command)) {
      markIronLawSatisfied(sessionID);
    }
  }

  // Iron Law enforcement: edit-class tools are DENIED until the model has
  // demonstrated reading behavior this session. This is the enforcement
  // mechanism — not just text injection, but an actual block. Mirrors the
  // OpenCode adapter's tool.execute.before deny-on-throw behavior.
  if (isEditClassTool(toolName) && !ironLawSatisfied(sessionID)) {
    return denyOutput(
      "IRON LAW ENFORCED: Read first. You must use Read, Glob, Grep, or keel tools (system-map, recall, doctor, code-search) BEFORE editing. The iron law is enforced — you cannot skip it.",
    );
  }

  // Fire-and-forget observation (PreToolUse). Codex hooks are synchronous, so
  // we call observe with a short timeout and discard the result.
  const stdin = input.tool_input != null
    ? JSON.stringify(input.tool_input)
    : "{}";
  const observeArgs = ["--session", sessionID, "--cwd", cwd, "--tool", toolName];
  if (input.failed) observeArgs.push("--failed");
  runBridgeWithStdin("observe", observeArgs, stdin);

  // Iron Law edit-class gate for tools that have already satisfied reading.
  // keel bridge pre-tool-use is the host-neutral edit gate (wired in
  // wb-19f01d1d0a1). It returns "KEEL_GATE_DENY\n<reason>" to deny; on deny
  // we block the edit via Codex's permissionDecision: "deny".
  if (isEditClassTool(toolName)) {
    const gate = runBridge("pre-tool-use", [
      "--session", sessionID,
      "--cwd", cwd,
      "--tool", toolName,
    ]);
    if (gate.startsWith("KEEL_GATE_DENY")) {
      const reason = gate.split("\n").slice(1).join("\n").trim();
      return denyOutput(
        reason ||
          "keel Iron Law gate: read context + load a skill before editing.",
      );
    }
  }

  // Compaction reroute for shell tools.
  if (isShellTool(toolName)) {
    const command = extractCommand(input.tool_input);
    if (command) {
      const rewrite = runBridgeWithStdin("rewrite", ["--tool", toolName], command);
      if (rewrite.startsWith("KEEL_REWRITE ")) {
        const rewritten = rewrite.slice("KEEL_REWRITE ".length).trim();
        if (rewritten) {
          return JSON.stringify({
            hookSpecificOutput: {
              hookEventName: "PreToolUse",
              permissionDecision: "allow",
              updatedInput: { command: rewritten },
            },
          });
        }
      }
    }
  }

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
  clearIronLawMarker(sessionID);
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
        contextText = handlePreToolUse(input, true);
        break;
      case "PostToolUse":
        handlePreToolUse(input, false);
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
