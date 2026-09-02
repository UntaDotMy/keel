#!/usr/bin/env node

// ---------------------------------------------------------------------------
// keel Pi Agent Extension — bridges Pi lifecycle events to `keel bridge`.
//
// Pi's extension API exposes a rich event set (see
// https://pi.dev/docs/latest/extensions). This extension wires the subset
// that keel's host-neutral `keel bridge` surface owns, mirroring the OpenCode
// adapter's coverage so a Pi session gets the same keel discipline as Claude
// Code or Codex:
//
//   session_start           -> keel bridge session-start   (bootstrap + digest, once)
//   input / message_start   -> keel bridge user-prompt     (per-prompt context)
//   tool_call (edit-class)  -> keel bridge pre-tool-use    (Iron Law edit gate; block on deny)
//   tool_call (bash)        -> keel bridge rewrite         (compaction reroute, in place)
//   tool_execution_end      -> keel bridge observe          (tool observation, fire-and-forget)
//   session_compact         -> keel bridge pre-compact + post-compact
//                             (learning checkpoint + post-compaction context)
//   session_shutdown        -> keel bridge session-end      (learning + session capture)
//
// Pi's tool_call contract (from official docs):
//   - event.input is mutable. Mutate it in place to patch tool arguments.
//   - Return value only controls blocking: { block: true, reason?: string }.
//   - Returning undefined (or nothing) allows execution with the mutation.
//   - Later tool_call handlers see mutations made by earlier handlers.
//
// Install: this extension is auto-discovered from
//   ~/.pi/agent/extensions/keel-pi.ts        (global)
//   .pi/extensions/keel-pi.ts               (project-local)
// or referenced explicitly via the `extensions` array in
//   ~/.pi/agent/settings.json (global) / .pi/settings.json (project).
// `keel install --with pi` wires the global copy automatically.
// ---------------------------------------------------------------------------

import { execFileSync } from "node:child_process";
import {
  clearIronLawMarker,
  clearSessionStarted,
  hasSessionStarted,
  ironLawSatisfied,
  isAlreadyCompacted,
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
// Types — minimal Pi ExtensionAPI surface (no hard dep on @earendil-works/pi)
// ---------------------------------------------------------------------------

interface PiToolCallInput {
  command?: string;
  [key: string]: unknown;
}

interface PiToolCallEvent {
  toolName?: string;
  toolCallId?: string;
  input?: PiToolCallInput;
}

interface PiToolExecutionEndEvent {
  toolName?: string;
  toolCallId?: string;
  error?: unknown;
  [key: string]: unknown;
}

interface PiSessionLikeEvent {
  sessionId?: string;
  cwd?: string;
  [key: string]: unknown;
}

interface PiMessageEvent {
  sessionId?: string;
  cwd?: string;
  role?: string;
  // Pi exposes message text via a `text` or `parts` field depending on version
  text?: string;
  parts?: unknown[];
  [key: string]: unknown;
}

interface PiExtensionContext {
  cwd?: string;
  sessionId?: string;
  getSystemPromptOptions?: () => { contextFiles?: unknown[]; [k: string]: unknown };
  [key: string]: unknown;
}

interface PiBlockResult {
  block: true;
  reason?: string;
}

interface PiExtensionAPI {
  // Handlers may return undefined/void to allow the action (with any in-place
  // mutation to event.input), or { block: true, reason? } to block it.
  on(
    event: string,
    handler: (event: any, ctx?: PiExtensionContext) => void | Promise<void> | undefined | PiBlockResult | Promise<PiBlockResult | undefined>,
  ): void;
  [key: string]: unknown;
}

const BRIDGE_BIN: string = resolveBinary();
const STARTED_DIR = sessionMarkerDirectory("pi");

function hasStarted(sessionID: string): boolean {
  return hasSessionStarted(STARTED_DIR, sessionID);
}

function markStarted(sessionID: string): void {
  markSessionStarted(STARTED_DIR, sessionID);
}

function clearStarted(sessionID: string): void {
  clearSessionStarted(STARTED_DIR, sessionID);
}

// ---------------------------------------------------------------------------
// Bridge invocation — print plain text, always exit 0 (never blocks a turn)
// ---------------------------------------------------------------------------

function runBridge(
  subcommand: string,
  args: string[],
  stdinInput?: string,
  timeoutMs = 5000,
): string {
  try {
    const result = execFileSync(BRIDGE_BIN, ["bridge", subcommand, ...args], {
      timeout: timeoutMs,
      input: stdinInput,
      stdio: ["pipe", "pipe", "pipe"],
      encoding: "utf-8",
      windowsHide: true,
    });
    return (result ?? "").trim();
  } catch {
    // Fail open — no context injected.
    return "";
  }
}

function bridgeRewrite(command: string, toolName: string): string {
  // why: --tool is required; without it `bridge rewrite` sees an empty tool
  // name, fails the shell-tool check, and returns nothing, so no command is
  // ever rerouted (mirrors the cursor adapter fix).
  return runBridge("rewrite", ["--tool", toolName], command, 500);
}


// ---------------------------------------------------------------------------
// Context injection helpers
// ---------------------------------------------------------------------------

function resolveSessionId(event: PiSessionLikeEvent, ctx?: PiExtensionContext): string {
  const raw =
    event?.sessionId ||
    ctx?.sessionId ||
    // PID-based fallback causes session collisions when multiple Pi sessions
    // run on the same machine. Use a unique identifier instead.
    (typeof crypto !== "undefined" && crypto.randomUUID
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`);
  return String(raw);
}

function resolveCwd(ctx?: PiExtensionContext): string {
  if (ctx?.cwd && typeof ctx.cwd === "string") return ctx.cwd;
  try {
    return process.cwd();
  } catch {
    return "";
  }
}

// Extract user message text from a Pi message event (field shape varies).
function extractUserText(event: PiMessageEvent): string {
  if (typeof event?.text === "string" && event.text.trim()) return event.text;
  const parts = event?.parts;
  if (Array.isArray(parts)) {
    const collected: string[] = [];
    for (const part of parts) {
      if (part && typeof part === "object") {
        const text = (part as { text?: unknown }).text;
        if (typeof text === "string" && text.trim()) collected.push(text);
      } else if (typeof part === "string" && part.trim()) {
        collected.push(part);
      }
    }
    if (collected.length) return collected.join("\n");
  }
  return "";
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

function handleSessionStart(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  if (hasStarted(sessionID)) return;
  markStarted(sessionID);

  // session-start prints the bootstrap + workspace digest and self-heals the
  // MCP registration. Running it (even if the text is discarded) keeps the
  // session's keel state coherent. The persistent Iron Law itself rides in
  // the AGENTS.md context file, loaded into the system prompt at startup.
  runBridge("session-start", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
}

function handleUserPrompt(event: PiMessageEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  const promptText = extractUserText(event);
  if (!promptText) return;

  // user-prompt composes the per-prompt context (skill brief + iron law +
  // pointers). The bridge reads the prompt from the --prompt flag (not stdin)
  // and resolves the workspace from --cwd, so both must be passed. As with
  // session-start, running it keeps the session's keel state coherent and drives
  // skill routing; the persistent Iron Law itself rides in the AGENTS.md context
  // file loaded into the system prompt, and the model can call skill_route /
  // context_brief via the MCP tools. The stdout is not injected into model
  // context here (Pi has no seam for it on this event), matching session-start.
  runBridge("user-prompt", [
    "--session",
    sessionID,
    "--cwd",
    resolveCwd(ctx),
    "--prompt",
    promptText,
  ]);
}

/**
 * tool_call handler. Fires BEFORE tool execution. Two responsibilities:
 *   1. Iron Law enforcement — block edit-class tools until the model has
 *      demonstrated reading behavior this session (mirrors OpenCode).
 *   2. Compaction reroute — rewrite noisy bash commands in place via
 *      `keel bridge rewrite`.
 *
 * Return contract: undefined = allow (with any in-place mutation);
 * { block: true, reason } = block.
 */
function handleToolCall(
  event: PiToolCallEvent,
  ctx?: PiExtensionContext,
): { block: true; reason: string } | undefined {
  const sessionID = resolveSessionId(event as PiSessionLikeEvent, ctx);
  const toolName = (event?.toolName || "").toLowerCase();
  const editTool = isEditClassTool(toolName);
  const shellTool = isShellTool(toolName);
  try {
    const command = shellTool && typeof event?.input?.command === "string"
      ? event.input.command
      : "";
    const readingCommand = command ? isKeelReadingCommand(command) : false;
    if (editTool || shellTool) {
      const gateArgs = [
        "--session",
        sessionID,
        "--cwd",
        resolveCwd(ctx),
        "--tool",
        toolName,
      ];
      if (command) gateArgs.push("--command", command);
      const gateResult = parseGateResponse(runBridge("pre-tool-use", gateArgs, 5000));
      if (gateResult.status === "deny") {
        return {
          block: true,
          reason:
            gateResult.reason ||
            (readingCommand
              ? "keel reading command gate denied this operation."
              : "keel Iron Law gate: call system_map/recall/context_brief before the operation."),
        };
      }
      if (gateResult.status !== "allow") {
        return {
          block: true,
          reason:
            "keel Iron Law gate could not be evaluated; retry after running `keel doctor`.",
        };
      }
    }

    if (editTool && !ironLawSatisfied(sessionID)) {
      return {
        block: true,
        reason:
          "IRON LAW ENFORCED (STRICT): Use a keel reading tool before editing.",
      };
    }

    if (shellTool && command && !isAlreadyCompacted(command)) {
      const rewritten = parseRewriteResponse(runBridge("rewrite", [
        "--tool",
        toolName,
        "--command",
        command,
      ], 500));
      if (rewritten && rewritten !== command && event?.input) {
        event.input.command = rewritten;
      }
    }
    return undefined;
  } catch (err) {
    if (editTool || shellTool) {
      return {
        block: true,
        reason: "keel adapter gate failed closed; retry after running `keel doctor`.",
      };
    }
    console.warn("[keel] unexpected error in tool_call handler", err);
    return undefined;
  }
}

function handleToolExecutionEnd(
  event: PiToolExecutionEndEvent,
  ctx?: PiExtensionContext,
): void {
  // Fire-and-forget observation capture. `bridge observe` reads the tool name,
  // cwd, and failed state from FLAGS (--tool/--cwd/--failed), not from the stdin
  // JSON — stdin is the tool_input used only for shell-command extraction, which
  // is absent at execution end. Passing the tool via a JSON body left the
  // observation recorded with an empty tool name; the flags fix that.
  const sessionID = resolveSessionId(event as PiSessionLikeEvent, ctx);
  const args = [
    "--session",
    sessionID,
    "--cwd",
    resolveCwd(ctx),
    "--tool",
    event?.toolName ?? "",
    "--phase",
    "post",
  ];
  if (event?.error) args.push("--failed");
  runBridge("observe", args, "{}", 2000);
}

function handlePostCompact(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  // Pre-window checkpoint, then post-compaction learning + context (idempotent).
  runBridge("pre-compact", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
  runBridge("post-compact", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
}

function handleSessionShutdown(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  try {
    runBridge("session-end", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
  } finally {
    clearStarted(sessionID);
    clearIronLawMarker(sessionID);
  }
}

// ---------------------------------------------------------------------------
// Extension entry point
// ---------------------------------------------------------------------------

function setup(pi: PiExtensionAPI): void {
  // session_start — bootstrap + digest, once per session.
  pi.on("session_start", (event: PiSessionLikeEvent, ctx?: PiExtensionContext) => {
    try {
      handleSessionStart(event, ctx);
    } catch (err) {
      console.error("[keel-pi] session_start handler error:", err);
    }
  });

  // user input — per-prompt context brief. Pi fires `input` for raw user
  // input; some versions also fire `message_start` with role=user.
  pi.on("input", (event: PiMessageEvent, ctx?: PiExtensionContext) => {
    try {
      handleUserPrompt(event, ctx);
    } catch {
      /* degrade */
    }
  });
  pi.on("message_start", (event: PiMessageEvent, ctx?: PiExtensionContext) => {
    try {
      if (event?.role === "user") handleUserPrompt(event, ctx);
    } catch {
      /* degrade */
    }
  });

  // tool_call — Iron Law edit gate + compaction reroute (before execution).
  pi.on("tool_call", (event: PiToolCallEvent, ctx?: PiExtensionContext) => {
    return handleToolCall(event, ctx);
  });

  // tool_execution_end — observation capture (fire-and-forget).
  pi.on("tool_execution_end", (event: PiToolExecutionEndEvent, ctx?: PiExtensionContext) => {
    try {
      handleToolExecutionEnd(event, ctx);
    } catch {
      /* degrade */
    }
  });

  // Compaction cycle: pre-compact checkpoint + post-compact context on the event.
  pi.on("session_compact", (event: PiSessionLikeEvent, ctx?: PiExtensionContext) => {
    try {
      handlePostCompact(event, ctx);
    } catch {
      /* degrade */
    }
  });

  // session_shutdown — learning + session capture + marker cleanup.
  pi.on("session_shutdown", (event: PiSessionLikeEvent, ctx?: PiExtensionContext) => {
    try {
      handleSessionShutdown(event, ctx);
    } catch {
      /* degrade */
    }
  });
}

// Pi loads the extension via its default export or a named `setup` export.
export default setup;
export { setup };
