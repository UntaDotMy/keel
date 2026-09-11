#!/usr/bin/env node

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { createInterface } from "node:readline";

const MCP_PROTOCOL_VERSION = "2026-07-28";
const MCP_FRAME_LIMIT_BYTES = 24_000;
const MCP_TEXT_LIMIT_CHARS = 12_000;
const REQUEST_TIMEOUT_MS = 30_000;
const CLOSE_TIMEOUT_MS = 10_000;
const NOISY_OUTPUT_BYTES = Number(process.env.KEEL_RELEASE_SMOKE_NOISY_OUTPUT_BYTES ?? "20000");
if (!Number.isSafeInteger(NOISY_OUTPUT_BYTES) || NOISY_OUTPUT_BYTES < 12_000) {
  throw new Error(
    "KEEL_RELEASE_SMOKE_NOISY_OUTPUT_BYTES must be an integer >= 12000 so the payload still exceeds the context firewall budget after the MCP text cap",
  );
}

function parseArguments(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (!argument.startsWith("--")) {
      throw new Error(`unexpected argument: ${argument}`);
    }
    const name = argument.slice(2);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) {
      throw new Error(`missing value for --${name}`);
    }
    values[name] = value;
    index += 1;
  }
  for (const required of ["binary", "bundle-root", "claude-home", "report"]) {
    if (!values[required]) {
      throw new Error(`missing required --${required}`);
    }
  }
  return {
    binary: resolve(values.binary),
    bundleRoot: resolve(values["bundle-root"]),
    claudeHome: resolve(values["claude-home"]),
    report: resolve(values.report),
  };
}

class McpSession {
  constructor(binary, bundleRoot, claudeHome) {
    this.closed = false;
    this.nextId = 1;
    this.waiters = new Map();
    this.stderrBytes = 0;
    this.child = spawn(binary, ["mcp", "serve"], {
      cwd: bundleRoot,
      env: {
        ...process.env,
        CLAUDE_TARGET_OVERRIDE: claudeHome,
        KEEL_HOME: claudeHome,
        HOME: claudeHome,
        USERPROFILE: claudeHome,
        KEEL_MCP_ALLOW_UNSAFE_COMMANDS: "1",
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.stdout = createInterface({ input: this.child.stdout });
    this.stdout.on("line", (line) => this.handleLine(line));
    this.child.stderr.on("data", (chunk) => {
      // Keep diagnostics bounded. The report only needs a short tail when a
      // process fails; server output must never become a second unbounded log.
      this.stderrBytes = Math.min(this.stderrBytes + chunk.length, 16_384);
    });
    this.child.on("error", (error) => this.failAll(error));
    this.child.on("close", (code, signal) => {
      this.closed = true;
      this.failAll(new Error(`MCP server exited before responding (code=${code}, signal=${signal ?? "none"})`));
    });
  }

  handleLine(line) {
    if (Buffer.byteLength(line, "utf8") > MCP_FRAME_LIMIT_BYTES) {
      this.failAll(new Error(`MCP response frame exceeded ${MCP_FRAME_LIMIT_BYTES} bytes`));
      return;
    }
    let message;
    try {
      message = JSON.parse(line);
    } catch (error) {
      this.failAll(new Error(`MCP server emitted invalid JSON: ${error.message}`));
      return;
    }
    if (message === null || typeof message !== "object" || !("id" in message)) {
      return;
    }
    const waiter = this.waiters.get(message.id);
    if (!waiter) {
      this.failAll(new Error(`MCP response used an unexpected id: ${String(message.id)}`));
      return;
    }
    this.waiters.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) {
      waiter.reject(new Error(`MCP request ${waiter.method} failed: ${JSON.stringify(message.error)}`));
    } else {
      waiter.resolve(message.result);
    }
  }

  failAll(error) {
    for (const waiter of this.waiters.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
    this.waiters.clear();
  }

  request(method, params) {
    if (this.closed) {
      return Promise.reject(new Error(`cannot request from closed MCP server: ${method}`));
    }
    const id = this.nextId;
    this.nextId += 1;
    const request = {
      jsonrpc: "2.0", id, method,
      params: {
        ...params,
        _meta: {
          ...params?._meta,
          "io.modelcontextprotocol/protocolVersion": MCP_PROTOCOL_VERSION,
          "io.modelcontextprotocol/clientCapabilities": {},
        },
      },
    };
    return new Promise((resolveResult, rejectResult) => {
      const timer = setTimeout(() => {
        this.waiters.delete(id);
        rejectResult(new Error(`MCP request ${method} timed out after ${REQUEST_TIMEOUT_MS}ms`));
      }, REQUEST_TIMEOUT_MS);
      this.waiters.set(id, { method, resolve: resolveResult, reject: rejectResult, timer });
      try {
        this.child.stdin.write(`${JSON.stringify(request)}\n`);
      } catch (error) {
        clearTimeout(timer);
        this.waiters.delete(id);
        rejectResult(error);
      }
    });
  }

  async close() {
    if (this.closed) {
      return;
    }
    const closed = new Promise((resolveClosed) => {
      this.child.once("close", () => resolveClosed(true));
    });
    this.child.stdin.end();
    const timer = setTimeout(() => {
      this.child.kill();
    }, CLOSE_TIMEOUT_MS);
    const exited = await Promise.race([
      closed,
      new Promise((resolveClosed) => setTimeout(() => resolveClosed(false), CLOSE_TIMEOUT_MS + 1_000)),
    ]);
    clearTimeout(timer);
    if (!exited) {
      throw new Error(`MCP server did not exit within ${CLOSE_TIMEOUT_MS}ms after stdin EOF`);
    }
  }
}

function requireObject(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value;
}

function requireToolText(result, label) {
  requireObject(result, `${label} result`);
  if (result.isError === true) {
    throw new Error(`${label} returned isError=true: ${JSON.stringify(result)}`);
  }
  const content = result.content;
  if (!Array.isArray(content) || content.length !== 1 || content[0]?.type !== "text") {
    throw new Error(`${label} did not return one text content item: ${JSON.stringify(result)}`);
  }
  if (typeof content[0].text !== "string") {
    throw new Error(`${label} text content is not a string`);
  }
  return content[0].text;
}

function parseToolJson(result, label) {
  const text = requireToolText(result, label);
  try {
    return { text, value: JSON.parse(text) };
  } catch (error) {
    throw new Error(`${label} text was not JSON: ${error.message}`);
  }
}

function requireBoundedContext(result, label, expectedTruncation = false) {
  const text = requireToolText(result, label);
  if (text.length > MCP_TEXT_LIMIT_CHARS) {
    throw new Error(`${label} text exceeded ${MCP_TEXT_LIMIT_CHARS} characters (actual=${text.length})`);
  }
  const context = requireObject(result.context, `${label} context metadata`);
  const visibleTokens = Number(context.visible_tokens);
  const budgetTokens = Number(context.budget);
  if (!Number.isFinite(visibleTokens) || !Number.isFinite(budgetTokens) || visibleTokens > budgetTokens) {
    throw new Error(`${label} exceeded its context budget: ${JSON.stringify(context)}`);
  }
  if (expectedTruncation && context.truncated !== true) {
    throw new Error(`${label} did not report truncation for oversized output: ${JSON.stringify(context)}`);
  }
  return {
    visibleTokens,
    budgetTokens,
    rawTokens: Number(context.raw_tokens),
    truncated: context.truncated === true,
  };
}

async function callTool(session, name, argumentsValue) {
  return session.request("tools/call", { name, arguments: argumentsValue });
}

async function recordCheck(checks, name, operation) {
  const startedAt = Date.now();
  try {
    const details = await operation();
    checks.push({ name, status: "passed", durationMs: Date.now() - startedAt, ...details });
    return details;
  } catch (error) {
    checks.push({ name, status: "failed", durationMs: Date.now() - startedAt, error: error.message });
    throw error;
  }
}

async function runSmoke(options) {
  const checks = [];
  const marker = `keelsmoke${Date.now().toString(36)}`;
  let session;
  try {
    session = new McpSession(options.binary, options.bundleRoot, options.claudeHome);

    await recordCheck(checks, "server-discovery", async () => {
      const result = await session.request("server/discover", {});
      if (result?.resultType !== "complete" || !result?.supportedVersions?.includes(MCP_PROTOCOL_VERSION) || result?._meta?.["io.modelcontextprotocol/serverInfo"]?.name !== "keel") {
        throw new Error(`unexpected discovery result: ${JSON.stringify(result)}`);
      }
      return { protocolVersion: MCP_PROTOCOL_VERSION, server: result._meta["io.modelcontextprotocol/serverInfo"] };
    });

    await recordCheck(checks, "tools-list", async () => {
      const tools = [];
      const seen = new Set();
      let cursor;
      do {
        const result = await session.request("tools/list", cursor ? { cursor } : {});
        if (result?.resultType !== "complete" || !Array.isArray(result.tools)) {
          throw new Error(`invalid catalog page: ${JSON.stringify(result)}`);
        }
        tools.push(...result.tools);
        cursor = result.nextCursor;
        if (cursor) {
          if (seen.has(cursor)) throw new Error("tools/list repeated a cursor");
          seen.add(cursor);
        }
      } while (cursor);
      if (!Array.isArray(tools) || tools.length < 7) {
        throw new Error(`tools/list returned an unexpectedly small catalog: ${JSON.stringify(tools)}`);
      }
      const names = new Set(tools.map((tool) => tool?.name));
      for (const required of ["run_command", "skill_route", "skill_get", "memory_status", "brief_create", "recall"]) {
        if (!names.has(required)) {
          throw new Error(`tools/list omitted required tool ${required}`);
        }
      }
      for (const tool of tools) {
        if (tool?.inputSchema?.type !== "object") {
          throw new Error(`tool ${tool?.name ?? "<unnamed>"} has an invalid inputSchema`);
        }
      }
      return { toolCount: tools.length, pageCount: seen.size + 1 };
    });

    await recordCheck(checks, "memory-write-seed", async () => {
      const result = await callTool(session, "brief_create", {
        id: `release-smoke-${marker}`,
        request: `packaged release smoke memory marker ${marker}`,
      });
      const { value } = parseToolJson(result, "brief_create");
      const context = requireBoundedContext(result, "brief_create");
      if (value.written !== true || !value.brief?.request?.includes(marker)) {
        throw new Error(`brief_create did not persist the smoke marker: ${JSON.stringify(value)}`);
      }
      return { ...context, briefId: value.brief.id };
    });

    const route = await recordCheck(checks, "skill-activation-route", async () => {
      const result = await callTool(session, "skill_route", {
        prompt: "preserve existing flow before editing brownfield source",
      });
      const { value } = parseToolJson(result, "skill_route");
      const context = requireBoundedContext(result, "skill_route");
      if (value.matched !== true || value.present !== true || typeof value.name !== "string") {
        throw new Error(`skill_route did not select an installed skill: ${JSON.stringify(value)}`);
      }
      return { ...context, skill: value.name, path: value.path };
    });

    const activatedSkill = await recordCheck(checks, "skill-activation-get", async () => {
      const result = await callTool(session, "skill_get", { name: route.skill, level: 1 });
      const { value } = parseToolJson(result, "skill_get");
      const context = requireBoundedContext(result, "skill_get");
      if (value.name !== route.skill || value.level !== 1 || typeof value.body !== "string" || value.body.length === 0) {
        throw new Error(`skill_get did not return an activated skill body: ${JSON.stringify(value)}`);
      }
      if (Number(value.visibleTokens) > Number(value.budgetTokens)) {
        throw new Error(`skill_get body exceeded its own budget: ${JSON.stringify(value)}`);
      }
      return { ...context, skill: value.name, visibleSkillTokens: value.visibleTokens };
    });

    await recordCheck(checks, "memory-status", async () => {
      const result = await callTool(session, "memory_status", {});
      const { value } = parseToolJson(result, "memory_status");
      const context = requireBoundedContext(result, "memory_status");
      if (!Number.isFinite(Number(value.index?.documents)) || Number(value.index.documents) < 1) {
        throw new Error(`memory_status did not expose an indexed document: ${JSON.stringify(value)}`);
      }
      return { ...context, indexedDocuments: value.index.documents };
    });

    await recordCheck(checks, "memory-retrieval", async () => {
      const result = await callTool(session, "recall", { query: marker, limit: 1 });
      const { value } = parseToolJson(result, "recall");
      const context = requireBoundedContext(result, "recall");
      const matches = value.matches;
      if (!Array.isArray(matches) || matches.length < 1 || !matches.some((match) => String(match?.excerpt ?? match?.snippet ?? "").includes(marker))) {
        throw new Error(`recall did not retrieve the persisted smoke marker: ${JSON.stringify(value)}`);
      }
      if (matches.length > 1 || matches.some((match) => String(match?.excerpt ?? "").length > 700)) {
        throw new Error(`recall result exceeded its requested bounded shape: ${JSON.stringify(value)}`);
      }
      return { ...context, matchCount: matches.length };
    });

    await recordCheck(checks, "noisy-command-bounded-result", async () => {
      // One long line of distinct word-shaped tokens survives the command reducer,
      // so the context firewall is the layer that must bound and mark it.
      const noisyExpression = `let s = ""; for (let i = 0; s.length < ${NOISY_OUTPUT_BYTES}; i += 1) { s += "w" + i + " "; } process.stdout.write(s.slice(0, ${NOISY_OUTPUT_BYTES}));`;
      const result = await callTool(session, "run_command", {
        argv: ["node", "-e", noisyExpression],
        cwd: options.bundleRoot,
        wait: true,
        json: true,
        confirm: true,
      });
      const context = requireBoundedContext(result, "run_command", true);
      const text = requireToolText(result, "run_command");
      if (!(context.rawTokens > context.visibleTokens)) {
        throw new Error(`run_command did not measure a reduced oversized payload: ${JSON.stringify(result.context)}`);
      }
      if (!text.includes("[keel] context truncated")) {
        throw new Error(`run_command did not report a bounded truncation state: ${text.slice(-200)}`);
      }
      return { ...context, sourceBytes: NOISY_OUTPUT_BYTES };
    });

    await session.close();
    session = new McpSession(options.binary, options.bundleRoot, options.claudeHome);
    await recordCheck(checks, "restart-and-reconnect", async () => {
      const result = await session.request("server/discover", {});
      if (result?.resultType !== "complete" || !result?.supportedVersions?.includes(MCP_PROTOCOL_VERSION) || result?._meta?.["io.modelcontextprotocol/serverInfo"]?.name !== "keel") {
        throw new Error(`unexpected reconnect discovery result: ${JSON.stringify(result)}`);
      }
        const recalled = await callTool(session, "recall", { query: marker, limit: 1 });
      const { value } = parseToolJson(recalled, "reconnect recall");
      const context = requireBoundedContext(recalled, "reconnect recall");
      if (!Array.isArray(value.matches) || value.matches.length < 1) {
        throw new Error(`reconnect could not retrieve persisted memory: ${JSON.stringify(value)}`);
      }
      return { ...context, matchCount: value.matches.length };
    });
  } catch (error) {
    error.checks = checks;
    throw error;
  } finally {
    if (session) {
      await session.close();
    }
  }
  return { protocolVersion: MCP_PROTOCOL_VERSION, marker, checks };
}

async function main() {
  let options;
  let checks = [];
  let outcome;
  try {
    options = parseArguments(process.argv.slice(2));
    if (!existsSync(options.binary)) {
      throw new Error(`packaged binary does not exist: ${options.binary}`);
    }
    outcome = await runSmoke(options);
    checks = outcome.checks;
  } catch (error) {
    if (error?.stack) {
      console.error(error.stack);
    } else {
      console.error(String(error));
    }
    // Preserve the checks collected before the failing operation whenever the
    // failure came from runSmoke; argument/setup failures have no checks.
    if (error && Array.isArray(error.checks)) {
      checks = error.checks;
    }
    outcome = { protocolVersion: MCP_PROTOCOL_VERSION, checks };
    const reportPath = options?.report;
    if (reportPath) {
      mkdirSync(dirname(reportPath), { recursive: true });
      writeFileSync(reportPath, `${JSON.stringify({ schemaVersion: 1, status: "failed", ...outcome }, null, 2)}\n`);
    }
    process.exitCode = 1;
    return;
  }

  const report = {
    schemaVersion: 1,
    status: "passed",
    binary: options.binary,
    bundleRoot: options.bundleRoot,
    claudeHome: options.claudeHome,
    ...outcome,
  };
  mkdirSync(dirname(options.report), { recursive: true });
  writeFileSync(options.report, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
}

await main();
