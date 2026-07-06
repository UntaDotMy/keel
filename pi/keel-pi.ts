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
//   session_before_compact  -> keel bridge pre-compact      (pre-compaction checkpoint)
//   session_compact         -> keel bridge post-compact     (post-compaction context + learning)
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

import * as os from "node:os";
import * as fs from "node:fs";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

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
// Marker-file helpers
// ---------------------------------------------------------------------------

const STATE_DIR = path.join(os.homedir(), ".claude", "state");
const STARTED_DIR = path.join(STATE_DIR, "pi-session-started");
// Per-session Iron Law satisfaction marker (mirrors the OpenCode adapter).
const IRONLAW_DIR = path.join(STATE_DIR, "pi-iron-law-satisfied");

function markerPath(dir: string, sessionID: string): string {
  return path.join(dir, sessionID);
}

function ensureDir(dir: string): void {
  try {
    fs.mkdirSync(dir, { recursive: true });
  } catch {
    /* best-effort */
  }
}

function hasMarker(dir: string, sessionID: string): boolean {
  ensureDir(dir);
  try {
    return fs.existsSync(markerPath(dir, sessionID));
  } catch {
    return false;
  }
}

function setMarker(dir: string, sessionID: string): void {
  ensureDir(dir);
  try {
    fs.writeFileSync(markerPath(dir, sessionID), "", "utf-8");
  } catch {
    /* best-effort */
  }
}

function clearMarker(dir: string, sessionID: string): void {
  try {
    fs.rmSync(markerPath(dir, sessionID), { force: true });
  } catch {
    /* best-effort */
  }
}

function hasStarted(sessionID: string): boolean {
  return hasMarker(STARTED_DIR, sessionID);
}

function markStarted(sessionID: string): void {
  setMarker(STARTED_DIR, sessionID);
}

function clearStarted(sessionID: string): void {
  clearMarker(STARTED_DIR, sessionID);
}

function ironLawSatisfied(sessionID: string): boolean {
  return hasMarker(IRONLAW_DIR, sessionID);
}

function markIronLawSatisfied(sessionID: string): void {
  setMarker(IRONLAW_DIR, sessionID);
}

// ---------------------------------------------------------------------------
// Bridge invocation — print plain text, always exit 0 (never blocks a turn)
// ---------------------------------------------------------------------------

function runBridge(
  subcommand: string,
  args: string[],
  stdinInput?: string,
  timeoutMs = 500,
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

function bridgeRewrite(command: string): string {
  return runBridge("rewrite", [], command, 500);
}

// ---------------------------------------------------------------------------
// Iron Law classification. Kept in sync with opencode/keel.ts so the gate
// fires identically across hosts. Edit this set in both files.
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

const SHELL_TOOL_NAMES = new Set([
  "bash", "shell", "sh", "zsh", "fish", "powershell", "pwsh", "cmd",
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

function isShellTool(toolName: string): boolean {
  return SHELL_TOOL_NAMES.has(toolName.toLowerCase());
}

function isKeelReadingCommand(command: string): boolean {
  const c = command.trim();
  return KEEL_READING_COMMANDS.some((k) => c.startsWith(k));
}

// ---------------------------------------------------------------------------
// Context injection helpers
// ---------------------------------------------------------------------------

function resolveSessionId(event: PiSessionLikeEvent, ctx?: PiExtensionContext): string {
  const raw =
    event?.sessionId ||
    ctx?.sessionId ||
    (typeof process !== "undefined" ? String((process as any).pid ?? "") : "") ||
    "default";
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

  // user-prompt prints the composite per-prompt context (skill brief + iron
  // law + pointers) in one call. Surface it as a visible info line so the
  // user sees the routing decision; the model can also call skill_route /
  // context_brief directly via the MCP tools.
  const text = runBridge("user-prompt", ["--session", sessionID], promptText);
  if (text) {
    try {
      (console as unknown as { log?: (m: string) => void }).log?.(text);
    } catch {
      /* best-effort */
    }
  }
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
  try {
    const sessionID = resolveSessionId(event as PiSessionLikeEvent, ctx);
    const toolName = (event?.toolName || "").toLowerCase();

    // --- Iron Law: reading tools satisfy the gate and are always allowed. ---
    if (isReadingTool(toolName)) {
      markIronLawSatisfied(sessionID);
      return undefined;
    }

    // --- Iron Law: shell tools running keel reading commands satisfy the gate. ---
    if (isShellTool(toolName)) {
      const cmd = event?.input?.command;
      if (typeof cmd === "string" && isKeelReadingCommand(cmd)) {
        markIronLawSatisfied(sessionID);
      }
    }

    // --- Iron Law: block edit-class tools until the gate is satisfied. ---
    if (isEditClassTool(toolName) && !ironLawSatisfied(sessionID)) {
      return {
        block: true,
        reason:
          "IRON LAW ENFORCED: Read first. Use Read/Glob/Grep or keel tools (system-map, recall, doctor, code-search) BEFORE editing. The iron law is enforced — you cannot skip it.",
      };
    }

    // --- Iron Law edit-class gate for tools that have already satisfied reading. ---
    if (isEditClassTool(toolName)) {
      // pre-tool-use emits the per-edit gate reminder / decision. It is the
      // host-neutral Iron Law edit gate wired in wb-19f01d1d0a1. It does not
      // block (keel bridge always exits 0); the real block is the marker
      // check above. Running it records the gate state for session-end review.
      runBridge(
        "pre-tool-use",
        ["--session", sessionID, "--cwd", resolveCwd(ctx), "--tool", toolName],
      );
    }

    // --- Compaction reroute for shell tools. ---
    if (isShellTool(toolName)) {
      const cmd = event?.input?.command;
      if (typeof cmd === "string" && cmd.trim() !== "" && !cmd.startsWith("keel run --")) {
        const rewrite = bridgeRewrite(cmd);
        // keel bridge rewrite outputs "KEEL_REWRITE <cmd>" for noisy commands.
        if (rewrite.startsWith("KEEL_REWRITE ")) {
          const rewritten = rewrite.slice("KEEL_REWRITE ".length).trim();
          if (rewritten && rewritten !== cmd && event?.input) {
            // Pi's contract: mutate event.input in place.
            event.input.command = rewritten;
          }
        }
      }
    }

    // Returning undefined allows execution (with any in-place mutation).
    return undefined;
  } catch (err) {
    // Fail open — let the original tool call run unchanged.
    console.warn(
      "[keel] unexpected error in tool_call handler; passing through",
      err,
    );
    return undefined;
  }
}

function handleToolExecutionEnd(
  event: PiToolExecutionEndEvent,
  ctx?: PiExtensionContext,
): void {
  // Fire-and-forget observation capture. bridge observe reads a JSON payload
  // from stdin and records a memory observation; it never blocks.
  const sessionID = resolveSessionId(event as PiSessionLikeEvent, ctx);
  const payload = JSON.stringify({
    session_id: sessionID,
    cwd: resolveCwd(ctx),
    tool: event?.toolName ?? "",
    tool_call_id: event?.toolCallId ?? "",
    error: event?.error ? String(event.error) : undefined,
  });
  runBridge("observe", ["--session", sessionID], payload, 500);
}

function handlePreCompact(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  runBridge("pre-compact", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
}

function handlePostCompact(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  runBridge("post-compact", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
}

function handleSessionShutdown(event: PiSessionLikeEvent, ctx?: PiExtensionContext): void {
  const sessionID = resolveSessionId(event, ctx);
  try {
    runBridge("session-end", ["--session", sessionID, "--cwd", resolveCwd(ctx)]);
  } finally {
    clearStarted(sessionID);
    clearMarker(IRONLAW_DIR, sessionID);
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
    } catch {
      /* degrade */
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

  // compaction cycle.
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
