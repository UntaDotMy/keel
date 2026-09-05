#!/usr/bin/env node

// keel:managed-host-file (remove this line before customizing to opt out of upgrades)
// ---------------------------------------------------------------------------
// keel Pi Agent Extension — bridges Pi lifecycle events to `keel bridge`.
//
// Pi's extension API exposes a rich event set (see
// https://pi.dev/docs/latest/extensions). This extension wires the subset
// that keel's host-neutral `keel bridge` surface owns, mirroring the OpenCode
// adapter's coverage so a Pi session gets the same keel discipline as Claude
// Code or Codex:
//
//   session_start           -> keel bridge session-start   (bootstrap + digest, cached once)
//   before_agent_start      -> keel bridge user-prompt     (per-prompt system context)
//   tool_call (edit-class)  -> keel bridge pre-tool-use    (Iron Law edit gate; block on deny)
//   tool_call (bash)        -> keel bridge rewrite         (compaction reroute, in place)
//   tool_execution_end      -> keel bridge observe          (tool observation, fire-and-forget)
//   session_before_compact  -> keel bridge pre-compact      (learning checkpoint)
//   session_compact         -> keel bridge post-compact     (context cached for next turn)
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
  toolPathFromPayload,
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
  isError?: boolean;
  [key: string]: unknown;
}

interface PiSessionLikeEvent {
  sessionId?: string;
  cwd?: string;
  [key: string]: unknown;
}

interface PiBeforeAgentStartEvent extends PiSessionLikeEvent {
  prompt?: string;
  systemPrompt?: string;
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

interface PiBeforeAgentStartResult {
  systemPrompt: string;
}

interface PiExtensionAPI {
  // Handlers may return undefined/void to allow the action (with any in-place
  // mutation to event.input), or { block: true, reason? } to block it.
  on(
    event: string,
    handler: (
      event: any,
      ctx?: PiExtensionContext,
    ) =>
      | void
      | undefined
      | PiBlockResult
      | PiBeforeAgentStartResult
      | Promise<void | PiBlockResult | PiBeforeAgentStartResult | undefined>,
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

let moduleSessionId: string | undefined;

function resolveSessionId(event: PiSessionLikeEvent, ctx?: PiExtensionContext): string {
  if (event?.sessionId) return String(event.sessionId);
  if (ctx?.sessionId) return String(ctx.sessionId);
  if (!moduleSessionId) {
    moduleSessionId =
      (typeof crypto !== "undefined" && crypto.randomUUID
        ? crypto.randomUUID()
        : `${Date.now()}-${Math.random().toString(36).slice(2)}`);
  }
  return moduleSessionId;
}

const recentToolInputs = new Map<string, Record<string, unknown>>();
const pendingLifecycleContext = new Map<string, string>();

function resolveCwd(ctx?: PiExtensionContext): string {
  if (ctx?.cwd && typeof ctx.cwd === "string") return ctx.cwd;
  try {
    return process.cwd();
  } catch {
    return "";
  }
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

function handleSessionStart(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  if (hasStarted(sessionID)) return;
  markStarted(sessionID);

  const context = runBridge("session-start", [
    "--session",
    sessionID,
    "--cwd",
    resolveCwd(ctx),
  ]);
  if (context) pendingLifecycleContext.set(sessionID, context);
}

function handleBeforeAgentStart(
  event: PiBeforeAgentStartEvent,
  ctx?: PiExtensionContext,
): PiBeforeAgentStartResult | undefined {
  const sessionID = resolveSessionId(event, ctx);
  const promptContext = runBridge("user-prompt", [
    "--session",
    sessionID,
    "--cwd",
    resolveCwd(ctx),
    "--prompt",
    event?.prompt ?? "",
  ]);
  const lifecycleContext = pendingLifecycleContext.get(sessionID) ?? "";
  pendingLifecycleContext.delete(sessionID);
  const context = [lifecycleContext, promptContext].filter(Boolean).join("\n\n");
  if (!context) return undefined;
  return {
    systemPrompt: `${event?.systemPrompt ?? ""}\n\n${context}`,
  };
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
      const pathArg = toolPathFromPayload(event);
      if (pathArg) gateArgs.push("--path", pathArg);
      const gateResult = parseGateResponse(
        runBridge("pre-tool-use", gateArgs, undefined, 5000),
      );
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
        if (command && (isKeelReadingCommand(command) || command.trim().startsWith("keel doctor"))) {
          // allow recovery command through
        } else {
          return {
            block: true,
            reason:
              "keel Iron Law gate could not be evaluated; retry after running `keel doctor`.",
          };
        }
      }
    }

    if (editTool && !ironLawSatisfied(sessionID)) {
      return {
        block: true,
        reason:
          "IRON LAW ENFORCED (STRICT): Use a keel reading tool before editing.",
      };
    }

    if (event?.toolCallId && event?.input) {
      recentToolInputs.set(event.toolCallId, event.input);
    }

    if (shellTool && command && !isAlreadyCompacted(command)) {
      const rewritten = parseRewriteResponse(bridgeRewrite(command, toolName));
      if (rewritten && rewritten !== command && event?.input) {
        event.input.command = rewritten;
      }
    }
    return undefined;
  } catch (err) {
    if (editTool || shellTool) {
      const cmd = typeof event?.input?.command === "string" ? event.input.command : "";
      if (shellTool && cmd && (isKeelReadingCommand(cmd) || cmd.trim().startsWith("keel doctor"))) {
        return undefined;
      }
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
  if (event?.error || event?.isError) args.push("--failed");
  let stdin = "{}";
  if (event?.toolCallId && recentToolInputs.has(event.toolCallId)) {
    const savedInput = recentToolInputs.get(event.toolCallId);
    recentToolInputs.delete(event.toolCallId);
    if (savedInput) {
      stdin = JSON.stringify(savedInput);
    }
  }
  runBridge("observe", args, stdin, 2000);
}

function handlePreCompact(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  runBridge("pre-compact", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
}

function handlePostCompact(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  const context = runBridge("post-compact", [
    "--session",
    sessionID,
    "--cwd",
    resolveCwd(ctx),
  ]);
  if (context) pendingLifecycleContext.set(sessionID, context);
}

function handleSessionShutdown(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  try {
    runBridge("session-end", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
  } finally {
    clearStarted(sessionID);
    clearIronLawMarker(sessionID);
    pendingLifecycleContext.delete(sessionID);
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

  // Official Pi seam: this return value replaces the system prompt for this
  // turn, so bridge output reaches the model without accumulating messages.
  pi.on("before_agent_start", (event: PiBeforeAgentStartEvent, ctx?: PiExtensionContext) => {
    try {
      return handleBeforeAgentStart(event, ctx);
    } catch {
      return undefined;
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

  pi.on("session_before_compact", (event: PiSessionLikeEvent, ctx?: PiExtensionContext) => {
    try {
      handlePreCompact(event, ctx);
    } catch {
      /* degrade */
    }
  });

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
