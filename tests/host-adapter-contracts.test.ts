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
