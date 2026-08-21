import assert from "node:assert/strict";
import test from "node:test";

import { createTitleExtension } from "../extensions/jpi-title/extension.ts";
import { FakeEventBus, ManualScheduler, ok, statusResponse } from "./jpi-title-test-helpers.ts";

const REQUEST_CHANNEL = "pi-background-tasks:request:v1";
const RESPONSE_CHANNEL = "pi-background-tasks:response:v1";
const TERMINAL_CHANNEL = "pi-background-tasks:terminal:v1";

function requests(events: FakeEventBus) {
  return events.emitted
    .filter(({ channel }) => channel === REQUEST_CHANNEL)
    .map(({ data }) => data as { request_id: string; [key: string]: unknown });
}

test("background status polling and terminal refresh use the documented protocol", async () => {
  const scheduler = new ManualScheduler();
  const events = new FakeEventBus();
  const titles: string[] = [];
  const context = {
    mode: "tui",
    cwd: "/repo/project",
    ui: { setTitle: (title: string) => titles.push(title) },
  };
  const extension = createTitleExtension({
    exec: async () => ok("tree"),
    events,
    getSessionName: () => undefined,
    scheduler,
    requestId: () => "unique",
  });
  await extension.onSessionStart({}, context);

  assert.equal(requests(events).length, 1);
  assert.deepEqual(requests(events)[0], {
    schema_version: "pi-background-tasks.extension-request.v1",
    request_id: "1:1:unique",
    operation: "status",
    payload: {},
  });
  events.emit(RESPONSE_CHANNEL, statusResponse(requests(events)[0], ["running", "completed"]));
  assert.equal(titles.at(-1), "⠋ tree");

  extension.onAgentStart({}, context);
  events.emit(TERMINAL_CHANNEL, { task: { id: "done" } });
  assert.equal(requests(events).length, 2);
  events.emit(RESPONSE_CHANNEL, statusResponse(requests(events)[1], ["completed"]));
  assert.notEqual(titles.at(-1)?.startsWith("⏹"), true);
  extension.onAgentSettled({}, context);
  assert.equal(titles.at(-1), "⏹ tree");

  const poll = scheduler.active("interval", 1_000)[0];
  scheduler.fire(poll);
  assert.equal(requests(events).length, 3);
  events.emit(RESPONSE_CHANNEL, statusResponse({ request_id: "not-matching" }, ["running"]));
  assert.equal(titles.at(-1), "⏹ tree");
});

test("shutdown clears session listeners and every timer while ignoring stale lookup", async () => {
  const scheduler = new ManualScheduler();
  const events = new FakeEventBus();
  const titles: string[] = [];
  let resolveLookup: (value: ReturnType<typeof ok>) => void = () => {};
  let lookupSignal: AbortSignal | undefined;
  const lookup = new Promise<ReturnType<typeof ok>>((resolve) => { resolveLookup = resolve; });
  const context = {
    mode: "tui",
    cwd: "/repo/project",
    ui: { setTitle: (title: string) => titles.push(title) },
  };
  const extension = createTitleExtension({
    exec: async (_command, _args, options) => {
      lookupSignal = options.signal;
      return lookup;
    },
    events,
    getSessionName: () => undefined,
    scheduler,
    requestId: () => "pending",
  });

  const starting = extension.onSessionStart({}, context);
  extension.onAgentStart({}, context);
  extension.onSessionShutdown({}, context);
  assert.equal(titles.at(-1), "⏹ project");
  assert.equal(lookupSignal?.aborted, true);
  assert.equal(events.unsubscribed, 5);
  assert.ok(scheduler.timers.every((timer) => timer.cleared));

  const titleCount = titles.length;
  resolveLookup(ok("stale tree"));
  await starting;
  events.emit("subagents:started", { id: "stale" });
  assert.equal(titles.length, titleCount);
  assert.ok(scheduler.timers.every((timer) => timer.cleared));
});
