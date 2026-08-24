import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const BIN_NAME = os.platform() === "win32" ? "keel.exe" : "keel";

/** Resolve the installed bridge binary, falling back to PATH. */
export function resolveBinary(): string {
  const home = os.homedir();
  const candidates: string[] = [];
  const envHome = process.env.KEEL_HOME;
  if (envHome && envHome.trim()) candidates.push(path.join(envHome.trim(), BIN_NAME));
  candidates.push(path.join(home, ".keel", BIN_NAME));
  candidates.push(path.join(home, ".claude", BIN_NAME));
  for (const candidate of candidates) {
    try {
      const usable =
        os.platform() === "win32"
          ? fs.existsSync(candidate)
          : (() => {
              try {
                fs.accessSync(candidate, fs.constants.X_OK);
                return true;
              } catch {
                return false;
              }
            })();
      if (usable) return candidate;
    } catch {
      // fs failure, try the next candidate
    }
  }
  return BIN_NAME;
}

/** Resolve the host-neutral state root, retaining the legacy bridge fallback. */
export function keelStateRoot(): string {
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

export function sessionMarkerDirectory(host: string): string {
  return path.join(keelStateRoot(), `${host}-session-started`);
}

export function ironLawMarkerDirectory(): string {
  return path.join(keelStateRoot(), "iron-law-satisfied");
}

export function legacyIronLawMarkerDirectory(): string {
  return path.join(os.homedir(), ".claude", "state", "iron-law-satisfied");
}

/** Match Rust `sanitize_memory_key`: lowercase alnum, other runs become `-`. */
export function sanitizeSessionKey(sessionID: string): string {
  const raw = (sessionID || "default").trim() || "default";
  return raw.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "workspace";
}

export function markerPath(dir: string, sessionID: string, sanitize = false): string {
  return path.join(dir, sanitize ? sanitizeSessionKey(sessionID) : sessionID);
}

export function ensureMarkerDirectory(dir: string): void {
  try {
    fs.mkdirSync(dir, { recursive: true });
  } catch {
    /* best-effort */
  }
}

export function hasMarker(dir: string, sessionID: string, sanitize = false): boolean {
  ensureMarkerDirectory(dir);
  try {
    return fs.existsSync(markerPath(dir, sessionID, sanitize));
  } catch {
    return false;
  }
}

export function setMarker(
  dir: string,
  sessionID: string,
  contents = "",
  sanitize = false,
): void {
  ensureMarkerDirectory(dir);
  try {
    fs.writeFileSync(markerPath(dir, sessionID, sanitize), contents, "utf-8");
  } catch {
    /* best-effort */
  }
}

export function clearMarker(dir: string, sessionID: string, sanitize = false): void {
  try {
    fs.rmSync(markerPath(dir, sessionID, sanitize), { force: true });
  } catch {
    /* best-effort */
  }
}

export function hasSessionStarted(dir: string, sessionID: string): boolean {
  return hasMarker(dir, sessionID, true);
}

export function markSessionStarted(dir: string, sessionID: string): void {
  setMarker(dir, sessionID, "", true);
}

export function clearSessionStarted(dir: string, sessionID: string): void {
  clearMarker(dir, sessionID, true);
}

export function ironLawSatisfied(sessionID: string): boolean {
  const currentDir = ironLawMarkerDirectory();
  const legacyDir = legacyIronLawMarkerDirectory();
  ensureMarkerDirectory(currentDir);
  try {
    return fs.existsSync(markerPath(currentDir, sessionID, true)) ||
      fs.existsSync(markerPath(legacyDir, sessionID, true));
  } catch {
    return false;
  }
}

export function markIronLawSatisfied(sessionID: string): void {
  setMarker(ironLawMarkerDirectory(), sessionID, "satisfied", true);
}

export function clearIronLawMarker(sessionID: string): void {
  clearMarker(ironLawMarkerDirectory(), sessionID, true);
  clearMarker(legacyIronLawMarkerDirectory(), sessionID, true);
}

const EDIT_CLASS_TOOL_NAMES: Record<string, true> = {
  edit: true, write: true, multiedit: true, multi_edit: true,
  notebookedit: true, notebook_edit: true, apply_patch: true, applypatch: true,
  str_replace: true, strreplace: true, search_replace: true, searchreplace: true,
  patch: true,
};
const SHELL_TOOL_NAMES: Record<string, true> = {
  bash: true, shell: true, sh: true, zsh: true, fish: true,
  powershell: true, pwsh: true, cmd: true,
};

export function isEditClassTool(toolName: string): boolean {
  return EDIT_CLASS_TOOL_NAMES[toolName.toLowerCase()] === true;
}

export function isShellTool(toolName: string): boolean {
  return SHELL_TOOL_NAMES[toolName.toLowerCase()] === true;
}

export function isKeelResearchTool(toolName: string): boolean {
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

const KEEL_RESEARCH_SUBCOMMANDS = [
  "system-map", "system_map", "recall", "doctor", "code-search", "code_search",
  "skill-route", "skill_route", "skill-list", "skill_list", "skill-get", "skill_get",
  "context-brief", "context_brief", "memory status", "memory recall",
  "memory system-map", "memory scope", "anvil prefix-check", "anvil sieve",
];

function shellCommandWords(command: string): string[] {
  const words: string[] = [];
  let current = "";
  let quote = "";
  for (let index = 0; index < command.length; index += 1) {
    const character = command[index];
    if (quote) {
      if (character === quote) {
        quote = "";
      } else {
        current += character;
      }
      continue;
    }
    if (character === "'" || character === "\"") {
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
    if ("&|;`\n".includes(character) || (character === "$" && command[index + 1] === "(")) {
      return [];
    }
    current += character;
  }
  if (quote) return [];
  if (current) words.push(current);
  return words;
}

function isKeelExecutable(value: string): boolean {
  const basename = value.replace(/\\/g, "/").split("/").pop() ?? value;
  return basename === "keel" || basename === "keel.exe";
}

export function isKeelReadingCommand(command: string): boolean {
  let words = shellCommandWords(command.trim().toLowerCase());
  if (words.length >= 3 && isKeelExecutable(words[0]) && words[1] === "run" && words[2] === "--") {
    words = words.slice(3);
  }
  if (words.length < 2 || !isKeelExecutable(words[0])) return false;
  const subcommand = words.slice(1).join(" ");
  return KEEL_RESEARCH_SUBCOMMANDS.some(
    (hit) => subcommand === hit || subcommand.startsWith(`${hit} `),
  );
}

export type GateResponse = "allow" | "deny" | "unknown";

export function parseGateResponse(output: string): { status: GateResponse; reason: string } {
  if (output.startsWith("KEEL_GATE_DENY")) {
    return { status: "deny", reason: output.split("\n").slice(1).join("\n").trim() };
  }
  if (output.startsWith("KEEL_GATE_ALLOW")) return { status: "allow", reason: "" };
  return { status: "unknown", reason: "" };
}

export function parseRewriteResponse(output: string): string {
  if (!output.startsWith("KEEL_REWRITE ")) return "";
  return output.slice("KEEL_REWRITE ".length).trim();
}
