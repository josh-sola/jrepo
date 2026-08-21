import { randomUUID } from "node:crypto";
import { homedir } from "node:os";

import { PlanterController, type PlanterContext } from "./controller.ts";
import type { ExecCommand } from "./helpers.ts";
import type { EventBus, Scheduler } from "./protocol.ts";
import type { StateEnvironment } from "./state.ts";
import { planterStateDirectory } from "./store.ts";

export type PlanterDependencies = {
  exec: ExecCommand;
  events: EventBus;
  getSessionName(): string | undefined;
  scheduler?: Scheduler;
  requestId?: () => string;
  tempId?: () => string;
  environment?: StateEnvironment;
  pid?: number;
  home?: string;
  now?: () => number;
};
export type PlanterExtension = {
  onSessionStart(event: unknown, context: PlanterContext): Promise<void>;
  onSessionInfoChanged(event: unknown, context: PlanterContext): Promise<void>;
  onAgentStart(event: unknown, context: PlanterContext): Promise<void>;
  onAgentSettled(event: unknown, context: PlanterContext): Promise<void>;
  onSessionShutdown(event: unknown, context: PlanterContext): Promise<void>;
  flush(): Promise<void>;
  recordPath(): string | undefined;
};

const defaultScheduler: Scheduler = {
  setInterval: (callback, delay) => setInterval(callback, delay),
  clearInterval: (timer) => clearInterval(timer as ReturnType<typeof setInterval>),
  setTimeout: (callback, delay) => setTimeout(callback, delay),
  clearTimeout: (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
};

function eventReason(event: unknown): string | undefined {
  if (typeof event !== "object" || event === null || Array.isArray(event)) return undefined;
  const reason = (event as Record<string, unknown>).reason;
  return typeof reason === "string" ? reason : undefined;
}

export function createPlanterExtension(dependencies: PlanterDependencies): PlanterExtension {
  const scheduler = dependencies.scheduler ?? defaultScheduler;
  const requestId = dependencies.requestId ?? randomUUID;
  const environment = dependencies.environment ?? process.env;
  const pid = dependencies.pid ?? process.pid;
  const now = dependencies.now ?? (() => Math.floor(Date.now() / 1_000));
  const directory = planterStateDirectory(environment, dependencies.home ?? homedir());
  let generation = 0;
  let active: PlanterController | undefined;

  return {
    async onSessionStart(_event, context) {
      generation += 1;
      if (context.mode !== "tui") {
        const previous = active;
        active = undefined;
        await previous?.shutdown(false);
        return;
      }
      const sessionId = context.sessionManager.getSessionId();
      if (typeof sessionId !== "string" || sessionId.length === 0) {
        const previous = active;
        active = undefined;
        await previous?.shutdown(false);
        return;
      }
      const previous = active;
      active = undefined;
      await previous?.shutdown(previous.sessionId === sessionId);

      const controller = new PlanterController({
        exec: dependencies.exec,
        events: dependencies.events,
        getSessionName: dependencies.getSessionName,
        scheduler,
        requestId,
        generation,
        environment,
        pid,
        now,
        stateDirectory: directory,
        tempId: dependencies.tempId,
      }, context);
      active = controller;
      await controller.start();
      if (active !== controller) await controller.shutdown(true);
    },

    async onSessionInfoChanged() {
      await active?.refreshLabel();
    },

    async onAgentStart() {
      await active?.setMain(true);
    },

    async onAgentSettled() {
      await active?.setMain(false);
    },

    async onSessionShutdown(event) {
      generation += 1;
      const controller = active;
      active = undefined;
      await controller?.shutdown(eventReason(event) === "reload");
    },

    async flush() {
      await active?.flush();
    },

    recordPath() {
      return active?.recordPath();
    },
  };
}
