import type { Plugin } from "@opencode-ai/plugin";
import * as os from "node:os";
import * as fs from "node:fs";
import * as path from "node:path";

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
// Binary resolution — resolved once at plugin init
// ---------------------------------------------------------------------------

const BIN_NAME: string =
  os.platform() === "win32" ? "claude-skills.exe" : "claude-skills";

/**
 * Resolve the bridge binary path.
 * Prefer ~/.claude/<binary>; fall back to bare name (PATH lookup by Bun shell).
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
  "opencode-session-started",
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

const ClaudeCorePlugin: Plugin = async ({ client, directory, $ }) => {
  console.error("[claude-core-plugin] initialized — bridge:", BRIDGE_BIN);

  const cwd = directory;

  // ---- bridge runner (closes over $), never throws, 500ms hard timeout ----

  async function runBridge(
    subcommand: string,
    args: string[],
  ): Promise<string> {
    try {
      // Bun's $ escapes each interpolation as a single argument; a string array
      // interpolation is argv-split into separate args. Interpolate the binary,
      // subcommand, and args array as distinct tokens so they are NOT collapsed
      // into one bogus program name.
      const result = await $`${BRIDGE_BIN} bridge ${subcommand} ${args}`
        .timeout(500)
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
        console.error("[claude-core-plugin] chat.message error:", e);
        // Degrade gracefully — no context injected, turn continues.
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
              // The event shape per the task: { type, tool, input, failed? }
              const te = event as unknown as {
                tool?: string;
                input?: unknown;
                failed?: boolean;
              };
              const toolName = te.tool ?? "";
              const stdin =
                te.input != null ? JSON.stringify(te.input) : "{}";
              const obsArgs = [
                "--session",
                (event.properties as { sessionID?: string })?.sessionID ?? "",
                "--cwd",
                cwd,
                "--tool",
                toolName,
              ];
              if (te.failed) obsArgs.push("--failed");
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
              break;
            }
          }
        } catch (e) {
          console.error("[claude-core-plugin] event handler error:", e);
          // Never throw from event — fire-and-forget.
        }
      })();
    },

    // -----------------------------------------------------------------------
    // "experimental.session.compacting" — NAMED, awaited. Sole post-compact
    // caller: injects the post-compaction context into the compaction summary
    // AND triggers the learning checkpoint (bridge post-compact does both), so
    // the cycle runs exactly once per compaction.
    // -----------------------------------------------------------------------
    "experimental.session.compacting": async (input, output) => {
      try {
        const sessionID: string = input.sessionID;
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
          "[claude-core-plugin] experimental.session.compacting error:",
          e,
        );
        // Degrade gracefully.
      }
    },
  };
};

export default ClaudeCorePlugin;