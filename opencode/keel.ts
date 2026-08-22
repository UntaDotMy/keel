import type { Plugin } from "@opencode-ai/plugin";
import {
  clearIronLawMarker as clearIronLaw,
  clearSessionStarted,
  hasSessionStarted,
  isEditClassTool,
  isKeelReadingCommand,
  isKeelResearchTool,
  isShellTool,
  markIronLawSatisfied,
  markSessionStarted,
  parseGateResponse,
  parseRewriteResponse,
  resolveBinary,
  sessionMarkerDirectory,
} from "../_shared/ts/bridge-core";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface TextPart {
  type: "text";
  text: string;
}

/** Any part appearing in output.parts. */
type MessagePart = TextPart | { type: string; [key: string]: unknown };

// ---------------------------------------------------------------------------
const BRIDGE_BIN: string = resolveBinary();
const STARTED_DIR = sessionMarkerDirectory("opencode");

function hasStarted(sessionID: string): boolean {
  return hasSessionStarted(STARTED_DIR, sessionID);
}

function markStarted(sessionID: string): void {
  markSessionStarted(STARTED_DIR, sessionID);
}

function clearMarker(sessionID: string): void {
  clearSessionStarted(STARTED_DIR, sessionID);
}

// ---------------------------------------------------------------------------
// User-text extraction
// ---------------------------------------------------------------------------

function extractUserText(parts: MessagePart[]): string {
  for (const part of parts) {
    if (part.type === "text") return (part as TextPart).text;
  }
  return "";
}


// ---------------------------------------------------------------------------
// Plugin entry point
// ---------------------------------------------------------------------------

const KeelPlugin: Plugin = async ({ client, directory, $ }) => {
  console.error("[keel-plugin] initialized — bridge:", BRIDGE_BIN);

  const cwd = directory;

  // ---- bridge runner (closes over $), never throws, 500ms hard timeout ----

  async function runBridge(
    subcommand: string,
    args: string[],
    timeoutMs = 500,
  ): Promise<string> {
    try {
      // Bun's $ escapes each interpolation as a single argument; a string array
      // interpolation is argv-split into separate args. Interpolate the binary,
      // subcommand, and args array as distinct tokens so they are NOT collapsed
      // into one bogus program name.
      const result = await $`${BRIDGE_BIN} bridge ${subcommand} ${args}`
        .timeout(timeoutMs)
        .quiet()
        .text();
      return result ?? "";
    } catch {
      return "";
    }
  }

  async function runBridgeWithStdin(
    subcommand: string,
    args: string[],
    stdin: string,
  ): Promise<string> {
    try {
      // Redirect stdin from a Buffer (Bun's documented stdin source) rather than
      // `echo | cmd`, which mangles JSON across shells.
      const input = Buffer.from(stdin, "utf-8");
      const result = await $`${BRIDGE_BIN} bridge ${subcommand} ${args} < ${input}`
        .timeout(500)
        .quiet()
        .text();
      return result ?? "";
    } catch {
      return "";
    }
  }

  return {
    // -----------------------------------------------------------------------
    // "chat.message" — NAMED, awaited; PRIMARY context-injection seam.
    // Runs BEFORE the model sees the user message.
    // -----------------------------------------------------------------------
    "chat.message": async (input, output) => {
      try {
        const sessionID: string = input.sessionID;
        const extraParts: MessagePart[] = [];

        // 1) Session-start (once per session, guarded by on-disk marker)
        if (!hasStarted(sessionID)) {
          const startupText = await runBridge("session-start", [
            "--session",
            sessionID,
            "--cwd",
            cwd,
          ]);
          if (startupText) {
            extraParts.push({ type: "text", text: startupText });
          }
          markStarted(sessionID);
        }

        // 2) Per-prompt injection
        const userText = extractUserText(output.parts);
        if (userText) {
          const promptContext = await runBridge("user-prompt", [
            "--session",
            sessionID,
            "--cwd",
            cwd,
            "--prompt",
            userText,
          ]);
          if (promptContext) {
            extraParts.push({ type: "text", text: promptContext });
          }
        }

        // Prepend injected context at the front of parts
        if (extraParts.length > 0) {
          output.parts = [...extraParts, ...output.parts];
        }
      } catch (e) {
        console.error("[keel-plugin] chat.message error:", e);
        // Degrade gracefully — no context injected, turn continues.
      }
    },

    "tool.execute.before": async (input, output) => {
      const toolName: string =
        (input as { tool?: string })?.tool ?? "";
      const sessionID: string =
        (input as { sessionID?: string })?.sessionID ?? "";

      // STRICT: only keel research tools clear the shared session marker.
      // Plain Read/Grep is allowed but does NOT satisfy (matches Rust default).
      if (isKeelResearchTool(toolName)) {
        markIronLawSatisfied(sessionID);
        return;
      }
      if (isShellTool(toolName)) {
        const command: string =
          (output as { args?: { command?: string } })?.args?.command ?? "";
        if (command && isKeelReadingCommand(command)) {
          markIronLawSatisfied(sessionID);
          return;
        }
      }

      // Edit-class: Rust core is source of truth (evidence-based deny). This
      // gate is fail-CLOSED: an empty result means the bridge timed out or
      // errored, and an unevaluated Iron Law gate must BLOCK the edit — never
      // silently allow it. A 500ms budget is too tight for a cold keel.exe
      // (Windows Defender scan on first run), so the gate call gets a larger
      // budget than the advisory calls.
      if (isEditClassTool(toolName)) {
        const result = await runBridge(
          "pre-tool-use",
          ["--session", sessionID, "--cwd", cwd, "--tool", toolName],
          5000,
        );
        const gateResult = parseGateResponse(result);
        if (gateResult.status === "deny") {
          throw new Error(
            gateResult.reason ||
              "keel Iron Law gate: call system_map/recall/context_brief before editing.",
          );
        }
        if (gateResult.status !== "allow") {
          // Timeout/error/unexpected output — fail closed.
          throw new Error(
            "keel Iron Law gate could not be evaluated (keel did not respond in time). Retry the edit; if it persists, run `keel doctor`.",
          );
        }
        return;
      }

      // Compaction reroute for shell tools (existing behavior).
      if (isShellTool(toolName)) {
        const command: string =
          (output as { args?: { command?: string } })?.args?.command ?? "";
        if (!command) return;

        // why: the local marker check here never consulted the Rust core, so
        // KEEL_IRON_LAW_GATE=off/balanced was ignored on this host.
        const gate = await runBridge(
          "pre-tool-use",
          ["--session", sessionID, "--cwd", cwd, "--tool", toolName, "--command", command],
          5000,
        );
        const gateResult = parseGateResponse(gate);
        if (gateResult.status === "deny") {
          throw new Error(
            gateResult.reason ||
              "keel Iron Law gate: call system_map/recall/context_brief before running shell commands.",
          );
        }
        if (gateResult.status !== "allow") {
          throw new Error(
            "keel Iron Law gate could not be evaluated (keel did not respond in time). Retry; if it persists, run `keel doctor`.",
          );
        }

        const rewritten = parseRewriteResponse(
          await runBridgeWithStdin("rewrite", ["--tool", toolName], command),
        );
        if (rewritten) {
          (output as { args: { command: string } }).args.command = rewritten;
        }
      }
    },

    // -----------------------------------------------------------------------
    // "event" — FIRE-AND-FORGET, not awaited, CANNOT block.
    // Used for observation recording, learning checkpoints, and session-end.
    // -----------------------------------------------------------------------
    event: async ({ event }) => {
      (async () => {
        try {
          switch (event.type) {
            // ---- Tool observation (non-blocking side-effect) ----
            case "tool.execute.after": {
              // why: session id came from `properties` while tool/input came from
              // the event root, so one was always undefined. Read both shapes.
              type ToolEventFields = {
                tool?: string;
                input?: unknown;
                failed?: boolean;
                sessionID?: string;
              };
              const root = event as unknown as ToolEventFields;
              const props = (event.properties ?? {}) as ToolEventFields;
              const toolName = props.tool ?? root.tool ?? "";
              const payload = props.input ?? root.input;
              const failed = props.failed ?? root.failed;
              const stdin = payload != null ? JSON.stringify(payload) : "{}";
              const obsArgs = [
                "--session",
                props.sessionID ?? root.sessionID ?? "",
                "--cwd",
                cwd,
                "--tool",
                toolName,
              ];
              if (failed) obsArgs.push("--failed");
              await runBridgeWithStdin("observe", obsArgs, stdin);
              break;
            }

            // ---- Session deleted: run session-end (learning + save) ----
            case "session.deleted": {
              const sid =
                (event.properties as { sessionID?: string })?.sessionID ??
                "";
              await runBridge("session-end", [
                "--session",
                sid,
                "--cwd",
                cwd,
              ]);
              clearMarker(sid);
              clearIronLaw(sid);
              break;
            }
          }
        } catch (e) {
          console.error("[keel-plugin] event handler error:", e);
          // Never throw from event — fire-and-forget.
        }
      })();
    },

    // "experimental.session.compacting" (NAMED, awaited): pre-compact learning
    // checkpoint, then post-compact context injection into the summary.
    "experimental.session.compacting": async (input, output) => {
      try {
        const sessionID: string = input.sessionID;
        await runBridge("pre-compact", [
          "--session",
          sessionID,
          "--cwd",
          cwd,
        ]);
        const contextText = await runBridge("post-compact", [
          "--session",
          sessionID,
          "--cwd",
          cwd,
        ]);
        if (contextText) {
          output.context.push(contextText);
        }
      } catch (e) {
        console.error(
          "[keel-plugin] experimental.session.compacting error:",
          e,
        );
        // Degrade gracefully.
      }
    },
  };
};

export default KeelPlugin;