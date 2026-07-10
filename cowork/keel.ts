/**
 * keel Cowork Plugin for Claude Desktop
 *
 * Bridges Claude Desktop lifecycle events to the `keel` Rust CLI for:
 * - Context injection (iron law, skill catalog, working brief reminders)
 * - Observation recording (tool timing, behavioral patterns)
 * - Learning checkpoints (session-end synthesis)
 * - Command compaction (pre-tool-use rewrite)
 * - Review gates (post-tool-batch enforcement)
 *
 * This plugin enables the full keel experience in Claude Desktop, including
 * the iron law enforcement, command compaction, review gates, and memory management.
 */

import * as os from "node:os";
import * as fs from "node:fs";
import * as path from "node:path";
import { spawn } from "node:child_process";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/**
 * Plugin interface for Claude Desktop Cowork.
 * Matches the Claude Desktop plugin API.
 */
interface CoworkPlugin {
  name: string;
  version: string;
  onInit?: () => Promise<void>;
  onSessionStart?: (session: { id: string }) => Promise<void>;
  onUserPromptSubmit?: (input: {
    sessionID: string;
    message: { parts: MessagePart[] };
  }) => Promise<{ additionalContext?: string }>;
  onPreToolUse?: (input: {
    tool: string;
    toolInput: Record<string, unknown>;
  }) => Promise<{ rerunCommand?: string; allow?: boolean }>;
  onPostToolUse?: (input: {
    tool: string;
    toolInput: Record<string, unknown>;
    toolOutput?: unknown;
    durationMs?: number;
    failed?: boolean;
  }) => Promise<void>;
  onPostToolBatch?: (input: {
    sessionID: string;
    toolsUsed: Array<{ tool: string; durationMs?: number }>;
  }) => Promise<{ additionalContext?: string }>;
  onSessionEnd?: (session: { id: string }) => Promise<void>;
  onSessionCompacting?: (input: {
    sessionID: string;
  }) => Promise<{ additionalContext?: string }>;
  commands?: Record<string, {
    description?: string;
    execute: (input: { args: string }) => Promise<{ content: string }>;
  }>;
}

interface TextPart {
  type: "text";
  text: string;
}

type MessagePart = TextPart | { type: string; [key: string]: unknown };

// ---------------------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------------------

const BIN_NAME = os.platform() === "win32" ? "keel.exe" : "keel";

/**
 * Resolve the keel binary path.
 * Prefer ~/.claude/<binary>; fall back to bare name (PATH lookup).
 */
function resolveBinary(): string {
  const home = os.homedir();
  const explicit = path.join(home, ".claude", BIN_NAME);
  try {
    if (fs.existsSync(explicit)) return explicit;
  } catch {
    // fs failure — PATH only
  }
  return BIN_NAME;
}

const BRIDGE_BIN = resolveBinary();

// ---------------------------------------------------------------------------
// Bridge call helper
// ---------------------------------------------------------------------------

/**
 * Run a keel bridge subcommand and return the result.
 * Times out after 500ms to prevent blocking the session.
 */
async function runBridge(
  subcommand: string,
  args: string[] = [],
  timeoutMs: number = 500
): Promise<string> {
  return new Promise((resolve) => {
    const fullArgs = ["bridge", subcommand, ...args];

    const proc = spawn(BRIDGE_BIN, fullArgs, {
      cwd: process.cwd(),
      env: { ...process.env },
      stdio: ["ignore", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    let settled = false;

    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        proc.kill();
        resolve(""); // Timeout — degrade gracefully
      }
    }, timeoutMs);

    proc.stdout?.on("data", (data: Buffer) => {
      stdout += data.toString();
    });

    proc.stderr?.on("data", (data: Buffer) => {
      stderr += data.toString();
    });

    proc.on("close", (code) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        if (code === 0) {
          resolve(stdout.trim());
        } else {
          console.error(`[keel-cowork] bridge ${subcommand} failed: ${stderr}`);
          resolve(""); // Degrade gracefully
        }
      }
    });

    proc.on("error", (err) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        console.error(`[keel-cowork] bridge ${subcommand} error: ${err.message}`);
        resolve("");
      }
    });
  });
}

/**
 * Run a top-level `keel` subcommand (not a bridge lifecycle event).
 * Used by slash commands — recall, sprint, review, --version — which are
 * real CLI surfaces, not part of the `keel bridge` event dispatch.
 */
async function runCli(
  args: string[] = [],
  timeoutMs: number = 5000,
): Promise<string> {
  return new Promise((resolve) => {
    const proc = spawn(BRIDGE_BIN, args, {
      cwd: process.cwd(),
      env: { ...process.env },
      stdio: ["ignore", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    let settled = false;

    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        proc.kill();
        resolve("");
      }
    }, timeoutMs);

    proc.stdout?.on("data", (data: Buffer) => {
      stdout += data.toString();
    });
    proc.stderr?.on("data", (data: Buffer) => {
      stderr += data.toString();
    });

    proc.on("close", (code) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        if (code === 0) {
          resolve(stdout.trim());
        } else {
          console.error(`[keel-cowork] keel ${args.join(" ")} failed: ${stderr}`);
          resolve("");
        }
      }
    });

    proc.on("error", (err) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        console.error(`[keel-cowork] keel ${args.join(" ")} error: ${err.message}`);
        resolve("");
      }
    });
  });
}

// ---------------------------------------------------------------------------
// Marker helpers (session-start deduplication)
// ---------------------------------------------------------------------------

const MARKER_DIR = path.join(os.homedir(), ".claude", "state", "cowork-session-started");

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
// User text extraction
// ---------------------------------------------------------------------------

function extractUserText(parts: MessagePart[]): string {
  for (const part of parts) {
    if (part && typeof part === "object" && "text" in part && part.type === "text") {
      return (part as TextPart).text;
    }
  }
  return "";
}

// ---------------------------------------------------------------------------
// Plugin definition
// ---------------------------------------------------------------------------

const KeelCoworkPlugin: CoworkPlugin = {
  name: "keel-cowork",
  version: "0.1.0",

  /**
   * Initialize the plugin.
   * Called once when Claude Desktop starts.
   */
  async onInit(): Promise<void> {
    console.log("[keel-cowork] Initializing keel plugin for Claude Desktop");

    // Verify the keel binary is accessible
    try {
      const version = await runCli(["version"], 1000);
      if (version) {
        console.log(`[keel-cowork] Connected to keel ${version.split("\n")[0].trim()}`);
      }
    } catch (e) {
      console.warn("[keel-cowork] Could not verify keel binary:", e);
    }
  },

  /**
   * Handle session start.
   * Inject bootstrap context + workspace digest on first message.
   */
  async onSessionStart(session: { id: string }): Promise<void> {
    const sessionID = session.id;
    if (hasStarted(sessionID)) return;

    try {
      const context = await runBridge("session-start", [
        "--session", sessionID,
        "--cwd", process.cwd(),
      ]);

      if (context) {
        console.log(`[keel-cowork] Injected session-start context (${context.length} chars)`);
      }

      markStarted(sessionID);
    } catch (e) {
      console.error("[keel-cowork] session-start error:", e);
    }
  },

  /**
   * Handle user prompt submission.
   * Inject iron law + skill brief before model sees the message.
   */
  async onUserPromptSubmit(input: {
    sessionID: string;
    message: { parts: MessagePart[] };
  }): Promise<{ additionalContext?: string }> {
    const sessionID = input.sessionID;
    const promptText = extractUserText(input.message.parts);

    try {
      const context = await runBridge("user-prompt", [
        "--session", sessionID,
        "--cwd", process.cwd(),
        "--prompt", promptText,
      ]);

      if (context) {
        return { additionalContext: context };
      }
    } catch (e) {
      console.error("[keel-cowork] user-prompt error:", e);
    }

    return {};
  },

  /**
   * Handle pre-tool-use.
   * Return rewritten command for compaction (e.g., cargo test → keel run -- cargo test).
   */
  async onPreToolUse(input: {
    tool: string;
    toolInput: Record<string, unknown>;
  }): Promise<{ rerunCommand?: string; allow?: boolean }> {
    try {
      const toolName = input.tool;
      const toolArgs = JSON.stringify(input.toolInput);

      const result = await runBridge("pre-tool-use", [
        "--session", "desktop",
        "--cwd", process.cwd(),
        "--tool", toolName,
      ], 200);

      if (result) {
        // Check if the bridge returned a rerun command
        if (result.startsWith("Rerun that as:")) {
          const command = result.replace("Rerun that as:", "").trim();
          return { rerunCommand: command };
        }
        // Check for allow/deny
        if (result.includes("ALLOW")) {
          return { allow: true };
        }
        if (result.includes("DENY")) {
          return { allow: false };
        }
      }
    } catch (e) {
      console.error("[keel-cowork] pre-tool-use error:", e);
    }

    return {};
  },

  /**
   * Handle post-tool-use.
   * Record observation for learning loop.
   */
  async onPostToolUse(input: {
    tool: string;
    toolInput: Record<string, unknown>;
    toolOutput?: unknown;
    durationMs?: number;
    failed?: boolean;
  }): Promise<void> {
    const sessionID = "desktop"; // Cowork may not have session ID in this context

    try {
      await runBridge("observe", [
        "--session", sessionID,
        "--cwd", process.cwd(),
        "--tool", input.tool,
        ...(input.failed ? ["--failed"] : []),
      ], 100);
    } catch (e) {
      // Fire-and-forget, no logging needed
    }
  },

  /**
   * Handle post-tool-batch.
   * Fire review/working-brief gates.
   */
  async onPostToolBatch(input: {
    sessionID: string;
    toolsUsed: Array<{ tool: string; durationMs?: number }>;
  }): Promise<{ additionalContext?: string }> {
    const sessionID = input.sessionID;

    try {
      const gates = await runBridge("gate-status", [
        "--session", sessionID,
        "--cwd", process.cwd(),
      ]);

      if (gates) {
        return { additionalContext: gates };
      }
    } catch (e) {
      console.error("[keel-cowork] gate-status error:", e);
    }

    return {};
  },

  /**
   * Handle session end.
   * Trigger learning checkpoint.
   */
  async onSessionEnd(session: { id: string }): Promise<void> {
    const sessionID = session.id;

    try {
      await runBridge("session-end", [
        "--session", sessionID,
        "--cwd", process.cwd(),
      ], 1000);

      clearMarker(sessionID);
    } catch (e) {
      console.error("[keel-cowork] session-end error:", e);
    }
  },

  /**
   * Handle session compaction.
   * Inject post-compact context into the compaction summary.
   */
  async onSessionCompacting(input: {
    sessionID: string;
  }): Promise<{ additionalContext?: string }> {
    const sessionID = input.sessionID;

    try {
      const context = await runBridge("post-compact", [
        "--session", sessionID,
        "--cwd", process.cwd(),
      ]);

      if (context) {
        return { additionalContext: context };
      }
    } catch (e) {
      console.error("[keel-cowork] post-compact error:", e);
    }

    return {};
  },

  /**
   * Custom slash commands provided by keel.
   */
  commands: {
    "keel": {
      description: "Run keel CLI commands (workflow, memory, sprint, review, etc.)",
      async execute(input: { args: string }): Promise<{ content: string }> {
        try {
          const result = await runCli(input.args.split(" "), 5000);
          return { content: result || "Command completed" };
        } catch (e) {
          return { content: `Error: ${e}` };
        }
      },
    },
    "keel:recall": {
      description: "Search memory and working briefs using full-text search",
      async execute(input: { args: string }): Promise<{ content: string }> {
        try {
          const result = await runCli(["memory", "recall", ...input.args.split(" ")], 3000);
          return { content: result };
        } catch (e) {
          return { content: `Error: ${e}` };
        }
      },
    },
    "keel:sprint": {
      description: "Manage sprint backlog and track story progress",
      async execute(input: { args: string }): Promise<{ content: string }> {
        try {
          const result = await runCli(["sprint", ...input.args.split(" ")], 3000);
          return { content: result };
        } catch (e) {
          return { content: `Error: ${e}` };
        }
      },
    },
    "keel:review": {
      description: "Run pre-commit or pre-PR review",
      async execute(input: { args: string }): Promise<{ content: string }> {
        try {
          const result = await runCli(["review", ...input.args.split(" ")], 5000);
          return { content: result };
        } catch (e) {
          return { content: `Error: ${e}` };
        }
      },
    },
  },
};

export default KeelCoworkPlugin;
