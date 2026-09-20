#!/usr/bin/env node

// keel:managed-host-file (remove this line before customizing to opt out of upgrades)
// codex/keel-codex.ts
import * as fs2 from "node:fs";
import { execFileSync } from "node:child_process";

// _shared/ts/bridge-core.ts
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
var BIN_NAME = os.platform() === "win32" ? "keel.exe" : "keel";
function resolveBinary() {
  const home = os.homedir();
  const candidates = [];
  const envHome = process.env.KEEL_HOME;
  if (envHome && envHome.trim())
    candidates.push(path.join(envHome.trim(), BIN_NAME));
  candidates.push(path.join(home, ".keel", BIN_NAME));
  candidates.push(path.join(home, ".claude", BIN_NAME));
  for (const candidate of candidates) {
    try {
      const usable = os.platform() === "win32" ? fs.existsSync(candidate) : (() => {
        try {
          fs.accessSync(candidate, fs.constants.X_OK);
          return true;
        } catch {
          return false;
        }
      })();
      if (usable)
        return candidate;
    } catch {}
  }
  return BIN_NAME;
}
function keelStateRoot() {
  const home = os.homedir();
  const envHome = process.env.KEEL_HOME;
  if (envHome && envHome.trim())
    return path.join(envHome.trim(), "state");
  const neutralHome = path.join(home, ".keel");
  try {
    if (fs.existsSync(neutralHome))
      return path.join(neutralHome, "state");
  } catch {}
  return path.join(home, ".claude", "state");
}
// Canonical Gate Control Environment Variables
var IRON_LAW_GATE_ENV_VAR = "KEEL_IRON_LAW_GATE";
var REVIEW_GATE_ENV_VAR = "CLAUDE_SKILLS_REVIEW_GATE";
var REVIEW_GATE_MAX_BLOCKS_ENV_VAR = "CLAUDE_SKILLS_REVIEW_GATE_MAX_BLOCKS";
var BRIEF_GATE_ENV_VAR = "CLAUDE_SKILLS_BRIEF_GATE";
var BRIEF_GATE_MAX_BLOCKS_ENV_VAR = "CLAUDE_SKILLS_BRIEF_GATE_MAX_BLOCKS";
var RESEARCH_GATE_ENV_VAR = "CLAUDE_SKILLS_RESEARCH_GATE";
var RESEARCH_GATE_MAX_BLOCKS_ENV_VAR = "CLAUDE_SKILLS_RESEARCH_GATE_MAX_BLOCKS";
var COMPLETENESS_GATE_ENV_VAR = "CLAUDE_SKILLS_COMPLETENESS_GATE";
var COMPLETENESS_GATE_MAX_BLOCKS_ENV_VAR = "CLAUDE_SKILLS_COMPLETENESS_GATE_MAX_BLOCKS";
var MEMORY_GATE_ENV_VAR = "CLAUDE_SKILLS_MEMORY_GATE";
var LEARNED_SKILL_GATE_ENV_VAR = "CLAUDE_SKILLS_LEARNED_SKILL_GATE";

// Canonical State Marker Directories
var IRON_LAW_SATISFIED_DIR = "iron-law-satisfied";
var IRON_LAW_LEGACY_GATE_DIR = "iron-law-gate";
var REVIEW_GATE_DIR = "review-gate";
var BRIEF_GATE_DIR = "brief-gate";
var COMPLETENESS_GATE_DIR = "completeness-gate";

// Canonical Gate Decisions
var GATE_DECISION_BLOCK = "block";
var GATE_DECISION_WARN = "warn";
var GATE_DECISION_ESCALATE = "escalate";
var GATE_DECISION_NUDGE = "nudge";
var GATE_DECISION_OFF = "off";

function sessionMarkerDirectory(host) {
  return path.join(keelStateRoot(), `${host}-session-started`);
}
function ironLawMarkerDirectory() {
  return path.join(keelStateRoot(), IRON_LAW_SATISFIED_DIR);
}
function legacyIronLawMarkerDirectory() {
  return path.join(os.homedir(), ".claude", "state", IRON_LAW_SATISFIED_DIR);
}
function sanitizeSessionKey(sessionID) {
  const raw = (sessionID || "default").trim() || "default";
  return raw.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "workspace";
}
function markerPath(dir, sessionID, sanitize = false) {
  return path.join(dir, sanitize ? sanitizeSessionKey(sessionID) : sessionID);
}
function ensureMarkerDirectory(dir) {
  try {
    fs.mkdirSync(dir, { recursive: true });
  } catch {}
}
function hasMarker(dir, sessionID, sanitize = false) {
  ensureMarkerDirectory(dir);
  try {
    return fs.existsSync(markerPath(dir, sessionID, sanitize));
  } catch {
    return false;
  }
}
function setMarker(dir, sessionID, contents = "", sanitize = false) {
  ensureMarkerDirectory(dir);
  try {
    fs.writeFileSync(markerPath(dir, sessionID, sanitize), contents, "utf-8");
  } catch {}
}
function clearMarker(dir, sessionID, sanitize = false) {
  try {
    fs.rmSync(markerPath(dir, sessionID, sanitize), { force: true });
  } catch {}
}
function hasSessionStarted(dir, sessionID) {
  return hasMarker(dir, sessionID, true);
}
function markSessionStarted(dir, sessionID) {
  setMarker(dir, sessionID, "", true);
}
function clearSessionStarted(dir, sessionID) {
  clearMarker(dir, sessionID, true);
}
function clearIronLawMarker(sessionID) {
  clearMarker(ironLawMarkerDirectory(), sessionID, true);
  clearMarker(legacyIronLawMarkerDirectory(), sessionID, true);
}
// Normalized (lowercase, separators removed) so `StrReplace`, `str_replace`, and
// `edit_file` classify together. Hand-maintained copy of bridge-core.ts.
function normalizeToolName(toolName) {
  return (toolName || "").toLowerCase().replace(/[^a-z0-9]/g, "");
}
var EDIT_CLASS_TOOL_NAMES = new Set([
  "edit",
  "write",
  "multiedit",
  "notebookedit",
  "applypatch",
  "delete",
  "strreplace",
  "patch",
  "searchreplace",
  "writetofile",
  "replacefilecontent",
  "multireplacefilecontent",
  "editfile",
  "writefile",
  "createfile",
  "deletefile",
  "strreplaceeditor",
  "multieditfile"
]);
var SHELL_TOOL_NAMES = new Set([
  "bash",
  "shell",
  "sh",
  "zsh",
  "fish",
  "powershell",
  "pwsh",
  "cmd",
  "shellcommand",
  "command",
  "terminal",
  "runcommand",
  "runterminalcommand",
  "execcommand",
  "localshell",
  "unifiedexec"
]);
function isEditClassTool(toolName) {
  var normalized = normalizeToolName(toolName);
  return normalized.length > 0 && EDIT_CLASS_TOOL_NAMES.has(normalized);
}
function isShellTool(toolName) {
  var normalized = normalizeToolName(toolName);
  return normalized.length > 0 && SHELL_TOOL_NAMES.has(normalized);
}
var KEEL_RESEARCH_SUBCOMMANDS = [
  "system-map",
  "system_map",
  "recall",
  "doctor",
  "code-search",
  "code_search",
  "skill-route",
  "skill_route",
  "skill-list",
  "skill_list",
  "skill-get",
  "skill_get",
  "context-brief",
  "context_brief",
  "memory status",
  "memory recall",
  "memory system-map",
  "memory scope",
  "anvil prefix-check",
  "anvil sieve"
];
var SAFE_PIPE_CONSUMERS = new Set([
  "cat", "head", "tail", "grep", "egrep", "fgrep", "jq", "less", "more", "wc",
  "sort", "uniq", "cut", "tr", "column", "fold", "awk", "sed", "findstr",
  "select-string", "select-object", "where-object", "measure-object",
  "out-string", "out-host", "out-null"
]);
function shellCommandWords(command) {
  const words = [];
  let current = "";
  let quote = "";
  for (let index = 0;index < command.length; index += 1) {
    const character = command[index];
    if (quote) {
      if (character === quote) {
        quote = "";
      } else {
        current += character;
      }
      continue;
    }
    if (character === "'" || character === '"') {
      quote = character;
      continue;
    }
    if (/\s/.test(character)) {
      if (current) {
        words.push(current);
        current = "";
      }
      continue;
    }
    if (
      "|;`\n".includes(character) ||
      (character === "$" && command[index + 1] === "(") ||
      (character === "&" && command[index + 1] === "&")
    ) {
      break;
    }
    current += character;
  }
  if (quote)
    return [];
  if (current)
    words.push(current);
  return words;
}
function isKeelExecutable(value) {
  const basename = value.replace(/\\/g, "/").split("/").pop() ?? value;
  return basename === "keel" || basename === "keel.exe";
}
function isKeelReadingCommand(command) {
  const trimmed = command.trim();
  if (!trimmed) return false;

  const stages = [];
  let currentStage = "";
  let quote = "";
  for (let i = 0; i < trimmed.length; i += 1) {
    const ch = trimmed[i];
    if (quote) {
      if (ch === quote) quote = "";
      currentStage += ch;
      continue;
    }
    if (ch === "'" || ch === '"') {
      quote = ch;
      currentStage += ch;
      continue;
    }
    if (
      ch === ";" ||
      ch === "`" ||
      ch === "\n" ||
      (ch === "$" && trimmed[i + 1] === "(") ||
      (ch === "&" && trimmed[i + 1] === "&")
    ) {
      return false;
    }
    if (ch === "|") {
      stages.push(currentStage.trim());
      currentStage = "";
      continue;
    }
    currentStage += ch;
  }
  if (quote) return false;
  if (currentStage.trim()) stages.push(currentStage.trim());

  if (stages.length === 0) return false;

  let firstWords = shellCommandWords(stages[0].toLowerCase());
  if (firstWords.length >= 3 && isKeelExecutable(firstWords[0]) && firstWords[1] === "run" && firstWords[2] === "--") {
    firstWords = firstWords.slice(3);
  }
  if (firstWords.length < 2 || !firstWords[0] || !isKeelExecutable(firstWords[0]))
    return false;
  const subcommand = firstWords.slice(1).join(" ");
  const isFirstStageReading = KEEL_RESEARCH_SUBCOMMANDS.some((hit) => subcommand === hit || subcommand.startsWith(`${hit} `));
  if (!isFirstStageReading) return false;

  for (let i = 1; i < stages.length; i += 1) {
    const words = shellCommandWords(stages[i].toLowerCase());
    if (words.length === 0) return false;
    const consumer = words[0];
    if (!SAFE_PIPE_CONSUMERS.has(consumer)) return false;
  }

  return true;
}
function parseGateResponse(output) {
  const normalized = output.replace(/\r/g, "");
  if (normalized.startsWith("KEEL_GATE_DENY")) {
    return { status: "deny", reason: normalized.split("\n").slice(1).join("\n").trim() };
  }
  if (normalized.startsWith("KEEL_GATE_ESCALATE")) {
    return { status: "escalate", reason: normalized.split("\n").slice(1).join("\n").trim() };
  }
  if (normalized.startsWith("KEEL_GATE_WARN")) {
    return { status: "warn", reason: normalized.split("\n").slice(1).join("\n").trim() };
  }
  if (normalized.startsWith("KEEL_GATE_ALLOW"))
    return { status: "allow", reason: "" };
  return { status: "unknown", reason: "" };
}
function parseRewriteResponse(output) {
  if (!output.startsWith("KEEL_REWRITE "))
    return "";
  return output.slice("KEEL_REWRITE ".length).trim();
}

// codex/keel-codex.ts
function eventName(input) {
  return input.hook_event_name ?? input.event ?? "";
}
function toolName(input) {
  return input.tool_name ?? input.tool ?? "";
}
function toolFailed(input) {
  if (typeof input.failed === "boolean") {
    return input.failed;
  }
  if (input.tool_response && typeof input.tool_response === "object") {
    const response = input.tool_response;
    return response.isError === true || response.error != null;
  }
  return false;
}
var BRIDGE_BIN = resolveBinary();
var STARTED_DIR = sessionMarkerDirectory("codex");
function hasStarted(sessionID) {
  return hasSessionStarted(STARTED_DIR, sessionID);
}
function markStarted(sessionID) {
  markSessionStarted(STARTED_DIR, sessionID);
}
function clearMarker2(sessionID) {
  clearSessionStarted(STARTED_DIR, sessionID);
}
function denyOutput(reason) {
  return JSON.stringify({
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: reason
    }
  });
}
// Runs `keel hook <event>`, the native hook router rather than a bridge subcommand.
function runHookStop(payload, timeoutMs = 5e3) {
  try {
    const result = execFileSync(BRIDGE_BIN, ["hook", "stop"], {
      timeout: timeoutMs,
      input: JSON.stringify(payload),
      stdio: ["pipe", "pipe", "pipe"],
      encoding: "utf-8",
      windowsHide: true
    });
    return result ?? "";
  } catch {
    return "";
  }
}
function runBridge(subcommand, args, timeoutMs = 5e3) {
  try {
    const result = execFileSync(BRIDGE_BIN, ["bridge", subcommand, ...args], {
      timeout: timeoutMs,
      stdio: ["pipe", "pipe", "pipe"],
      encoding: "utf-8",
      windowsHide: true
    });
    return result ?? "";
  } catch {
    return "";
  }
}
function runBridgeWithStdin(subcommand, args, stdin) {
  try {
    const result = execFileSync(BRIDGE_BIN, ["bridge", subcommand, ...args], {
      timeout: 2e3,
      input: stdin,
      stdio: ["pipe", "pipe", "pipe"],
      encoding: "utf-8",
      windowsHide: true
    });
    return result ?? "";
  } catch {
    return "";
  }
}
function resolveSessionContext(input) {
  return {
    sessionID: input.session_id ?? "unknown",
    cwd: input.cwd ?? process.cwd()
  };
}
function handleSessionStart(input) {
  const { sessionID, cwd } = resolveSessionContext(input);
  if (hasStarted(sessionID))
    return "";
  const text = runBridge("session-start", [
    "--session",
    sessionID,
    "--cwd",
    cwd
  ]);
  markStarted(sessionID);
  return text;
}
function handleUserPromptSubmit(input) {
  const { sessionID, cwd } = resolveSessionContext(input);
  const prompt = input.prompt ?? "";
  if (!prompt)
    return "";
  return runBridge("user-prompt", [
    "--session",
    sessionID,
    "--cwd",
    cwd,
    "--prompt",
    prompt
  ]);
}
function extractCommand(toolInput) {
  if (toolInput && typeof toolInput === "object" && "command" in toolInput) {
    const cmd = toolInput.command;
    return typeof cmd === "string" ? cmd : "";
  }
  return "";
}
function rewriteToolName(toolName) {
  return process.platform === "win32" && toolName.toLowerCase() === "bash"
    ? "powershell"
    : toolName;
}
function handlePreToolUse(input, isPre) {
  if (!isPre) {
    const { sessionID: sessionID2, cwd: cwd2 } = resolveSessionContext(input);
    const currentToolName2 = toolName(input);
    const stdin2 = JSON.stringify({
      tool_input: input.tool_input ?? null,
      tool_response: input.tool_response ?? null,
    });
    const args = [
      "--session",
      sessionID2,
      "--cwd",
      cwd2,
      "--tool",
      currentToolName2,
      "--phase",
      "post"
    ];
    if (toolFailed(input))
      args.push("--failed");
    runBridgeWithStdin("observe", args, stdin2);
    return "";
  }
  const { sessionID, cwd } = resolveSessionContext(input);
  const currentToolName = toolName(input);
  if (isEditClassTool(currentToolName)) {
    const pathArg = (input.tool_input && (input.tool_input.path || input.tool_input.file_path || input.tool_input.filePath)) || input.path || "";
    const gateArgs = ["--session", sessionID, "--cwd", cwd, "--tool", currentToolName];
    if (pathArg) gateArgs.push("--path", String(pathArg));
    const gate = runBridge("pre-tool-use", gateArgs, 5000);
    const gateResult = parseGateResponse(gate);
    if (gateResult.status === "deny") {
      return denyOutput(gateResult.reason || "keel Iron Law gate: call system_map/recall/context_brief before editing.");
    }
    if (gateResult.status !== "allow") {
      return denyOutput("keel Iron Law gate could not be evaluated (keel did not respond in time). Retry the edit; if it persists, run `keel doctor`.");
    }
  }
  if (isShellTool(currentToolName)) {
    const command = extractCommand(input.tool_input);
    const readingCommand = command ? isKeelReadingCommand(command) : false;
    const gateArgs = [
      "--session",
      sessionID,
      "--cwd",
      cwd,
      "--tool",
      currentToolName
    ];
    if (command)
      gateArgs.push("--command", command);
    const gate = runBridge("pre-tool-use", gateArgs, 5000);
    const gateResult = parseGateResponse(gate);
    if (gateResult.status === "deny") {
      return denyOutput(gateResult.reason || (readingCommand ? "keel reading command gate denied this command." : "keel Iron Law gate: call system_map/recall/context_brief before running shell commands."));
    }
    if (gateResult.status !== "allow") {
      return denyOutput("keel Iron Law shell gate could not be evaluated. Retry after running `keel doctor`.");
    }
    if (command) {
      const rewritten = parseRewriteResponse(runBridgeWithStdin("rewrite", ["--tool", rewriteToolName(currentToolName)], command));
      if (rewritten) {
        return JSON.stringify({
          hookSpecificOutput: {
            hookEventName: "PreToolUse",
            permissionDecision: "allow",
            updatedInput: { command: rewritten }
          }
        });
      }
    }
  }
  return "";
}
function handlePreCompact(input) {
  const { sessionID, cwd } = resolveSessionContext(input);
  runBridge("pre-compact", [
    "--session",
    sessionID,
    "--cwd",
    cwd
  ]);
  return "";
}
function handlePostCompact(input) {
  const { sessionID, cwd } = resolveSessionContext(input);
  return runBridge("post-compact", [
    "--session",
    sessionID,
    "--cwd",
    cwd
  ]);
}
function handleStop(input) {
  const { sessionID, cwd } = resolveSessionContext(input);
  const stopHookActive = input.stop_hook_active === true || Number(input.execution_num ?? 1) > 1;
  const output = runHookStop({ session_id: sessionID, cwd, stop_hook_active: stopHookActive });
  if (!output.trim()) return "";
  try {
    const decision = JSON.parse(output);
    if (decision.decision === "block") {
      return JSON.stringify({
        decision: "block",
        reason: decision.reason || "Keel closeout checks are incomplete."
      });
    }
  } catch {
    return JSON.stringify({
      decision: "block",
      reason: "Keel could not evaluate closeout. Run `keel doctor` and retry."
    });
  }
  return "";
}
function handleSessionEnd(input) {
  const { sessionID, cwd } = resolveSessionContext(input);
  runBridge("session-end", [
    "--session",
    sessionID,
    "--cwd",
    cwd
  ]);
  clearMarker2(sessionID);
  clearIronLawMarker(sessionID);
  return "";
}
function main() {
  let raw = "";
  try {
    const chunks = [];
    const buf = Buffer.alloc(65536);
    while (true) {
      const n = fs2.readSync(process.stdin.fd, buf, 0, buf.length, null);
      if (n === 0)
        break;
      chunks.push(Buffer.from(buf.subarray(0, n)));
    }
    raw = Buffer.concat(chunks).toString("utf-8");
  } catch {
    return;
  }
  let input;
  try {
    input = JSON.parse(raw);
  } catch {
    return;
  }
  let contextText = "";
  try {
    switch (eventName(input)) {
      case "SessionStart":
        contextText = handleSessionStart(input);
        break;
      case "UserPromptSubmit":
        contextText = handleUserPromptSubmit(input);
        break;
      case "PreToolUse":
        contextText = handlePreToolUse(input, true);
        break;
      case "PostToolUse":
        handlePreToolUse(input, false);
        break;
      case "PostToolUseFailure":
        // The event name is authoritative here; Codex does not have to set `failed`.
        handlePreToolUse({ ...input, failed: true }, false);
        break;
      case "PreCompact":
        contextText = handlePreCompact(input);
        break;
      case "PostCompact": {
        const postCompactContext = handlePostCompact(input);
        contextText = postCompactContext ? JSON.stringify({
          hookSpecificOutput: {
            hookEventName: "PostCompact",
            additionalContext: postCompactContext
          }
        }) : "";
        break;
      }
      case "Stop":
        contextText = handleStop(input);
        break;
      case "SessionEnd":
        handleSessionEnd(input);
        break;
      default:
        break;
    }
  } catch {}
  if (contextText) {
    process.stdout.write(contextText);
  }
}
main();
