#!/usr/bin/env node

// keel:managed-host-file (remove this line before customizing to opt out of upgrades)
// Keel Antigravity hook adapter translates payloads to bridge commands and gate results.

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

function resolveBinary() {
  const executable = process.platform === "win32" ? "keel.exe" : "keel";
  const candidates = [
    process.env.KEEL_BIN,
    process.env.KEEL_HOME && join(process.env.KEEL_HOME, executable),
    join(homedir(), ".keel", executable),
  ].filter(Boolean);
  return candidates.find((candidate) => existsSync(candidate)) ?? "keel";
}

function runKeel(args, stdin = undefined, timeout = 10_000) {
  try {
    return execFileSync(resolveBinary(), args, {
      encoding: "utf8",
      input: stdin,
      timeout,
      windowsHide: true,
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
  } catch {
    return "";
  }
}

function runBridge(subcommand, args, stdin = undefined, timeout = 10_000) {
  return runKeel(["bridge", subcommand, ...args], stdin, timeout);
}

function parseGate(output) {
  if (!output) return { status: "error" };
  if (output.startsWith("KEEL_GATE_DENY")) {
    return {
      status: "deny",
      reason: output.split("\n").slice(1).join("\n").trim(),
    };
  }
  if (output.startsWith("KEEL_GATE_ALLOW")) {
    return { status: "allow", reason: "" };
  }
  return { status: "error" };
}

function sessionId(input) {
  return String(input.conversationId ?? "antigravity");
}

function cwd(input) {
  const args = input.toolCall?.args;
  if (args && typeof args === "object") {
    for (const key of ["Cwd", "cwd", "workingDirectory"]) {
      if (typeof args[key] === "string" && args[key]) return args[key];
    }
  }
  const paths = Array.isArray(input.workspacePaths) ? input.workspacePaths : [];
  return typeof paths[0] === "string" ? paths[0] : process.cwd();
}

function toolName(input) {
  const name = String(input.toolCall?.name ?? "");
  if (name === "call_mcp_tool") {
    const args = toolArgs(input);
    if (
      args &&
      typeof args === "object" &&
      args.ServerName === "keel" &&
      typeof args.ToolName === "string"
    ) {
      return `mcp__keel__${args.ToolName}`;
    }
  }
  return name;
}

function toolArgs(input) {
  const args = input.toolCall?.args;
  return args && typeof args === "object" ? args : {};
}

function commandFrom(args) {
  for (const key of ["CommandLine", "command", "cmd", "script"]) {
    if (typeof args[key] === "string") return args[key];
  }
  return "";
}

function handlePreToolUse(input) {
  const args = toolArgs(input);
  const bridgeArgs = [
    "--session",
    sessionId(input),
    "--cwd",
    cwd(input),
    "--tool",
    toolName(input),
  ];
  const command = commandFrom(args);
  if (command) bridgeArgs.push("--command", command);
  const gate = parseGate(
    runBridge("pre-tool-use", bridgeArgs, JSON.stringify(args)),
  );
  if (gate.status === "deny") {
    return {
      decision: "deny",
      reason: gate.reason || "Keel denied this operation until its research gate is satisfied.",
    };
  }
  if (gate.status !== "allow") {
    return {
      decision: "deny",
      reason: "Keel could not evaluate the operation. Run the Keel doctor and retry.",
    };
  }
  return { decision: "allow" };
}

function handlePostToolUse(input) {
  const args = [
    "--session",
    sessionId(input),
    "--cwd",
    cwd(input),
    "--tool",
    toolName(input),
    "--phase",
    "post",
  ];
  if (input.error) args.push("--failed");
  runBridge("observe", args, JSON.stringify(toolArgs(input)), 2_000);
  return {};
}

function handlePreInvocation(input) {
  if (Number(input.invocationNum ?? 0) !== 0) return { injectSteps: [] };
  const context = runBridge("session-start", [
    "--session",
    sessionId(input),
    "--cwd",
    cwd(input),
  ]);
  return context
    ? { injectSteps: [{ ephemeralMessage: context }] }
    : { injectSteps: [] };
}

function handleStop(input) {
  if (
    (typeof input.terminationReason === "string" &&
      input.terminationReason !== "model_stop") ||
    input.fullyIdle === false
  ) {
    runBridge("session-end", [
      "--session",
      sessionId(input),
      "--cwd",
      cwd(input),
    ]);
    return { decision: "allow" };
  }
  const executionNum = Number(input.executionNum ?? 1);
  const stopHookActive =
    input.stopHookActive === true ||
    input.stop_hook_active === true ||
    executionNum > 1;
  const stopOutput = runKeel(
    ["hook", "stop"],
    JSON.stringify({
      ...input,
      session_id: sessionId(input),
      cwd: cwd(input),
      stop_hook_active: stopHookActive,
    }),
  );
  if (stopOutput) {
    try {
      const stop = JSON.parse(stopOutput);
      if (stop.decision === "block") {
        return {
          decision: "continue",
          reason: stop.reason || "Keel closeout checks are incomplete.",
        };
      }
    } catch {
      return {
        decision: "continue",
        reason: "Keel could not evaluate closeout. Run the Keel doctor and retry.",
      };
    }
  }
  runBridge("session-end", [
    "--session",
    sessionId(input),
    "--cwd",
    cwd(input),
  ]);
  return { decision: "allow" };
}

async function main() {
  let raw = "";
  process.stdin.setEncoding("utf8");
  for await (const chunk of process.stdin) raw += chunk;
  let input = {};
  try {
    input = JSON.parse(raw || "{}");
  } catch {
    process.stdout.write(JSON.stringify({}));
    return;
  }
  const event = process.argv[2] ?? "";
  const output =
    event === "pre-tool-use"
      ? handlePreToolUse(input)
      : event === "post-tool-use"
        ? handlePostToolUse(input)
        : event === "pre-invocation"
          ? handlePreInvocation(input)
          : event === "stop"
            ? handleStop(input)
            : {};
  process.stdout.write(JSON.stringify(output));
}

await main();
