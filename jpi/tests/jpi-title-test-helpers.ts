export class ManualScheduler {
  timers: Array<{ kind: "interval" | "timeout"; callback: () => void; delay: number; cleared: boolean }> = [];

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

  fire(timer: { callback: () => void; kind: string; cleared: boolean }) {
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

export class FakeEventBus {
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

export function ok(stdout = "") {
  return { stdout, stderr: "", code: 0, killed: false };
}

export function statusResponse(request: { request_id: string }, statuses: string[]) {
  return {
    schema_version: "pi-background-tasks.extension-response.v1",
    request_id: request.request_id,
    operation: "status",
    ok: true,
    result: { tasks: statuses.map((status, id) => ({ id: String(id), status })) },
  };
}
