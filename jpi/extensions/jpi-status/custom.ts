import { dirname, resolve } from "node:path";

import type { ExecCommand, RepositoryMetadata } from "./data.ts";
import type { StatusLineFormat } from "./layout.ts";

export const CUSTOM_COMPONENT_PREFIX = "@custom:";
export const CUSTOM_REFRESH_INTERVAL_MS = 10_000;
export const CUSTOM_COMMAND_TIMEOUT_MS = 3_000;

export type CustomOccurrence = {
  key: string;
  id: string;
  path: string;
  lineIndex: number;
  componentIndex: number;
};

export type CustomStatusPayload = {
  cwd: string | null;
  idle: boolean | null;
  model: {
    id: string | null;
    name: string | null;
    provider: string | null;
    reasoning: boolean | null;
    contextWindow: number | null;
    maxTokens: number | null;
  } | null;
  thinkingLevel: string | null;
  context: {
    tokens: number | null;
    contextWindow: number | null;
    percent: number | null;
  };
  repository: RepositoryMetadata;
  statuses: Record<string, string>;
};

export type CustomPayloadContext = {
  cwd?: string;
  model?: {
    id?: string;
    name?: string;
    provider?: string;
    reasoning?: boolean;
    contextWindow?: number;
    maxTokens?: number;
  };
  thinkingLevel?: string;
  isIdle?(): boolean;
  getContextUsage(): {
    tokens?: number | null;
    contextWindow?: number | null;
    percent?: number | null;
  } | undefined;
};

export type IntervalScheduler = {
  setInterval(callback: () => void, delay: number): ReturnType<typeof setInterval>;
  clearInterval(timer: ReturnType<typeof setInterval>): void;
};

export function customOccurrenceKey(lineIndex: number, componentIndex: number): string {
  return `${lineIndex}:${componentIndex}`;
}

export function isCustomComponentId(componentId: string): boolean {
  return componentId.startsWith(CUSTOM_COMPONENT_PREFIX)
    && componentId.slice(CUSTOM_COMPONENT_PREFIX.length).trim() !== "";
}

export function resolveCustomExecutable(componentId: string, configPath: string): string {
  return resolve(dirname(configPath), componentId.slice(CUSTOM_COMPONENT_PREFIX.length));
}

export function getCustomOccurrences(
  format: StatusLineFormat,
  configPath: string,
): CustomOccurrence[] {
  const occurrences: CustomOccurrence[] = [];
  for (let lineIndex = 0; lineIndex < format.length; lineIndex += 1) {
    const line = format[lineIndex]!;
    for (let componentIndex = 0; componentIndex < line.length; componentIndex += 1) {
      const id = line[componentIndex]!;
      if (!isCustomComponentId(id)) continue;
      occurrences.push({
        key: customOccurrenceKey(lineIndex, componentIndex),
        id,
        path: resolveCustomExecutable(id, configPath),
        lineIndex,
        componentIndex,
      });
    }
  }
  return occurrences;
}

function nullable<T>(value: T | undefined): T | null {
  return value === undefined ? null : value;
}

export function createCustomStatusPayload(
  context: CustomPayloadContext,
  repository: RepositoryMetadata,
  statuses: ReadonlyMap<string, string>,
): CustomStatusPayload {
  const usage = context.getContextUsage();
  const sortedStatuses = Object.fromEntries(
    [...statuses.entries()].sort(([left], [right]) => left.localeCompare(right)),
  );
  return {
    cwd: nullable(context.cwd),
    idle: context.isIdle ? context.isIdle() : null,
    model: context.model
      ? {
          id: nullable(context.model.id),
          name: nullable(context.model.name),
          provider: nullable(context.model.provider),
          reasoning: nullable(context.model.reasoning),
          contextWindow: nullable(context.model.contextWindow),
          maxTokens: nullable(context.model.maxTokens),
        }
      : null,
    thinkingLevel: nullable(context.thinkingLevel),
    context: {
      tokens: nullable(usage?.tokens),
      contextWindow: nullable(usage?.contextWindow ?? context.model?.contextWindow),
      percent: nullable(usage?.percent),
    },
    repository,
    statuses: sortedStatuses,
  };
}

type CustomControllerOptions = {
  exec: ExecCommand;
  format: StatusLineFormat;
  configPath: string;
  getPayload(): CustomStatusPayload;
  requestRender(): void;
  notify(message: string, level: "warning"): void;
  scheduler: IntervalScheduler;
};

type CustomOccurrenceState = {
  output?: string;
  failure?: string;
};

type ActiveRun = {
  generation: number;
  abortController: AbortController;
  promise: Promise<void>;
};

function concise(text: string): string {
  const singleLine = text.replace(/[\r\n\t]/g, " ").replace(/ +/g, " ").trim();
  return singleLine.length > 160 ? `${singleLine.slice(0, 159)}…` : singleLine;
}

function resultFailure(result: Awaited<ReturnType<ExecCommand>>): string | undefined {
  const detail = concise(result.stderr);
  if (result.killed) {
    return detail
      ? `timed out after ${CUSTOM_COMMAND_TIMEOUT_MS}ms: ${detail}`
      : `timed out after ${CUSTOM_COMMAND_TIMEOUT_MS}ms`;
  }
  if (result.code !== 0) {
    return detail ? `exited with code ${result.code}: ${detail}` : `exited with code ${result.code}`;
  }
  return undefined;
}

function thrownFailure(error: unknown): string {
  const message = concise(error instanceof Error ? error.message : String(error));
  return message ? `could not run: ${message}` : "could not run";
}

export class CustomStatusController {
  private readonly options: CustomControllerOptions;
  private occurrences: CustomOccurrence[];
  private states = new Map<string, CustomOccurrenceState>();
  private generation = 0;
  private started = false;
  private disposed = false;
  private pending = false;
  private activeRun?: ActiveRun;
  private timer?: ReturnType<typeof setInterval>;

  constructor(options: CustomControllerOptions) {
    this.options = options;
    this.occurrences = getCustomOccurrences(options.format, options.configPath);
    this.rebuildStates();
  }

  get outputs(): ReadonlyMap<string, string> {
    return new Map(
      [...this.states.entries()]
        .filter((entry): entry is [string, CustomOccurrenceState & { output: string }] => entry[1].output !== undefined)
        .map(([key, state]) => [key, state.output]),
    );
  }

  start(): Promise<void> {
    if (this.started || this.disposed) return Promise.resolve();
    this.started = true;
    this.syncTimer();
    return this.launch();
  }

  refresh(): Promise<void> {
    if (!this.started || this.disposed) return Promise.resolve();
    return this.launch();
  }

  updateFormat(format: StatusLineFormat): Promise<void> {
    if (this.disposed) return Promise.resolve();
    this.generation += 1;
    this.pending = false;
    this.activeRun?.abortController.abort();
    this.activeRun = undefined;
    this.occurrences = getCustomOccurrences(format, this.options.configPath);
    this.rebuildStates();
    this.syncTimer();
    this.options.requestRender();
    return this.started ? this.launch() : Promise.resolve();
  }

  private rebuildStates(): void {
    this.states = new Map(this.occurrences.map((occurrence) => [occurrence.key, {}]));
  }

  private syncTimer(): void {
    if (!this.started || this.disposed) return;
    if (this.occurrences.length > 0 && this.timer === undefined) {
      this.timer = this.options.scheduler.setInterval(
        () => void this.refresh(),
        CUSTOM_REFRESH_INTERVAL_MS,
      );
    } else if (this.occurrences.length === 0 && this.timer !== undefined) {
      this.options.scheduler.clearInterval(this.timer);
      this.timer = undefined;
    }
  }

  private launch(): Promise<void> {
    if (this.disposed || this.occurrences.length === 0) return Promise.resolve();
    if (this.activeRun) {
      this.pending = true;
      return this.activeRun.promise;
    }

    const generation = this.generation;
    const abortController = new AbortController();
    const run: ActiveRun = {
      generation,
      abortController,
      promise: Promise.resolve(),
    };
    this.activeRun = run;
    run.promise = this.runOccurrences(run).finally(() => {
      if (this.activeRun !== run) return;
      this.activeRun = undefined;
      if (this.pending && !this.disposed) {
        this.pending = false;
        void this.launch();
      }
    });
    return run.promise;
  }

  private async runOccurrences(run: ActiveRun): Promise<void> {
    const occurrences = this.occurrences;
    const payload = this.options.getPayload();
    const argument = JSON.stringify(payload);
    await Promise.all(occurrences.map(async (occurrence) => {
      try {
        const result = await this.options.exec(occurrence.path, [argument], {
          cwd: payload.cwd ?? undefined,
          timeout: CUSTOM_COMMAND_TIMEOUT_MS,
          signal: run.abortController.signal,
        });
        if (!this.isCurrent(run)) return;
        const failure = resultFailure(result);
        if (failure) {
          this.recordFailure(occurrence, failure);
          return;
        }
        const state = this.states.get(occurrence.key);
        if (!state) return;
        state.output = result.stdout.trim() === "" ? undefined : result.stdout;
        state.failure = undefined;
      } catch (error) {
        if (this.isCurrent(run)) this.recordFailure(occurrence, thrownFailure(error));
      }
    }));

    if (this.isCurrent(run)) this.options.requestRender();
  }

  private isCurrent(run: ActiveRun): boolean {
    return !this.disposed && run.generation === this.generation;
  }

  private recordFailure(occurrence: CustomOccurrence, reason: string): void {
    const state = this.states.get(occurrence.key);
    if (!state) return;
    state.output = undefined;
    if (state.failure !== reason) {
      this.options.notify(
        `jpi-status ${occurrence.id} at format[${occurrence.lineIndex}][${occurrence.componentIndex}] failed: ${reason}.`,
        "warning",
      );
    }
    state.failure = reason;
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.generation += 1;
    this.pending = false;
    this.activeRun?.abortController.abort();
    this.activeRun = undefined;
    if (this.timer !== undefined) {
      this.options.scheduler.clearInterval(this.timer);
      this.timer = undefined;
    }
  }
}
