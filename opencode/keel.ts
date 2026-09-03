import type { Plugin } from "@opencode-ai/plugin";
import {
  clearIronLawMarker as clearIronLaw,
  clearSessionStarted,
  hasSessionStarted,
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

  // ---- bridge runner (closes over $), never throws, bounded lifecycle timeout ----

  async function runBridge(
    subcommand: string,
    args: string[],
    timeoutMs = 5000,
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
    timeoutMs: number = 2000,
  ): Promise<string> {
    try {
      // Use Bun.spawn to pass stdin directly with timeout signal.
      const signal = AbortSignal.timeout(timeoutMs);
      const proc = Bun.spawn([BRIDGE_BIN, "bridge", subcommand, ...args], {
        stdin: "pipe",
        stdout: "pipe",
        stderr: "pipe",
        signal,
      });
      await proc.stdin.write(stdin);
      proc.stdin.end();
      const result = await new Response(proc.stdout).text();
      await proc.exited;
      return result?.trim() ?? "";
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
        const readingCommand = isKeelReadingCommand(command);
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
              (readingCommand
                ? "keel reading command gate denied this command."
                : "keel Iron Law gate: call system_map/recall/context_brief before running shell commands."),
          );
        }
        if (gateResult.status !== "allow") {
          if (command && (isKeelReadingCommand(command) || command.trim().startsWith("keel doctor"))) {
            // Allow recovery command through
          } else {
            throw new Error(
              "keel Iron Law gate could not be evaluated (keel did not respond in time). Retry; if it persists, run `keel doctor`.",
            );
          }
        }

        if (!isAlreadyCompacted(command)) {
          const rewritten = parseRewriteResponse(
            await runBridgeWithStdin("rewrite", ["--tool", toolName], command),
          );
          if (rewritten) {
            output.args.command = rewritten;
          }
        }
      }
    },
    "tool.execute.after": async (input, output) => {
      try {
        const metadata = output.metadata;
        const failed =
          metadata != null &&
          typeof metadata === "object" &&
          "error" in metadata &&
          metadata.error != null;
        let cmd: string | undefined;
        let toolArgs: unknown;
        if (input && typeof input === "object" && "args" in input) {
          toolArgs = input.args;
          if (toolArgs && typeof toolArgs === "object" && "command" in toolArgs) {
            const candidate = toolArgs.command;
            if (typeof candidate === "string") cmd = candidate;
          }
        }
        const stdin = JSON.stringify({
          tool_input: toolArgs,
          command: cmd,
          output: output.output,
          metadata: output.metadata,
        });
        const args = [
          "--session",
          input.sessionID || "default",
          "--cwd",
          cwd,
          "--tool",
          input.tool,
        ];
        if (failed) args.push("--failed");
        await runBridgeWithStdin("observe", args, stdin);
      } catch (e) {
        console.error("[keel-plugin] tool.execute.after error:", e);
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
            case "session.deleted": {
              const properties = event.properties;
              const sid =
                properties != null &&
                typeof properties === "object" &&
                "sessionID" in properties &&
                typeof properties.sessionID === "string"
                  ? properties.sessionID
                  : "";
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
      })().catch((e) => {
        console.error("[keel-plugin] unhandled event error:", e);
      });
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
