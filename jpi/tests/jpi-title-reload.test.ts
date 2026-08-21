import assert from "node:assert/strict";
import test from "node:test";

import { createTitleExtension } from "../extensions/jpi-title/extension.ts";
import { FakeEventBus, ManualScheduler, ok } from "./jpi-title-test-helpers.ts";

test("repeated session starts ignore stale worktree results and callbacks", async () => {
  const scheduler = new ManualScheduler();
  const events = new FakeEventBus();
  const titles: string[] = [];
  let resolveFirst: (value: ReturnType<typeof ok>) => void = () => {};
  const firstLookup = new Promise<ReturnType<typeof ok>>((resolve) => { resolveFirst = resolve; });
  let calls = 0;
  const extension = createTitleExtension({
    exec: async () => {
      calls += 1;
      return calls === 1 ? firstLookup : ok("current tree");
    },
    events,
    getSessionName: () => undefined,
    scheduler,
    requestId: () => "reload",
  });
  const context = {
    mode: "tui",
    cwd: "/repo/project",
    ui: { setTitle: (title: string) => titles.push(title) },
  };

  const oldStart = extension.onSessionStart({}, context);
  const currentStart = extension.onSessionStart({}, context);
  await currentStart;
  const startup = scheduler.active("timeout", 0);
  assert.equal(startup.length, 1);
  scheduler.fire(startup[0]);
  assert.equal(titles.at(-1), "⏹ current tree");

  const titleCount = titles.length;
  resolveFirst(ok("stale tree"));
  await oldStart;
  assert.equal(scheduler.active("timeout", 0).length, 0);
  assert.equal(titles.length, titleCount);
  assert.equal(events.unsubscribed, 5);
});
