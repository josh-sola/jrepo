import {
  BACKGROUND_POLL_INTERVAL_MS,
  BACKGROUND_REQUEST_CHANNEL,
  BACKGROUND_REQUEST_SCHEMA,
  BACKGROUND_RESPONSE_CHANNEL,
  BACKGROUND_RESPONSE_TIMEOUT_MS,
  BACKGROUND_TERMINAL_CHANNEL,
  MAX_PENDING_BACKGROUND_REQUESTS,
  isBackgroundTerminal,
  runningBackgroundTasks,
  type EventBus,
  type RunningBackgroundTask,
  type Scheduler,
} from "./protocol.ts";

type PendingRequest = { sequence: number; timeout: unknown };

export class BackgroundTaskMonitor {
  private pending = new Map<string, PendingRequest>();
  private unsubscribers: Array<() => void> = [];
  private pollTimer?: unknown;
  private sequence = 0;
  private appliedSequence = 0;
  private disposed = false;
  private readonly events: EventBus;
  private readonly scheduler: Scheduler;
  private readonly generation: number;
  private readonly createRequestId: () => string;
  private readonly apply: (tasks: Map<string, RunningBackgroundTask>) => void;

  constructor(
    events: EventBus,
    scheduler: Scheduler,
    generation: number,
    createRequestId: () => string,
    apply: (tasks: Map<string, RunningBackgroundTask>) => void,
  ) {
    this.events = events;
    this.scheduler = scheduler;
    this.generation = generation;
    this.createRequestId = createRequestId;
    this.apply = apply;
  }

  start(): void {
    if (this.disposed) return;
    this.unsubscribers = [
      this.events.on(BACKGROUND_RESPONSE_CHANNEL, (data) => this.respond(data)),
      this.events.on(BACKGROUND_TERMINAL_CHANNEL, (data) => {
        if (isBackgroundTerminal(data)) this.request();
      }),
    ];
    this.request();
    this.pollTimer = this.scheduler.setInterval(
      () => this.request(),
      BACKGROUND_POLL_INTERVAL_MS,
    );
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const unsubscribe of this.unsubscribers) {
      try { unsubscribe(); } catch {}
    }
    this.unsubscribers = [];
    if (this.pollTimer !== undefined) this.scheduler.clearInterval(this.pollTimer);
    this.pollTimer = undefined;
    for (const request of this.pending.values()) this.scheduler.clearTimeout(request.timeout);
    this.pending.clear();
  }

  private request(): void {
    if (this.disposed) return;
    while (this.pending.size >= MAX_PENDING_BACKGROUND_REQUESTS) {
      const oldest = this.pending.entries().next().value as [string, PendingRequest] | undefined;
      if (!oldest) break;
      this.scheduler.clearTimeout(oldest[1].timeout);
      this.pending.delete(oldest[0]);
    }

    const sequence = ++this.sequence;
    const suffix = this.createRequestId().slice(0, 120);
    const requestId = `${this.generation}:${sequence}:${suffix}`;
    const timeout = this.scheduler.setTimeout(
      () => this.pending.delete(requestId),
      BACKGROUND_RESPONSE_TIMEOUT_MS,
    );
    this.pending.set(requestId, { sequence, timeout });
    try {
      this.events.emit(BACKGROUND_REQUEST_CHANNEL, {
        schema_version: BACKGROUND_REQUEST_SCHEMA,
        request_id: requestId,
        operation: "status",
        payload: {},
      });
    } catch {
      this.pending.delete(requestId);
      this.scheduler.clearTimeout(timeout);
    }
  }

  private respond(data: unknown): void {
    for (const [requestId, pending] of this.pending) {
      const tasks = runningBackgroundTasks(data, requestId);
      if (tasks === undefined) continue;
      this.pending.delete(requestId);
      this.scheduler.clearTimeout(pending.timeout);
      if (pending.sequence > this.appliedSequence) {
        this.appliedSequence = pending.sequence;
        this.apply(tasks);
      }
      return;
    }
  }
}
