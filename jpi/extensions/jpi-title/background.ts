import {
  BACKGROUND_POLL_INTERVAL_MS,
  BACKGROUND_REQUEST_CHANNEL,
  BACKGROUND_REQUEST_SCHEMA,
  BACKGROUND_RESPONSE_CHANNEL,
  BACKGROUND_RESPONSE_TIMEOUT_MS,
  BACKGROUND_TERMINAL_CHANNEL,
  runningStatusResponse,
  type EventBus,
  type Scheduler,
} from "./protocol.ts";

type Pending = { sequence: number; timeout: unknown };

export class BackgroundActivityMonitor {
  private pending = new Map<string, Pending>();
  private unsubscribers: Array<() => void> = [];
  private pollTimer?: unknown;
  private sequence = 0;
  private applied = 0;
  private disposed = false;
  private events: EventBus;
  private scheduler: Scheduler;
  private generation: number;
  private createId: () => string;
  private setActive: (active: boolean) => void;

  constructor(
    events: EventBus,
    scheduler: Scheduler,
    generation: number,
    createId: () => string,
    setActive: (active: boolean) => void,
  ) {
    this.events = events;
    this.scheduler = scheduler;
    this.generation = generation;
    this.createId = createId;
    this.setActive = setActive;
  }

  start(): void {
    this.unsubscribers = [
      this.events.on(BACKGROUND_RESPONSE_CHANNEL, (data) => this.respond(data)),
      this.events.on(BACKGROUND_TERMINAL_CHANNEL, () => this.request()),
    ];
    // Polling is required because the public protocol has no task-start broadcast.
    this.request();
    this.pollTimer = this.scheduler.setInterval(
      () => this.request(),
      BACKGROUND_POLL_INTERVAL_MS,
    );
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const unsubscribe of this.unsubscribers) unsubscribe();
    if (this.pollTimer !== undefined) this.scheduler.clearInterval(this.pollTimer);
    for (const value of this.pending.values()) this.scheduler.clearTimeout(value.timeout);
    this.pending.clear();
  }

  private request(): void {
    if (this.disposed) return;
    const sequence = ++this.sequence;
    const requestId = `${this.generation}:${sequence}:${this.createId()}`;
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
      const active = runningStatusResponse(data, requestId);
      if (active === undefined) continue;
      this.pending.delete(requestId);
      this.scheduler.clearTimeout(pending.timeout);
      if (pending.sequence >= this.applied) {
        this.applied = pending.sequence;
        this.setActive(active);
      }
      return;
    }
  }
}
