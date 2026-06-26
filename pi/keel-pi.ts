#!/usr/bin/env node

// ---------------------------------------------------------------------------
// keel Pi Agent Extension — compaction reroute for shell commands.
//
// Registers a tool_call handler that intercepts bash commands, calls
// `keel bridge rewrite`, and mutates event.input.command in place to
// reroute noisy commands through `keel run --` for output compaction.
//
// Pi's tool_call contract (from official docs + rtk-ai/rtk reference):
//   - Mutate event.input in place to patch tool arguments before execution.
//   - Return value only controls blocking: { block: true, reason?: string }.
//   - Returning undefined (or nothing) allows execution with the mutation.
//
// Install: copy this file to ~/.pi/extensions/keel-pi.ts (or project-scoped
// .pi/extensions/). Pi Agent loads extensions at startup.
// ---------------------------------------------------------------------------

import * as os from "node:os";
import * as fs from "node:fs";
import * as path from "node:path";
import { execFileSync } from "node:child_process";

// ---------------------------------------------------------------------------
// Binary resolution — resolved once at script init
// ---------------------------------------------------------------------------

const BIN_NAME: string =
  os.platform() === "win32" ? "keel.exe" : "keel";

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
// Bridge rewrite — call keel bridge rewrite with command on stdin
// ---------------------------------------------------------------------------

function bridgeRewrite(command: string): string {
  try {
    const result = execFileSync(
      BRIDGE_BIN,
      ["bridge", "rewrite"],
      {
        timeout: 500,
        input: command,
        stdio: ["pipe", "pipe", "pipe"],
        encoding: "utf-8",
        windowsHide: true,
      },
    );
    return result ?? "";
  } catch {
    return "";
  }
}

// ---------------------------------------------------------------------------
// Extension entry point
// ---------------------------------------------------------------------------

// Pi's ExtensionAPI type is available from @earendil-works/pi-coding-agent,
// but we use a minimal interface to avoid a hard dependency.
interface PiExtensionAPI {
  on(event: string, handler: (event: any, ctx: any) => void | Promise<void>): void;
}

interface ToolCallEvent {
  tool?: string;
  input?: {
    command?: string;
    [key: string]: unknown;
  };
}

const SHELL_TOOLS = new Set(["bash", "shell", "sh", "zsh"]);

export default function (pi: PiExtensionAPI): void {
  pi.on("tool_call", async (event: ToolCallEvent, _ctx: unknown) => {
    try {
      // Only intercept shell/bash tool calls.
      const toolName = event?.tool ?? "";
      if (!SHELL_TOOLS.has(toolName.toLowerCase())) return;

      const cmd = event?.input?.command;
      if (typeof cmd !== "string" || cmd.trim() === "") return;

      // Already wrapped — skip.
      if (cmd.startsWith("keel run --")) return;

      // Ask keel to rewrite the command.
      const rewrite = bridgeRewrite(cmd);

      // keel bridge rewrite outputs "KEEL_REWRITE <cmd>" for noisy commands.
      if (!rewrite.startsWith("KEEL_REWRITE ")) return;

      const rewritten = rewrite.slice("KEEL_REWRITE ".length).trim();
      if (rewritten && rewritten !== cmd) {
        // Pi's contract: mutate event.input in place.
        event.input!.command = rewritten;
      }

      // Returning undefined allows execution with the mutated command.
      // Return { block: true, reason: "..." } only to block.
    } catch (err) {
      // Fail open — let the original command run unchanged.
      console.warn(
        "[keel] unexpected error in tool_call handler; passing through",
        err,
      );
    }
  });
}
