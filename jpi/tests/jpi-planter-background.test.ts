import assert from "node:assert/strict";
import test from "node:test";

import { BackgroundTaskMonitor } from "../extensions/jpi-planter/background.ts";
import {
  BACKGROUND_RESPONSE_TIMEOUT_MS,
  isBackgroundTerminal,
  runningBackgroundTasks,
} from "../extensions/jpi-planter/protocol.ts";
import {
  PlanterEventBus,
  PlanterScheduler,
  backgroundRequests,
  backgroundResponse,
} from "./jpi-planter-test-helpers.ts";

test("background responses validate and deduplicate running task ids", () => {
  const parsed = runningBackgroundTasks({
    schema_version: "pi-background-tasks.extension-response.v1",
    request_id: "request",
    operation: "status",
    ok: true,
    result: { tasks: [
      { id: "agent", status: "running", isAgent: false },
      { id: "agent", status: "running", isAgent: true },
      { id: "job", status: "running", isAgent: false },
      { id: "done", status: "completed", isAgent: true },
    ] },
  }, "request");
  assert.deepEqual([...parsed!.values()], [
    { id: "agent", isAgent: true },
    { id: "job", isAgent: false },
  ]);

  for (const malformed of [
    null,
    {},
    { schema_version: "wrong" },
    {
      schema_version: "pi-background-tasks.extension-response.v1",
      request_id: "request",
      operation: "status",
      ok: true,
      result: { tasks: [{ id: "task", status: "running" }] },
    },
  ]) {
    assert.equal(runningBackgroundTasks(malformed, "request"), undefined);
  }
  assert.equal(isBackgroundTerminal({
    schema_version: "pi-background-tasks.extension-terminal.v1",
    task: { id: "done", status: "completed", isAgent: false },
  }), true);
  assert.equal(isBackgroundTerminal({ task: { id: "done" } }), false);
});

test("polling orders responses, bounds pending requests, and refreshes on terminal events", () => {
  const scheduler = new PlanterScheduler();
  const events = new PlanterEventBus();
  const applied: Array<Array<{ id: string; isAgent: boolean }>> = [];
  const monitor = new BackgroundTaskMonitor(
    events,
    scheduler,
    7,
    () => "id",
    (tasks) => applied.push([...tasks.values()]),
  );
  monitor.start();

  assert.deepEqual(backgroundRequests(events)[0], {
    schema_version: "pi-background-tasks.extension-request.v1",
    request_id: "7:1:id",
    operation: "status",
    payload: {},
  });
  const poll = scheduler.active("interval", 1_000)[0];
  scheduler.fire(poll);
  const requests = backgroundRequests(events);
  events.emit("pi-background-tasks:response:v1", backgroundResponse(requests[1], [
    { id: "agent", status: "running", isAgent: true },
    { id: "job", status: "running", isAgent: false },
  ]));
  events.emit("pi-background-tasks:response:v1", backgroundResponse(requests[0], []));
  assert.deepEqual(applied, [[
    { id: "agent", isAgent: true },
    { id: "job", isAgent: false },
  ]]);

  const beforeTerminal = backgroundRequests(events).length;
  events.emit("pi-background-tasks:terminal:v1", { task: { id: "bad" } });
  assert.equal(backgroundRequests(events).length, beforeTerminal);
  events.emit("pi-background-tasks:terminal:v1", {
    schema_version: "pi-background-tasks.extension-terminal.v1",
    task: { id: "done", status: "failed", isAgent: true },
  });
  assert.equal(backgroundRequests(events).length, beforeTerminal + 1);

  for (let index = 0; index < 10; index += 1) scheduler.fire(poll);
  assert.equal(scheduler.active("timeout", BACKGROUND_RESPONSE_TIMEOUT_MS).length, 4);
  monitor.dispose();
  assert.equal(events.unsubscribed, 2);
  assert.ok(scheduler.timers.every((timer) => timer.cleared));
});
