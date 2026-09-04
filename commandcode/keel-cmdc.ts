// keel:managed-host-file (remove this line before customizing to opt out of upgrades)
// keel Command Code (cmdc) Mod. Bridges cmdc lifecycle events to the host-
// neutral `keel bridge` surface. Install per commandcode/README.md.

import { execFileSync } from "node:child_process";
import type { ModApi, AgentMod, ModContext } from "@commandcode/harness";
import {
  clearIronLawMarker,
  ironLawSatisfied,
  isAlreadyCompacted,
  isEditClassTool,
  isKeelReadingCommand,
  isShellTool,
  parseGateResponse,
  parseRewriteResponse,
  resolveBinary,
  sanitizeSessionKey,
} from "../_shared/ts/bridge-core";


const MOD_ID = "keel-cmdc";
const SESSION_STARTED_TYPE = "keel-cmdc/session-started";

const BRIDGE_BIN: string = resolveBinary();

// Bridge runner. Never throws; lifecycle reads have a bounded index and disk budget.
function runBridge(
  subcommand: string,
  args: string[],
  timeoutMs = 5000,
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
        5000,
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

      if (isShellTool(tool)) {
        const command = typeof input?.command === "string" ? input.command : "";
        const readingCommand = command ? isKeelReadingCommand(command) : false;
        const gateArgs = ["--session", sessionId, "--cwd", cmd.cwd, "--tool", tool];
        if (command) gateArgs.push("--command", command);
        const gateResult = parseGateResponse(runBridge("pre-tool-use", gateArgs, 5000));
        if (gateResult.status === "deny") {
          return {
            block: true,
            additionalContext:
              gateResult.reason ||
              (readingCommand
                ? "keel reading command gate denied this command."
                : "keel Iron Law shell gate denied this command."),
          };
        }
        if (gateResult.status !== "allow") {
          return {
            block: true,
            additionalContext:
              "keel Iron Law shell gate could not be evaluated. Retry after running `keel doctor`.",
          };
        }
        if (command && !isAlreadyCompacted(command)) {
          const rewritten = parseRewriteResponse(
            runBridge("rewrite", ["--tool", tool], 500, command),
          );
          if (rewritten) {
            return { input: { ...input, command: rewritten } };
          }
        }
      }

      // Edit-class: Rust core is source of truth. Fail-CLOSED on timeout/error.
      if (isEditClassTool(tool) && !ironLawSatisfied(sessionId)) {
        const gate = runBridge(
          "pre-tool-use",
          ["--session", sessionId, "--cwd", cmd.cwd, "--tool", tool],
          5000,
        );
        const gateResult = parseGateResponse(gate);
        if (gateResult.status === "deny") {
          return {
            block: true,
            additionalContext:
              gateResult.reason ||
              "keel Iron Law gate: call system_map/recall/context_brief before editing.",
          };
        }
        if (gateResult.status !== "allow") {
          return {
            block: true,
            additionalContext:
              "keel Iron Law gate could not be evaluated (keel did not respond in time). Retry the edit; if it persists, run `keel doctor`.",
          };
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
      const args = [
        "--session", sessionId, "--cwd", cmd.cwd, "--tool", tool,
        "--phase", "post",
      ];
      if (isError) args.push("--failed");
      runBridge("observe", args, 2000, payload);
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
