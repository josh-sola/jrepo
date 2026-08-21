import { randomUUID } from "node:crypto";

import { TitleController, type TitleContext } from "./controller.ts";
import type { ExecCommand } from "./helpers.ts";
import type { EventBus, Scheduler } from "./protocol.ts";

export type TitleDependencies = {
  exec: ExecCommand;
  events: EventBus;
  getSessionName(): string | undefined;
  scheduler?: Scheduler;
  requestId?: () => string;
};

export type TitleExtension = {
  onSessionStart(event: unknown, context: TitleContext): Promise<void>;
  onSessionInfoChanged(event: unknown, context: TitleContext): void;
  onAgentStart(event: unknown, context: TitleContext): void;
  onAgentSettled(event: unknown, context: TitleContext): void;
  onSessionShutdown(event: unknown, context: TitleContext): void;
};

const defaultScheduler: Scheduler = {
  setInterval: (callback, delay) => setInterval(callback, delay),
  clearInterval: (timer) => clearInterval(timer as ReturnType<typeof setInterval>),
  setTimeout: (callback, delay) => setTimeout(callback, delay),
  clearTimeout: (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
};

export function createTitleExtension(dependencies: TitleDependencies): TitleExtension {
  const scheduler = dependencies.scheduler ?? defaultScheduler;
  const createRequestId = dependencies.requestId ?? randomUUID;
  let generation = 0;
  let activeController: TitleController | undefined;

  return {
    async onSessionStart(_event, context) {
      activeController?.shutdown();
      activeController = undefined;
      generation += 1;
      if (context.mode !== "tui") return;

      const controller = new TitleController({
        exec: dependencies.exec,
        events: dependencies.events,
        getSessionName: dependencies.getSessionName,
        scheduler,
        createRequestId,
        generation,
      }, context);
      activeController = controller;
      await controller.start();
    },

    onSessionInfoChanged() {
      activeController?.refreshName();
    },

    onAgentStart() {
      activeController?.setMainActive(true);
    },

    onAgentSettled() {
      activeController?.setMainActive(false);
    },

    onSessionShutdown() {
      activeController?.shutdown();
      activeController = undefined;
      generation += 1;
    },
  };
}
