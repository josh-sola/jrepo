import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { PlanterSessionState, planterColor, positiveIntegerText } from "../extensions/jpi-planter/state.ts";
import {
  planterRecordPath,
  planterStateDirectory,
  safeSessionId,
} from "../extensions/jpi-planter/store.ts";
import { planterHarness, readRecord } from "./jpi-planter-test-helpers.ts";

async function temporaryDirectory() {
  return mkdtemp(join(tmpdir(), "jpi-planter-"));
}

test("state paths and environment values use Planter's exact validation", () => {
  assert.equal(planterStateDirectory({
    PLANTER_STATE_DIR: "/new",
    CLAUDE_PLANTER_DIR: "/legacy",
  }, "/home/me"), "/new");
  assert.equal(planterStateDirectory({
    PLANTER_STATE_DIR: "",
    CLAUDE_PLANTER_DIR: "/legacy",
  }, "/home/me"), "/legacy");
  assert.equal(planterStateDirectory({
    PLANTER_STATE_DIR: "",
    CLAUDE_PLANTER_DIR: "",
  }, "/home/me"), "/home/me/.claude/planter");
  assert.equal(safeSessionId("saved/id:名"), "saved_id__");
  assert.equal(planterRecordPath("/state", "saved/id:名", 42), "/state/pi-saved_id__-42.json");
  assert.equal(positiveIntegerText("1"), 1);
  assert.equal(positiveIntegerText("01"), undefined);
  assert.equal(positiveIntegerText("0"), undefined);
  assert.equal(positiveIntegerText("1.5"), undefined);
  assert.equal(planterColor("pink"), "pink");
  assert.equal(planterColor("teal"), null);
});

test("the initial state file has the provider-neutral Pi identity and record", async (t) => {
  const directory = await temporaryDirectory();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const harness = planterHarness(directory, {
    environment: { PLANTER_COLOR: "blue", PLANTER_TAB_INDEX: "3" },
  });
  harness.context.sessionManager.getSessionId = () => "saved/id:名";
  await harness.extension.onSessionStart({}, harness.context);
  const path = harness.extension.recordPath();
  assert.equal(path, join(directory, "pi-saved_id__-4321.json"));
  assert.deepEqual(await readRecord(path!), {
    provider: "pi",
    identity: "pi:saved/id:名:4321",
    session_id: "saved/id:名",
    cwd: "/repo/project",
    label: "project",
    state: "waiting",
    agents: 0,
    turn: 0,
    since: 100,
    agents_at: 0,
    pid: 4321,
    created_at: 100,
    updated_at: 100,
    color: "blue",
    tab: 3,
  });
  await harness.extension.onSessionShutdown({ reason: "quit" }, harness.context);
});

test("working, waiting, and attention keep the correct waiting clock", () => {
  let now = 10;
  const state = new PlanterSessionState({
    sessionId: "session",
    pid: 9,
    cwd: "/repo",
    label: "repo",
    environment: {},
    now: () => now,
  });
  assert.deepEqual(
    { state: state.record().state, turn: state.record().turn, since: state.record().since },
    { state: "waiting", turn: 0, since: 10 },
  );
  now = 20;
  state.setMain(true);
  assert.deepEqual(
    { state: state.record().state, turn: state.record().turn, since: state.record().since },
    { state: "working", turn: 1, since: 0 },
  );
  now = 30;
  state.setAttention(true);
  assert.deepEqual(
    { state: state.record().state, turn: state.record().turn, since: state.record().since },
    { state: "attention", turn: 1, since: 30 },
  );

  now = 40;
  state.setMain(false);
  assert.deepEqual(
    { state: state.record().state, turn: state.record().turn, since: state.record().since },
    { state: "attention", turn: 0, since: 30 },
  );
  now = 50;
  state.setAttention(false);
  assert.deepEqual(
    { state: state.record().state, turn: state.record().turn, since: state.record().since },
    { state: "waiting", turn: 0, since: 30 },
  );
  now = 60;
  state.setMain(true);
  now = 70;
  state.setMain(false);
  assert.equal(state.record().since, 70);
});

test("subagents and background tasks use union and bud-set semantics", () => {
  let now = 100;
  const state = new PlanterSessionState({
    sessionId: "session", pid: 9, cwd: "/repo", label: "repo",
    environment: {}, now: () => now,
  });
  state.startSubagent("same");
  const first = state.record();
  assert.deepEqual(
    { state: first.state, agents: first.agents, agentsAt: first.agents_at },
    { state: "working", agents: 1, agentsAt: 100 },
  );
  now = 110;
  assert.equal(state.startSubagent("same"), false);
  assert.equal(state.record().agents_at, 100);
  state.setBackground(new Map([
    ["same", { id: "same", isAgent: true }],
    ["build", { id: "build", isAgent: false }],
  ]));
  assert.deepEqual(
    { state: state.record().state, agents: state.record().agents, agentsAt: state.record().agents_at },
    { state: "working", agents: 2, agentsAt: 110 },
  );
  now = 120;
  state.finishSubagent("same");
  assert.equal(state.record().agents, 1);
  state.setBackground(new Map([["build", { id: "build", isAgent: false }]]));
  assert.deepEqual(
    { state: state.record().state, agents: state.record().agents },
    { state: "working", agents: 0 },
  );
  now = 130;
  state.setBackground(new Map());
  assert.deepEqual(
    { state: state.record().state, since: state.record().since },
    { state: "waiting", since: 130 },
  );
});
