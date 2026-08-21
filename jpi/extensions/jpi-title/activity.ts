import { ACTIVE_FRAMES, IDLE_INDICATOR, SPINNER_INTERVAL_MS } from "./helpers.ts";
import type { Scheduler } from "./protocol.ts";

export class ActivityTitle {
  private main = false;
  private background = false;
  private subagents = new Set<string>();
  private frame = 0;
  private timer?: unknown;
  private disposed = false;
  private scheduler: Scheduler;
  private getName: () => string;
  private setTitle: (title: string) => void;

  constructor(
    scheduler: Scheduler,
    getName: () => string,
    setTitle: (title: string) => void,
  ) {
    this.scheduler = scheduler;
    this.getName = getName;
    this.setTitle = setTitle;
  }

  setMain(active: boolean): void {
    this.change(() => { this.main = active; });
  }

  setBackground(active: boolean): void {
    this.change(() => { this.background = active; });
  }

  startSubagent(id: string): void {
    this.change(() => { this.subagents.add(id); });
  }

  finishSubagent(id: string): void {
    this.change(() => { this.subagents.delete(id); });
  }

  refresh(): void {
    if (!this.disposed) this.render();
  }

  shutdown(): void {
    if (this.disposed) return;
    this.disposed = true;
    if (this.timer !== undefined) this.scheduler.clearInterval(this.timer);
    this.timer = undefined;
    this.main = false;
    this.background = false;
    this.subagents.clear();
    this.setTitle(`${IDLE_INDICATOR} ${this.getName()}`);
  }

  private change(update: () => void): void {
    if (this.disposed) return;
    const wasActive = this.active();
    update();
    const active = this.active();
    if (!wasActive && active) this.start();
    if (wasActive && !active) this.stop();
  }

  private start(): void {
    this.frame = 0;
    this.render();
    this.timer = this.scheduler.setInterval(() => {
      if (this.disposed) return;
      this.frame = (this.frame + 1) % ACTIVE_FRAMES.length;
      this.render();
    }, SPINNER_INTERVAL_MS);
  }

  private stop(): void {
    if (this.timer !== undefined) this.scheduler.clearInterval(this.timer);
    this.timer = undefined;
    this.frame = 0;
    this.render();
  }

  private render(): void {
    const indicator = this.active() ? ACTIVE_FRAMES[this.frame] : IDLE_INDICATOR;
    this.setTitle(`${indicator} ${this.getName()}`);
  }

  private active(): boolean {
    return this.main || this.background || this.subagents.size > 0;
  }
}
