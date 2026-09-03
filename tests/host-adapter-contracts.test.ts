import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Fixture = Record<string, unknown>;

const repoRoot = join(import.meta.dir, "..");
const fixtures = JSON.parse(
  readFileSync(join(import.meta.dir, "fixtures", "host-adapters", "contracts.json"), "utf8"),
) as Record<string, Fixture[]>;

function source(relativePath: string): string {
  return readFileSync(join(repoRoot, relativePath), "utf8");
}

function assertFields(value: Fixture | undefined, fields: string[], label: string): void {
  expect(value, `${label} fixture is missing`).toBeDefined();
  for (const field of fields) {
    expect(value, `${label} fixture is missing ${field}`).toHaveProperty(field);
  }
}

test("Codex fixtures use the official hook payload and output fields", () => {
  const codex = source("codex/keel-codex.ts");
  for (const fixture of fixtures.codex ?? []) {
    assertFields(fixture.input as Fixture | undefined, fixture.required_fields as string[], `codex ${fixture.name}`);
  }
  expect(codex).toContain("hook_event_name");
  expect(codex).toContain("tool_name");
  expect(codex).toContain("tool_response");
  expect(codex).toContain("source");
  expect(codex).toContain("hookSpecificOutput");
  expect(codex).toContain("permissionDecision");
  expect(codex).toContain('"--phase", "post"');
  expect(codex).toContain('"--phase", "pre"');
});

test("Codex hooks execute the bundled Node runtime without npx", () => {
  const manifest = JSON.parse(source("codex/hooks/hooks.json")) as {
    hooks: Record<string, Array<{ hooks?: Array<{ command?: string }> }>>;
  };
  const commands = Object.values(manifest.hooks).flatMap((entries) =>
    entries.flatMap((entry) => (entry.hooks ?? []).map((hook) => hook.command ?? "")),
  );
  expect(commands.length).toBeGreaterThan(0);
  expect(commands.every((command) => command === 'node "${PLUGIN_ROOT}/keel-codex.js"')).toBe(true);
  expect(source("codex/keel-codex.js")).toContain("resolveBinary");
});

test("Codex bundled adapter handles PreToolUse end to end", async () => {
  const tempHome = await mkdtemp(join(tmpdir(), "keel-codex-e2e-"));
  const child = Bun.spawn(["node", "codex/keel-codex.js"], {
    cwd: repoRoot,
    env: {
      ...process.env,
      KEEL_HOME: join(repoRoot, "target", "debug"),
      HOME: tempHome,
      USERPROFILE: tempHome,
      CLAUDE_TARGET_OVERRIDE: join(tempHome, ".claude"),
    },
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
  });
  try {
    child.stdin.write(
      `${JSON.stringify({
        hook_event_name: "PreToolUse",
        session_id: "codex-contract-e2e",
        cwd: repoRoot,
        tool_name: "Edit",
        tool_input: { file_path: join(repoRoot, "README.md") },
      })}\n`,
    );
    child.stdin.end();
    const output = await new Response(child.stdout).text();
    const exitCode = await child.exited;
    expect(exitCode).toBe(0);
    expect(output).toContain("permissionDecision");
  } finally {
    await rm(tempHome, { recursive: true, force: true });
  }
});

test("Cursor fixtures match the shipped hooks JSON parser", () => {
  const cursor = source("cursor/hooks/keel-cursor.sh");
  for (const fixture of fixtures.cursor ?? []) {
    assertFields(fixture.input as Fixture | undefined, fixture.required_fields as string[], `cursor ${fixture.name}`);
  }
  expect(cursor).toContain(".hook_event_name");
  expect(cursor).toContain(".conversation_id");
  expect(cursor).toContain(".tool_name");
  expect(cursor).toContain(".tool_input.command");
  expect(cursor).toContain("postToolUse)");
  expect(cursor).toContain("sessionEnd)");
  expect(cursor).toContain("--phase post");
});

test("OpenCode fixtures match named plugin hook contracts", () => {
  const opencode = source("opencode/keel.ts");
  for (const fixture of fixtures.opencode ?? []) {
    if (fixture.input) {
      assertFields(
        fixture.input as Fixture,
        fixture.required_input_fields as string[],
        `opencode ${fixture.name} input`,
      );
    }
    if (fixture.output) {
      assertFields(
        fixture.output as Fixture,
        fixture.required_output_fields as string[],
        `opencode ${fixture.name} output`,
      );
    }
    if (fixture.event) {
      assertFields(
        fixture.event as Fixture,
        fixture.required_event_fields as string[],
        `opencode ${fixture.name} event`,
      );
    }
  }
  expect(opencode).toContain('"chat.message"');
  expect(opencode).toContain('"tool.execute.before"');
  expect(opencode).toContain('"tool.execute.after"');
  expect(opencode).toContain('"session.deleted"');
  expect(opencode).toContain("input.sessionID");
  expect(opencode).toContain("input.tool");
  expect(opencode).toContain("output.metadata");
});

test("Command Code fixtures match the ModApi lifecycle surface", () => {
  const commandCode = source("commandcode/keel-cmdc.ts");
  for (const fixture of fixtures.commandcode ?? []) {
    assertFields(fixture.event as Fixture | undefined, fixture.required_event_fields as string[], `commandcode ${fixture.name}`);
  }
  expect(commandCode).toContain("onSessionStart");
  expect(commandCode).toContain("beforeToolCall");
  expect(commandCode).toContain("afterToolCall");
  expect(commandCode).toContain("onSessionEnd");
  expect(commandCode).toContain('cmd.on("compaction_start"');
  expect(commandCode).toContain('cmd.on("compaction_done"');
});

test("Antigravity adapter translates the documented camelCase hook contract", () => {
  const adapter = source("antigravity/keel-antigravity.js");
  expect(fixtures.antigravity?.length).toBeGreaterThan(0);
  for (const fixture of fixtures.antigravity ?? []) {
    assertFields(
      fixture.input as Fixture | undefined,
      fixture.required_fields as string[],
      `antigravity ${fixture.name}`,
    );
  }
  expect(adapter).toContain("toolCall");
  expect(adapter).toContain("conversationId");
  expect(adapter).toContain("workspacePaths");
  expect(adapter).toContain('decision: "deny"');
  expect(adapter).toContain('decision: "allow"');
  expect(adapter).toContain('startsWith("KEEL_GATE_DENY")');
  expect(adapter).toContain('startsWith("KEEL_GATE_ALLOW")');
  expect(adapter).toContain("injectSteps");
  expect(adapter).toContain('["hook", "stop"]');
  expect(adapter).toContain('decision: "continue"');
  expect(adapter).toContain('"bridge", subcommand');
});
