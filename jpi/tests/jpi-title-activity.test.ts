import assert from "node:assert/strict";
import test from "node:test";

import { createTitleExtension } from "../extensions/jpi-title/extension.ts";
import { ACTIVE_FRAMES } from "../extensions/jpi-title/helpers.ts";
import { FakeEventBus, ManualScheduler, ok } from "./jpi-title-test-helpers.ts";

function harness() {
  const scheduler = new ManualScheduler();
  const events = new FakeEventBus();
  const titles: string[] = [];
  const context = {
    mode: "tui",
    cwd: "/repo/project",
    ui: { setTitle: (title: string) => titles.push(title) },
  };
  const extension = createTitleExtension({
    exec: async () => ({ ...ok(), code: 1 }),
    events,
    getSessionName: () => undefined,
    scheduler,
    requestId: () => "id",
  });
  return { scheduler, events, titles, context, extension };
}

test("activity starts at the first frame and advances in the exact 80 ms order", async () => {
  const { scheduler, titles, context, extension } = harness();
  await extension.onSessionStart({}, context);
  scheduler.fire(scheduler.active("timeout", 0)[0]);
  titles.length = 0;

  extension.onAgentStart({}, context);
  const spinner = scheduler.active("interval", 80)[0];
  assert.ok(spinner);
  assert.equal(titles[0], `${ACTIVE_FRAMES[0]} project`);
  for (let index = 1; index < ACTIVE_FRAMES.length; index += 1) scheduler.fire(spinner);
  assert.deepEqual(titles, ACTIVE_FRAMES.map((frame) => `${frame} project`));

  scheduler.fire(spinner);
  assert.equal(titles.at(-1), `${ACTIVE_FRAMES[0]} project`);
  extension.onAgentSettled({}, context);
  assert.equal(titles.at(-1), "⏹ project");
  assert.equal(spinner.cleared, true);
});

test("main, background agents, and multiple subagents use union semantics", async () => {
  const { scheduler, events, titles, context, extension } = harness();
  await extension.onSessionStart({}, context);
  scheduler.fire(scheduler.active("timeout", 0)[0]);

  extension.onAgentStart({}, context);
  const spinner = scheduler.active("interval", 80)[0];
  events.emit("subagents:started", { id: "one" });
  events.emit("subagents:started", { id: "two" });
  extension.onAgentSettled({}, context);
  assert.equal(spinner.cleared, false);

  events.emit("subagents:completed", { id: "one" });
  assert.equal(spinner.cleared, false);
  assert.notEqual(titles.at(-1)?.startsWith("⏹"), true);
  events.emit("subagents:failed", { id: "two" });
  assert.equal(spinner.cleared, true);
  assert.equal(titles.at(-1), "⏹ project");
});
