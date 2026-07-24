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
// Shared with Rust core / OpenCode / Codex (STRICT keel-required).
const IRONLAW_DIR = path.join(STATE_DIR, "iron-law-satisfied");

/** Match Rust `sanitize_memory_key`: lowercase alnum, other runs → single `-`. */
function sanitizeSessionKey(sessionID: string): string {
  const raw = (sessionID || "default").trim() || "default";
  return (
    raw
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "default"
  );
}

function markerPath(dir: string, sessionID: string): string {
  const key =
    dir === IRONLAW_DIR ? sanitizeSessionKey(sessionID) : sessionID || "default";
  return path.join(dir, key);
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
  ensureDir(IRONLAW_DIR);
  try {
    fs.writeFileSync(markerPath(IRONLAW_DIR, sessionID), "satisfied", "utf-8");
  } catch {
    /* best-effort */
  }
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

function bridgeRewrite(command: string, toolName: string): string {
  // why: --tool is required; without it `bridge rewrite` sees an empty tool
  // name, fails the shell-tool check, and returns nothing, so no command is
  // ever rerouted (mirrors the cursor adapter fix).
  return runBridge("rewrite", ["--tool", toolName], command, 500);
}

// ---------------------------------------------------------------------------
// Iron Law classification. STRICT: keel research tools only (sync with OpenCode).
// ---------------------------------------------------------------------------

const EDIT_CLASS_TOOL_NAMES = new Set([
  "edit", "write", "multiedit", "notebookedit",
  "apply_patch", "str_replace", "patch",
]);

const SHELL_TOOL_NAMES = new Set([
  "bash", "shell", "sh", "zsh", "fish", "powershell", "pwsh", "cmd",
]);

function isEditClassTool(toolName: string): boolean {
  return EDIT_CLASS_TOOL_NAMES.has(toolName.toLowerCase());
}

function isKeelResearchTool(toolName: string): boolean {
  const lower = toolName.toLowerCase();
  if (
    lower.includes("install") ||
    lower.includes("uninstall") ||
    lower.includes("self-replace") ||
    lower.includes("self_replace")
  ) {
    return false;
  }
  return (
    lower.includes("mcp__keel__") ||
    lower.includes("keel__") ||
    lower.startsWith("keel_") ||
    lower === "keel"
  );
}

function isShellTool(toolName: string): boolean {
  return SHELL_TOOL_NAMES.has(toolName.toLowerCase());
}

function isKeelReadingCommand(command: string): boolean {
  const trimmed = command.trim().toLowerCase();
  const body = trimmed.startsWith("keel run -- ")
    ? trimmed.slice("keel run -- ".length)
    : trimmed;
  if (!(body.startsWith("keel ") || body.includes("keel.exe"))) {
    return false;
  }
  return (
    body.includes("system-map") ||
    body.includes("system_map") ||
    body.includes("recall") ||
    body.includes("doctor") ||
    body.includes("code-search") ||
    body.includes("code_search") ||
    body.includes("skill") ||
    body.includes("context") ||
    body.includes("memory") ||
    body.includes("status") ||
    body.includes("help") ||
    body.includes("brief")
  );
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
  try {
    const sessionID = resolveSessionId(event as PiSessionLikeEvent, ctx);
    const toolName = (event?.toolName || "").toLowerCase();

    // STRICT: only keel research tools clear the shared marker (not plain Read).
    if (isKeelResearchTool(toolName)) {
      markIronLawSatisfied(sessionID);
      return undefined;
    }
    if (isShellTool(toolName)) {
      const cmd = event?.input?.command;
      if (typeof cmd === "string" && isKeelReadingCommand(cmd)) {
        markIronLawSatisfied(sessionID);
      }
    }

    // Edit-class: prefer Rust core decision; also block when local marker absent.
    if (isEditClassTool(toolName)) {
      const gate = runBridge("pre-tool-use", [
        "--session",
        sessionID,
        "--cwd",
        resolveCwd(ctx),
        "--tool",
        toolName,
      ]);
      if (gate.startsWith("KEEL_GATE_DENY")) {
        const reason = gate.split("\n").slice(1).join("\n").trim();
        return {
          block: true,
          reason:
            reason ||
            "keel Iron Law gate: call system_map/recall/context_brief before editing.",
        };
      }
      if (!ironLawSatisfied(sessionID)) {
        return {
          block: true,
          reason:
            "IRON LAW ENFORCED (STRICT): Use a keel tool first (MCP system_map, recall, context_brief, skill_route, or `keel doctor` / `keel code-search`). Plain Read does not clear the gate.",
        };
      }
    }

    // --- Compaction reroute for shell tools. ---
    if (isShellTool(toolName)) {
      const cmd = event?.input?.command;
      if (typeof cmd === "string" && cmd.trim() !== "" && !cmd.startsWith("keel run --")) {
        const rewrite = bridgeRewrite(cmd, toolName);
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
  ];
  if (event?.error) args.push("--failed");
  runBridge("observe", args, "{}", 500);
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

  // compaction cycle. Only post-compact is wired: there is no `bridge
  // pre-compact` subcommand, so no session_before_compact handler is registered
  // (it would only invoke a nonexistent subcommand). Learning + post-compaction
  // context runs here, on the actual compaction event.
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
