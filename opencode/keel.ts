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
  os.platform() === "win32" ? "keel.exe" : "keel";

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

const EDIT_CLASS_TOOLS = new Set([
  "edit", "write", "multiedit", "notebookedit", "apply_patch", "str_replace", "patch",
]);

function isEditClassTool(toolName: string): boolean {
  return EDIT_CLASS_TOOLS.has(toolName.toLowerCase());
}

const SHELL_TOOLS = new Set([
  "bash", "shell", "sh", "zsh", "fish", "powershell", "pwsh", "cmd",
]);

function isShellTool(toolName: string): boolean {
  return SHELL_TOOLS.has(toolName.toLowerCase());
}

// ---------------------------------------------------------------------------
// Iron Law enforcement — reading tools that satisfy "read first"
// ---------------------------------------------------------------------------

const READING_TOOLS = new Set([
  "read", "glob", "grep", "lsp_diagnostics", "lsp_goto_definition",
  "lsp_find_references", "lsp_symbols", "lsp_prepare_rename",
]);

function isReadingTool(toolName: string): boolean {
  return READING_TOOLS.has(toolName.toLowerCase());
}

const IRON_LAW_DIR = path.join(
  os.homedir(),
  ".claude",
  "state",
  "iron-law-satisfied",
);

function ensureIronLawDir(): void {
  try {
    fs.mkdirSync(IRON_LAW_DIR, { recursive: true });
  } catch {
    /* best-effort */
  }
}

function ironLawSatisfied(sessionID: string): boolean {
  ensureIronLawDir();
  try {
    return fs.existsSync(path.join(IRON_LAW_DIR, sessionID));
  } catch {
    return false;
  }
}

function markIronLawSatisfied(sessionID: string): void {
  ensureIronLawDir();
  try {
    fs.writeFileSync(path.join(IRON_LAW_DIR, sessionID), "", "utf-8");
  } catch {
    /* best-effort */
  }
}

function clearIronLaw(sessionID: string): void {
  try {
    fs.rmSync(path.join(IRON_LAW_DIR, sessionID), { force: true });
  } catch {
    /* best-effort */
  }
}

function isKeelReadingCommand(command: string): boolean {
  const trimmed = command.trim().toLowerCase();
  return trimmed.startsWith("keel ") && (
    trimmed.includes("system-map") ||
    trimmed.includes("recall") ||
    trimmed.includes("doctor") ||
    trimmed.includes("code-search") ||
    trimmed.includes("status") ||
    trimmed.includes("help")
  );
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
        console.error("[keel-plugin] chat.message error:", e);
        // Degrade gracefully — no context injected, turn continues.
      }
    },

    "tool.execute.before": async (input, output) => {
      const toolName: string =
        (input as { tool?: string })?.tool ?? "";
      const sessionID: string =
        (input as { sessionID?: string })?.sessionID ?? "";

      // Iron Law enforcement: reading tools satisfy the gate and are always allowed.
      if (isReadingTool(toolName)) {
        markIronLawSatisfied(sessionID);
        return;
      }

      // Shell tools running keel reading commands also satisfy the gate.
      if (isShellTool(toolName)) {
        const command: string =
          (output as { args?: { command?: string } })?.args?.command ?? "";
        if (command && isKeelReadingCommand(command)) {
          markIronLawSatisfied(sessionID);
          return;
        }
      }

      // Iron Law enforcement: non-reading tools are DENIED until the model has
      // demonstrated reading behavior in this session. This is the enforcement
      // mechanism — not just text injection, but an actual block.
      if (!ironLawSatisfied(sessionID)) {
        throw new Error(
          "IRON LAW ENFORCED: Read first. You must use Read, Glob, Grep, LSP, or keel tools (system-map, recall, doctor, code-search) BEFORE acting. The iron law is enforced — you cannot skip it.",
        );
      }

      // Iron Law gate for edit-class tools (existing behavior).
      if (isEditClassTool(toolName)) {
        const result = await runBridge("pre-tool-use", [
          "--session",
          sessionID,
          "--cwd",
          cwd,
          "--tool",
          toolName,
        ]);
        if (result.startsWith("KEEL_GATE_DENY")) {
          const reason = result
            .split("\n")
            .slice(1)
            .join("\n")
            .trim();
          throw new Error(
            reason ||
              "keel Iron Law gate: read context + load a skill before editing.",
          );
        }
        return;
      }

      // Compaction reroute for shell tools (existing behavior).
      if (isShellTool(toolName)) {
        const command: string =
          (output as { args?: { command?: string } })?.args?.command ?? "";
        if (!command) return;
        const rewrite = await runBridgeWithStdin("rewrite", ["--tool", toolName], command);
        if (rewrite.startsWith("KEEL_REWRITE ")) {
          const rewritten = rewrite.slice("KEEL_REWRITE ".length).trim();
          if (rewritten) {
            (output as { args: { command: string } }).args.command = rewritten;
          }
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
          "[keel-plugin] experimental.session.compacting error:",
          e,
        );
        // Degrade gracefully.
      }
    },
  };
};

export default KeelPlugin;