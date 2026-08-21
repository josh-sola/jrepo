import assert from "node:assert/strict";
import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { SUBAGENT_STALE_MS } from "../extensions/jpi-planter/protocol.ts";
import {
  backgroundRequests,
  backgroundResponse,
  planterHarness,
  readRecord,
} from "./jpi-planter-test-helpers.ts";

async function temporaryDirectory() {
  return mkdtemp(join(tmpdir(), "jpi-planter-life-"));
}

test("main, attention, subagent, and background activity use union precedence", async (t) => {
  const directory = await temporaryDirectory();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const harness = planterHarness(directory);
  await harness.extension.onSessionStart({}, harness.context);
  const path = harness.extension.recordPath()!;

  harness.setNow(110);
  await harness.extension.onAgentStart({}, harness.context);
  harness.events.emit("subagents:started", { id: "sub" });
  harness.events.emit("subagents:started", { id: "sub" });
  const firstRequest = backgroundRequests(harness.events)[0];
  harness.events.emit("pi-background-tasks:response:v1", backgroundResponse(firstRequest, [
    { id: "agent", status: "running", isAgent: true },
    { id: "build", status: "running", isAgent: false },
  ]));
  await harness.extension.flush();
  assert.deepEqual(
    (({ state, agents, turn, since, agents_at }) => ({ state, agents, turn, since, agents_at }))(
      await readRecord(path),
    ),
    { state: "working", agents: 2, turn: 1, since: 0, agents_at: 110 },
  );

  await harness.extension.onAgentSettled({}, harness.context);
  harness.setNow(120);
  harness.events.emit("rpiv:ask-user:blocked", { active: true });
  await harness.extension.flush();
  let record = await readRecord(path);
  assert.deepEqual(
    { state: record.state, turn: record.turn, since: record.since },
    { state: "attention", turn: 0, since: 120 },
  );
  harness.events.emit("rpiv:ask-user:blocked", { active: false });
  harness.events.emit("subagents:completed", { id: "sub" });
  await harness.extension.flush();
  record = await readRecord(path);
  assert.deepEqual(
    { state: record.state, agents: record.agents, since: record.since },
    { state: "working", agents: 1, since: 0 },
  );

  harness.setNow(130);
  const poll = harness.scheduler.active("interval", 1_000)[0];
  harness.scheduler.fire(poll);
  const refresh = backgroundRequests(harness.events).at(-1)!;
  harness.events.emit("pi-background-tasks:response:v1", backgroundResponse(refresh, []));
  await harness.extension.flush();
  record = await readRecord(path);
  assert.deepEqual(
    { state: record.state, agents: record.agents, since: record.since },
    { state: "waiting", agents: 0, since: 130 },
  );

  const unchanged = JSON.stringify(record);
  harness.events.emit("rpiv:ask-user:blocked", { active: "yes" });
  harness.events.emit("subagents:started", { id: "" });
  harness.events.emit("subagents:failed", null);
  harness.scheduler.fire(poll);
  const malformedRequest = backgroundRequests(harness.events).at(-1)!;
  harness.events.emit("pi-background-tasks:response:v1", {
    ...backgroundResponse(malformedRequest, []),
    result: { tasks: [{ id: "bad", status: "running" }] },
  });
  await harness.extension.flush();
  assert.equal(JSON.stringify(await readRecord(path)), unchanged);

  harness.setNow(140);
  harness.events.emit("subagents:started", { id: "stale" });
  await harness.extension.flush();
  harness.setNow(200);
  const staleTimer = harness.scheduler.active("timeout", SUBAGENT_STALE_MS)[0];
  harness.scheduler.fire(staleTimer);
  await harness.extension.flush();
  record = await readRecord(path);
  assert.deepEqual(
    { state: record.state, agents: record.agents, since: record.since, agentsAt: record.agents_at },
    { state: "waiting", agents: 0, since: 200, agentsAt: 200 },
  );

  await harness.extension.onSessionShutdown({ reason: "quit" }, harness.context);
  await assert.rejects(access(path));
  assert.equal(harness.events.unsubscribed, 6);
  assert.ok(harness.scheduler.timers.every((timer) => timer.cleared));
});

test("non-TUI sessions do not read session identity or install behavior", async (t) => {
  const directory = await temporaryDirectory();
  t.after(() => rm(directory, { recursive: true, force: true }));
  let execCalls = 0;
  const harness = planterHarness(directory, {
    exec: async () => { execCalls += 1; return { code: 0, stdout: "label" }; },
  });
  const context = {
    ...harness.context,
    mode: "json",
    sessionManager: { getSessionId: () => { throw new Error("must not run"); } },
  };
  await harness.extension.onSessionStart({}, context);
  await harness.extension.onAgentStart({}, context);
  await harness.extension.onAgentSettled({}, context);
  await harness.extension.onSessionInfoChanged({}, context);
  await harness.extension.onSessionShutdown({ reason: "quit" }, context);
  assert.equal(execCalls, 0);
  assert.equal(harness.extension.recordPath(), undefined);
  assert.equal(harness.events.handlers.size, 0);
  assert.equal(harness.scheduler.timers.length, 0);
});
