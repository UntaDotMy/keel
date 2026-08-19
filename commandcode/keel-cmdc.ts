// keel Command Code (cmdc) Mod. Bridges cmdc lifecycle events to the host-
// neutral `keel bridge` surface. Install per commandcode/README.md.

import * as os from "node:os";
import * as fs from "node:fs";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

import type { ModApi, AgentMod, ModContext } from "@commandcode/harness";

const MOD_ID = "keel-cmdc";
const SESSION_STARTED_TYPE = "keel-cmdc/session-started";

// Binary resolution, resolved once at mod init
const BIN_NAME: string = os.platform() === "win32" ? "keel.exe" : "keel";

/** Resolve the bridge binary: $KEEL_HOME, ~/.keel, legacy ~/.claude, then PATH. */
function resolveBinary(): string {
  const home = os.homedir();
  const candidates: string[] = [];
  const envHome = process.env.KEEL_HOME;
  if (envHome && envHome.trim()) candidates.push(path.join(envHome.trim(), BIN_NAME));
  candidates.push(path.join(home, ".keel", BIN_NAME));
  candidates.push(path.join(home, ".claude", BIN_NAME));
  for (const candidate of candidates) {
    try {
      if (fs.existsSync(candidate)) return candidate;
    } catch {
      // fs failure, try the next candidate
    }
  }
  return BIN_NAME;
}

const BRIDGE_BIN: string = resolveBinary();

// Bridge runner. Never throws; 500ms hard timeout via execFileSync
function runBridge(
  subcommand: string,
  args: string[],
  timeoutMs = 500,
  stdin?: string,
): string {
  try {
    const result = execFileSync(
      BRIDGE_BIN,
      ["bridge", subcommand, ...args],
      {
        timeout: timeoutMs,
        stdio: ["pipe", "pipe", "pipe"],
        encoding: "utf-8",
        windowsHide: true,
        ...(stdin !== undefined ? { input: stdin } : {}),
      },
    );
    return result ?? "";
  } catch {
    return "";
  }
}

// Tool classification for the Iron Law gate (kept in sync with codex/keel-codex.ts)
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

function isShellTool(toolName: string): boolean {
  return SHELL_TOOL_NAMES.has(toolName.toLowerCase());
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

/** Mirrors the Rust `is_keel_research_command` HITS list; the doc-parity test
 *  `adapter_gate_lists_match_the_rust_source_of_truth` fails on drift. */
const KEEL_RESEARCH_SUBCOMMANDS = [
  "system-map", "system_map", "recall", "doctor", "code-search", "code_search",
  "skill-route", "skill_route", "skill-list", "skill_list", "skill-get", "skill_get",
  "context-brief", "context_brief", "memory status", "memory recall",
  "memory system-map", "memory scope", "anvil",
];

function isKeelReadingCommand(command: string): boolean {
  const trimmed = command.trim().toLowerCase();
  const body = trimmed.startsWith("keel run -- ")
    ? trimmed.slice("keel run -- ".length)
    : trimmed.startsWith("keel.exe run -- ")
      ? trimmed.slice("keel.exe run -- ".length)
      : trimmed;
  const hasKeel =
    body.startsWith("keel ") ||
    body.startsWith("keel.exe ") ||
    body.includes("\\keel.exe ") ||
    body.includes("/keel ") ||
    body.includes("\\keel ");
  if (!hasKeel) {
    return false;
  }
  if (/[&|;`\n]/.test(body) || body.includes("$(")) {
    return false;
  }
  return KEEL_RESEARCH_SUBCOMMANDS.some((hit) => body.includes(hit));
}

// Iron Law markers, SHARED with the Rust core:
// ~/.keel/state/iron-law-satisfied/<sanitized-session> (legacy ~/.claude)
function keelStateRoot(): string {
  const home = os.homedir();
  const envHome = process.env.KEEL_HOME;
  if (envHome && envHome.trim()) return path.join(envHome.trim(), "state");
  const neutralHome = path.join(home, ".keel");
  try {
    if (fs.existsSync(neutralHome)) return path.join(neutralHome, "state");
  } catch {
    // fall through to legacy
  }
  return path.join(home, ".claude", "state");
}

const IRONLAW_DIR = path.join(keelStateRoot(), "iron-law-satisfied");
const IRONLAW_DIR_LEGACY = path.join(
  os.homedir(),
  ".claude",
  "state",
  "iron-law-satisfied",
);

/** Match Rust `sanitize_memory_key`: lowercase alnum, other runs become a single `-`. */
function sanitizeSessionKey(sessionID: string): string {
  const raw = (sessionID || "default").trim() || "default";
  return (
    raw
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "workspace"
  );
}

function ironLawMarkerPath(sessionID: string): string {
  return path.join(IRONLAW_DIR, sanitizeSessionKey(sessionID));
}

function ironLawSatisfied(sessionID: string): boolean {
  try {
    if (fs.existsSync(ironLawMarkerPath(sessionID))) return true;
    return fs.existsSync(path.join(IRONLAW_DIR_LEGACY, sanitizeSessionKey(sessionID)));
  } catch {
    return false;
  }
}

function markIronLawSatisfied(sessionID: string): void {
  try {
    fs.mkdirSync(IRONLAW_DIR, { recursive: true });
    fs.writeFileSync(ironLawMarkerPath(sessionID), "satisfied", "utf-8");
  } catch {
    /* best-effort */
  }
}

function clearIronLawMarker(sessionID: string): void {
  try {
    fs.rmSync(ironLawMarkerPath(sessionID), { force: true });
    fs.rmSync(path.join(IRONLAW_DIR_LEGACY, sanitizeSessionKey(sessionID)), { force: true });
  } catch {
    /* best-effort */
  }
}

// Mod state helpers. Durable per-session bookkeeping via ModSessionApi
interface SessionStartedData {
  started: true;
}

/** True when a session-start bridge call has already been made for this session
 *  (durable across compaction + resume via custom entries). */
function hasSessionStarted(
  ctx: ModContext | undefined,
  sessionId: string,
): boolean {
  try {
    const entries = ctx?.session?.getCustomEntries({ customType: SESSION_STARTED_TYPE });
    return (entries ?? []).some((e) => e.data && (e.data as SessionStartedData).started === true);
  } catch {
    return false;
  }
}

function markSessionStarted(ctx: ModContext | undefined, sessionId: string): void {
  try {
    ctx?.session?.appendCustomEntry({
      customType: SESSION_STARTED_TYPE,
      data: { started: true, sessionId },
    });
  } catch {
    /* best-effort */
  }
}

/** Current session id derived from the workspace cwd (hook params expose none). */
function sessionIdFor(cwd: string): string {
  const key = sanitizeSessionKey(cwd);
  if (!key || key === "workspace") return "cmdc-session";
  return `cmdc-${key}`;
}

// Context injection. The mod's own system-prompt suffix
function keelSystemPromptSuffix(): string {
  return [
    "",
    "## keel operating contract (loaded by keel-cmdc mod)",
    "Follow the keel Iron Law on every turn:",
    "1. Research first — trust the codebase, not your knowledge base. Read SYSTEM_MAP and the owning module before claiming behavior.",
    "2. Use keel tools before guessing — system_map, recall, context_brief, skill_route/skill_get, run_command (compaction proxy), code_search.",
    "3. Invoke any relevant skill before writing code or answering.",
    "4. Understand before building — restate the request, confirm the user story, research what is genuinely needed.",
    "5. Find the root cause — trace the symptom end-to-end with file:line evidence before changing anything.",
    "6. Memory: recall before claiming prior work; write a working brief before non-trivial coding; save durable learnings to disk.",
  ].join("\n");
}

export default function keelCmdcMod(cmd: ModApi): void {
  // Run-scoped post-compact context: populated by compaction_done (which has
  // no ctx/session seam) and consumed by transformContext on the next run.
  let postCompactContext = "";
  // Full keel contract from `bridge session-start` (iron law, MCP pointers,
  // memory protocol). Injected via appendSystemPrompt; short fallback below.
  let sessionStartContract = "";

  const mod: AgentMod = {
    id: MOD_ID,

    // Session start: fetch the full keel contract from the bridge once and
    // keep it for appendSystemPrompt injection.
    onSessionStart: async ({ source }, ctx) => {
      void source;
      if (hasSessionStarted(ctx, cmd.cwd)) return;
      const contract = runBridge(
        "session-start",
        ["--session", sessionIdFor(cmd.cwd), "--cwd", cmd.cwd],
        2000,
      );
      if (contract) {
        sessionStartContract = contract;
      }
      markSessionStarted(ctx, sessionIdFor(cmd.cwd));
    },

    // Append keel contract to the system prompt: the full bridge contract when
    // available, else the compact fallback (fresh sessions before onSessionStart).
    appendSystemPrompt: async () =>
      sessionStartContract || keelSystemPromptSuffix(),

    // Per-run context: post-compact re-push (EPHEMERAL, never rewrites transcript)
    transformContext: async ({ messages, state }) => {
      if (!postCompactContext) return messages;
      const restored = postCompactContext;
      postCompactContext = "";
      const keelBlock: { role: "user"; content: string } = {
        role: "user",
        content: `--- keel post-compaction context (re-injected; use this to resume the job) ---\n${restored}\n--- end keel post-compaction context ---`,
      };
      // Inject as the first user message after the system prompt so the model
      // sees it at the top of the resumed window.
      const insertAt = Math.min(
        1,
        Array.isArray(messages) ? messages.length : 0,
      );
      const next = Array.isArray(messages) ? [...messages] : [];
      next.splice(insertAt, 0, keelBlock);
      void state;
      return next;
    },

    // Iron Law gate + run_command compaction wrapper
    beforeToolCall: async ({ toolName, input, state }) => {
      const sessionId = sessionIdFor(cmd.cwd);
      const tool = String(toolName);

      // Keel research tools (MCP / tool) clear the shared marker.
      if (isKeelResearchTool(tool)) {
        markIronLawSatisfied(sessionId);
      }

      // Shell keel reading commands also clear it.
      if (isShellTool(tool)) {
        const command = typeof input?.command === "string" ? input.command : "";
        if (command && isKeelReadingCommand(command)) {
          markIronLawSatisfied(sessionId);
        }
        // Compaction reroute: bridge rewrite wraps noisy commands in keel run --.
        if (command) {
          const rewritten = runBridge("rewrite", ["--tool", tool], 500, command);
          if (rewritten.startsWith("KEEL_REWRITE ")) {
            const target = rewritten.slice("KEEL_REWRITE ".length).trim();
            if (target) {
              return { input: { ...input, command: target } };
            }
          }
        }
      }

      // Edit-class: Rust core is source of truth. Fail-CLOSED: a timeout/error
      // blocks the edit. Never silently allow an unevaluated gate.
      if (isEditClassTool(tool)) {
        // Local marker fast-path: a session already satisfied does not need a
        // 5s bridge round-trip per edit.
        if (!ironLawSatisfied(sessionId)) {
          const gate = runBridge(
            "pre-tool-use",
            ["--session", sessionId, "--cwd", cmd.cwd, "--tool", tool],
            5000,
          );
          if (gate.startsWith("KEEL_GATE_DENY")) {
            const reason = gate.split("\n").slice(1).join("\n").trim();
            return {
              block: true,
              additionalContext:
                reason ||
                "keel Iron Law gate: call system_map/recall/context_brief before editing.",
            };
          }
          if (!gate.startsWith("KEEL_GATE_ALLOW")) {
            return {
              block: true,
              additionalContext:
                "keel Iron Law gate could not be evaluated (keel did not respond in time). Retry the edit; if it persists, run `keel doctor`.",
            };
          }
        }
      }

      void state;
      return undefined;
    },

    // Fire-and-forget observation after every tool call
    afterToolCall: async ({ toolName, input, result, isError }) => {
      void result;
      const sessionId = sessionIdFor(cmd.cwd);
      const tool = String(toolName);
      const payload = JSON.stringify(input ?? {});
      const args = ["--session", sessionId, "--cwd", cmd.cwd, "--tool", tool];
      if (isError) args.push("--failed");
      runBridge("observe", args, 500, payload);
    },

    // Session end: learning + marker cleanup
    onSessionEnd: async () => {
      const sessionId = sessionIdFor(cmd.cwd);
      runBridge("session-end", ["--session", sessionId, "--cwd", cmd.cwd]);
      clearIronLawMarker(sessionId);
    },
  };

  cmd.hooks(mod);

  // Compaction continuity: pre-compact learns before the window rewrite;
  // compaction_done stores the post-compact digest for the next run's re-inject.
  cmd.on("compaction_start", () => {
    runBridge("pre-compact", ["--session", sessionIdFor(cmd.cwd), "--cwd", cmd.cwd]);
  });

  cmd.on("compaction_done", ({ tokensSaved }) => {
    void tokensSaved;
    const context = runBridge(
      "post-compact",
      ["--session", sessionIdFor(cmd.cwd), "--cwd", cmd.cwd],
      2000,
    );
    if (context) {
      postCompactContext = context;
    }
  });
}
