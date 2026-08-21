import { readFile } from "node:fs/promises";

import { createPlanterExtension } from "../extensions/jpi-planter/extension.ts";

export class PlanterScheduler {
  timers: Array<{
    kind: "interval" | "timeout";
    callback: () => void;
    delay: number;
    cleared: boolean;
  }> = [];

  setInterval(callback: () => void, delay: number) {
    return this.add("interval", callback, delay);
  }
  clearInterval(timer: unknown) {
    (timer as { cleared: boolean }).cleared = true;
  }
  setTimeout(callback: () => void, delay: number) {
    return this.add("timeout", callback, delay);
  }
  clearTimeout(timer: unknown) {
    (timer as { cleared: boolean }).cleared = true;
  }
  active(kind: "interval" | "timeout", delay: number) {
    return this.timers.filter((timer) => (
      timer.kind === kind && timer.delay === delay && !timer.cleared
    ));
  }
  fire(timer: { kind: string; callback: () => void; cleared: boolean }) {
    if (timer.cleared) return;
    if (timer.kind === "timeout") timer.cleared = true;
    timer.callback();
  }
  private add(kind: "interval" | "timeout", callback: () => void, delay: number) {
    const timer = { kind, callback, delay, cleared: false };
    this.timers.push(timer);
    return timer;
  }
}

export class PlanterEventBus {
  handlers = new Map<string, Set<(data: unknown) => void>>();
  emitted: Array<{ channel: string; data: unknown }> = [];
  unsubscribed = 0;
  on(channel: string, handler: (data: unknown) => void) {
    const handlers = this.handlers.get(channel) ?? new Set();
    handlers.add(handler);
    this.handlers.set(channel, handlers);
    return () => {
      if (handlers.delete(handler)) this.unsubscribed += 1;
    };
  }
  emit(channel: string, data: unknown) {
    this.emitted.push({ channel, data });
    for (const handler of this.handlers.get(channel) ?? []) handler(data);
  }
}

export function backgroundRequests(events: PlanterEventBus) {
  return events.emitted
    .filter(({ channel }) => channel === "pi-background-tasks:request:v1")
    .map(({ data }) => data as { request_id: string; [key: string]: unknown });
}

export function backgroundResponse(
  request: { request_id: string },
  tasks: Array<{ id: string; status: string; isAgent: boolean }>,
) {
  return {
    schema_version: "pi-background-tasks.extension-response.v1",
    request_id: request.request_id,
    operation: "status",
    ok: true,
    result: { tasks },
  };
}

export async function readRecord(path: string) {
  return JSON.parse(await readFile(path, "utf8")) as Record<string, unknown>;
}

export function planterHarness(directory: string, overrides: Record<string, unknown> = {}) {
  const scheduler = new PlanterScheduler();
  const events = new PlanterEventBus();
  const environment: Record<string, string | undefined> = {
    PLANTER_STATE_DIR: directory,
    ...overrides.environment as Record<string, string | undefined> | undefined,
  };
  let now = 100;
  let sessionName = overrides.sessionName as string | undefined;
  let temp = 0;
  const extension = createPlanterExtension({
    exec: overrides.exec as never ?? (async () => { throw new Error("missing hook"); }),
    events,
    getSessionName: () => sessionName,
    scheduler,
    requestId: () => "request",
    tempId: () => `temp-${++temp}`,
    environment,
    pid: 4321,
    home: "/home/test",
    now: () => now,
  });
  const context = {
    mode: "tui",
    cwd: "/repo/project",
    sessionManager: { getSessionId: () => "saved-session" },
  };
  return {
    scheduler, events, environment, extension, context,
    setNow(value: number) { now = value; },
    setSessionName(value: string | undefined) { sessionName = value; },
  };
}
