import assert from "node:assert/strict";
import { access, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import jpiPlanter from "../extensions/jpi-planter/index.ts";
import { LABEL_HOOK_TIMEOUT_MS, resolvePlanterLabel } from "../extensions/jpi-planter/helpers.ts";
import { planterHarness, readRecord } from "./jpi-planter-test-helpers.ts";

async function temporaryDirectory() {
  return mkdtemp(join(tmpdir(), "jpi-planter-reload-"));
}

async function replaceRecord(path: string, patch: Record<string, unknown>) {
  const record = await readRecord(path);
  await writeFile(path, `${JSON.stringify({ ...record, ...patch })}\n`);
}

test("atomic writes adopt live tabs, reload preserves ordering, and final shutdown cleans up", async (t) => {
  const directory = await temporaryDirectory();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const harness = planterHarness(directory, {
    environment: {
      PLANTER_TAB_INDEX: "3",
      PLANTER_COLOR: "not-a-color",
    },
  });
  await harness.extension.onSessionStart({}, harness.context);
  const path = harness.extension.recordPath()!;
  let record = await readRecord(path);
  assert.deepEqual(
    { created: record.created_at, color: record.color, tab: record.tab },
    { created: 100, color: null, tab: 3 },
  );

  await replaceRecord(path, { tab: 9 });
  harness.setNow(110);
  await harness.extension.onAgentStart({}, harness.context);
  assert.equal((await readRecord(path)).tab, 9);

  await replaceRecord(path, { tab: 0 });
  harness.setNow(120);
  await harness.extension.onAgentSettled({}, harness.context);
  assert.equal((await readRecord(path)).tab, 3);
  assert.deepEqual(await readdir(directory), ["pi-saved-session-4321.json"]);

  await replaceRecord(path, { tab: 7 });
  await harness.extension.onSessionShutdown({ reason: "reload" }, harness.context);
  await access(path);
  assert.equal(harness.events.unsubscribed, 6);
  assert.ok(harness.scheduler.timers.every((timer) => timer.cleared));

  harness.setNow(200);
  await harness.extension.onSessionStart({ reason: "reload" }, harness.context);
  record = await readRecord(path);
  assert.deepEqual(
    {
      state: record.state,
      created: record.created_at,
      since: record.since,
      tab: record.tab,
    },
    { state: "waiting", created: 100, since: 120, tab: 7 },
  );
  await harness.extension.onSessionShutdown({ reason: "quit" }, harness.context);
  await assert.rejects(access(path));
});

test("labels follow environment, session name, direct hook, and cwd precedence", async (t) => {
  const directory = await temporaryDirectory();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const calls: Array<{ command: string; args: string[]; options: Record<string, unknown> }> = [];
  const harness = planterHarness(directory, {
    exec: async (command: string, args: string[], options: Record<string, unknown>) => {
      calls.push({ command, args, options });
      return { code: 0, stdout: calls.length === 1 ? " Hook\tlabel\nignored" : "Again" };
    },
  });
  await harness.extension.onSessionStart({}, harness.context);
  const path = harness.extension.recordPath()!;
  assert.equal((await readRecord(path)).label, "Hook label");
  assert.equal(calls[0].command, join(directory, "label-hook"));
  assert.deepEqual(calls[0].args, ["/repo/project"]);
  assert.equal(calls[0].options.cwd, "/repo/project");
  assert.equal(calls[0].options.timeout, LABEL_HOOK_TIMEOUT_MS);
  assert.ok(calls[0].options.signal instanceof AbortSignal);

  harness.setSessionName("Renamed\nignored");
  await harness.extension.onSessionInfoChanged({}, harness.context);
  assert.equal((await readRecord(path)).label, "Renamed");
  assert.equal(calls.length, 1);

  harness.setSessionName(undefined);
  await harness.extension.onSessionInfoChanged({}, harness.context);
  assert.equal((await readRecord(path)).label, "Again");
  assert.equal(calls.length, 2);
  await harness.extension.onSessionShutdown({ reason: "quit" }, harness.context);

  let envHookCalls = 0;
  assert.equal(await resolvePlanterLabel({
    environment: { PLANTER_LABEL: " Env label\nignored" },
    getSessionName: () => "Session name",
    exec: async () => { envHookCalls += 1; throw new Error("must not run"); },
    stateDirectory: directory,
    cwd: "/repo/fallback",
  }), "Env label");
  assert.equal(envHookCalls, 0);
  assert.equal(await resolvePlanterLabel({
    environment: {},
    getSessionName: () => undefined,
    exec: async () => { throw new Error("missing"); },
    stateDirectory: directory,
    cwd: "/repo/fallback",
  }), "fallback");
  assert.equal(await resolvePlanterLabel({
    environment: { PLANTER_LABEL: "\n" },
    getSessionName: () => "\t",
    exec: async () => { throw new Error("missing"); },
    stateDirectory: directory,
    cwd: "/repo/fallback",
  }), "fallback");
});

test("the directory entry point registers settled lifecycle handling", () => {
  const handlers = new Map<string, unknown>();
  const pi = {
    exec: async () => ({ code: 0 }),
    events: { on: () => () => {}, emit: () => {} },
    getSessionName: () => undefined,
    on: (name: string, handler: unknown) => handlers.set(name, handler),
  };
  jpiPlanter(pi as never);
  assert.deepEqual([...handlers.keys()], [
    "session_start",
    "session_info_changed",
    "agent_start",
    "agent_settled",
    "session_shutdown",
  ]);
  assert.equal(handlers.has("agent_end"), false);
});
