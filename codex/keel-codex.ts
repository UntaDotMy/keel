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

import * as fs from "node:fs";
import { execFileSync } from "node:child_process";
import {
  clearIronLawMarker,
  clearSessionStarted,
  hasSessionStarted,
  isEditClassTool,
  isKeelReadingCommand,
  isShellTool,
  markSessionStarted,
  parseGateResponse,
  parseRewriteResponse,
  resolveBinary,
  sessionMarkerDirectory,
} from "../_shared/ts/bridge-core";

// ---------------------------------------------------------------------------
// Types — Codex hook stdin payload (subset we use)
// ---------------------------------------------------------------------------

interface CodexHookInput {
  // Official Codex fields. Legacy aliases remain accepted for older hosts.
  hook_event_name?: string;
  event?: string;
  session_id?: string;
  cwd?: string;
  source?: string; // "startup" | "resume" | "clear" | "compact"
  start_source?: string;
  tool_name?: string;
  tool?: string;
  tool_input?: unknown;
  tool_response?: unknown;
  failed?: boolean;
  prompt?: string;
  turn_data?: Record<string, unknown>;
  trigger?: string; // "manual" | "auto"
  [key: string]: unknown;
}

function eventName(input: CodexHookInput): string {
  return input.hook_event_name ?? input.event ?? "";
}

function toolName(input: CodexHookInput): string {
  return input.tool_name ?? input.tool ?? "";
}


function toolFailed(input: CodexHookInput): boolean {
  if (typeof input.failed === "boolean") {
    return input.failed;
  }
  if (input.tool_response && typeof input.tool_response === "object") {
    const response = input.tool_response as Record<string, unknown>;
    return response.isError === true || response.error != null;
  }
  return false;
}

const BRIDGE_BIN: string = resolveBinary();
const STARTED_DIR = sessionMarkerDirectory("codex");

function hasStarted(sessionID: string): boolean {
  return hasSessionStarted(STARTED_DIR, sessionID);
}

function markStarted(sessionID: string): void {
  markSessionStarted(STARTED_DIR, sessionID);
}

function clearMarker(sessionID: string): void {
  clearSessionStarted(STARTED_DIR, sessionID);
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

function runBridge(subcommand: string, args: string[], timeoutMs = 500): string {
  try {
    const result = execFileSync(
      BRIDGE_BIN,
      ["bridge", subcommand, ...args],
      {
        timeout: timeoutMs,
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


function extractCommand(toolInput: unknown): string {
  if (toolInput && typeof toolInput === "object" && "command" in toolInput) {
    const cmd = (toolInput as { command?: unknown }).command;
    return typeof cmd === "string" ? cmd : "";
  }
  return "";
}

function handlePreToolUse(input: CodexHookInput, isPre: boolean): string {
  // PostToolUse only records the official tool_response payload.
  if (!isPre) {
    const sessionID = input.session_id ?? "unknown";
    const cwd = input.cwd ?? process.cwd();
    const currentToolName = toolName(input);
    const observation = input.tool_response ?? input.tool_input;
    const stdin = observation != null ? JSON.stringify(observation) : "{}";
    const args = [
      "--session", sessionID, "--cwd", cwd, "--tool", currentToolName,
      "--phase", "post",
    ];
    if (toolFailed(input)) args.push("--failed");
    runBridgeWithStdin("observe", args, stdin);
    return "";
  }

  // --- PreToolUse: Iron Law enforcement first, then observe + rewrite. ---
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();
  const currentToolName = toolName(input);


  // Fire-and-forget pre-tool observation; it cannot satisfy the post-tool marker.
  const stdin = input.tool_input != null
    ? JSON.stringify(input.tool_input)
    : "{}";
  const observeArgs = [
    "--session", sessionID, "--cwd", cwd, "--tool", currentToolName,
    "--phase", "pre",
  ];
  if (toolFailed(input)) observeArgs.push("--failed");
  runBridgeWithStdin("observe", observeArgs, stdin);

  // Edit-class: Rust core is source of truth (evidence-based deny). This gate
  // is fail-CLOSED: an empty result means the bridge timed out or errored.
  if (isEditClassTool(currentToolName)) {
    const gate = runBridge(
      "pre-tool-use",
      ["--session", sessionID, "--cwd", cwd, "--tool", currentToolName],
      5000,
    );
    const gateResult = parseGateResponse(gate);
    if (gateResult.status === "deny") {
      return denyOutput(
        gateResult.reason ||
          "keel Iron Law gate: call system_map/recall/context_brief before editing.",
      );
    }
    if (gateResult.status !== "allow") {
      return denyOutput(
        "keel Iron Law gate could not be evaluated (keel did not respond in time). Retry the edit; if it persists, run `keel doctor`.",
      );
    }
  }

  // Shell tools use the same Rust gate before compaction rewrite.
  if (isShellTool(currentToolName)) {
    const command = extractCommand(input.tool_input);
    const readingCommand = command ? isKeelReadingCommand(command) : false;
    const gateArgs = [
      "--session", sessionID, "--cwd", cwd, "--tool", currentToolName,
    ];
    if (command) gateArgs.push("--command", command);
    const gate = runBridge("pre-tool-use", gateArgs, 5000);
    const gateResult = parseGateResponse(gate);
    if (gateResult.status === "deny") {
      return denyOutput(
        gateResult.reason ||
          (readingCommand
            ? "keel reading command gate denied this command."
            : "keel Iron Law gate: call system_map/recall/context_brief before running shell commands."),
      );
    }
    if (gateResult.status !== "allow") {
      return denyOutput(
        "keel Iron Law shell gate could not be evaluated. Retry after running `keel doctor`.",
      );
    }
    if (command) {
      const rewritten = parseRewriteResponse(
        runBridgeWithStdin("rewrite", ["--tool", currentToolName], command),
      );
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

  return "";
}

function handlePreCompact(input: CodexHookInput): string {
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();

  // Pre-compact: persist what was learned before the window is rewritten.
  runBridge("pre-compact", [
    "--session", sessionID,
    "--cwd", cwd,
  ]);
  return "";
}

function handlePostCompact(input: CodexHookInput): string {
  // PostCompact: learning upsert (idempotent) + post-compaction context.
  const sessionID = input.session_id ?? "unknown";
  const cwd = input.cwd ?? process.cwd();

  return runBridge("post-compact", [
    "--session", sessionID,
    "--cwd", cwd,
  ]);
}

function handleStop(_input: CodexHookInput): string {
  // Stop fires on EVERY turn end. It must NOT run `bridge post-compact`: that
  // subcommand runs the full session-end learning cycle, so invoking it per turn
  // spawned and SIGTERM-killed a learning cycle every turn and discarded its
  // output. The learning checkpoint belongs on the actual compaction event
  // (handlePostCompact) and on session end (handleSessionEnd). Printing context
  // on Stop also risks a keep-going loop, so this handler is silenced (matches
  // the native Claude Code Stop handler and the OpenCode adapter, which do not
  // run learning on turn end).
  return "";
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
    switch (eventName(input)) {
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
      case "PostCompact": {
        const postCompactContext = handlePostCompact(input);
        contextText = postCompactContext
          ? JSON.stringify({
              hookSpecificOutput: {
                hookEventName: "PostCompact",
                additionalContext: postCompactContext,
              },
            })
          : "";
        break;
      }
      case "Stop":
        contextText = handleStop(input);
        break;
      case "SessionEnd":
        handleSessionEnd(input);
        break;
      default:
        // Unknown event — exit silently.
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
