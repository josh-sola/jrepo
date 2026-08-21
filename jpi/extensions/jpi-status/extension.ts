import {
  createDefaultStatusLineConfig,
  getStatusLineConfigPath,
  loadStatusLineConfig,
  type ReadTextFile,
  type StatusLineConfig,
} from "./config.ts";
import {
  createCustomStatusPayload,
  CustomStatusController,
  type IntervalScheduler,
} from "./custom.ts";
import { loadRepositoryMetadata, type ExecCommand, type RepositoryMetadata } from "./data.ts";
import { renderFooter, type WidthHelpers } from "./render.ts";

const REFRESH_INTERVAL_MS = 10_000;

type FooterData = {
  getExtensionStatuses(): ReadonlyMap<string, string>;
  onBranchChange(callback: () => void): () => void;
};

type FooterContext = {
  mode: string;
  cwd: string;
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
    percent: number | null;
  } | undefined;
  ui: {
    notify(message: string, level?: "info" | "warning" | "error"): void;
    setFooter(factory: ((
      tui: { requestRender(): void },
      theme: unknown,
      footerData: FooterData,
    ) => { render(width: number): string[]; invalidate(): void; dispose(): void }) | undefined): void;
  };
};

type Scheduler = IntervalScheduler;

export type StatusExtension = {
  onSessionStart(event: unknown, context: FooterContext): Promise<void>;
  onCommand(args: string, context: FooterContext): Promise<void>;
};

export type StatusConfigDependencies = {
  configPath?: string;
  readTextFile?: ReadTextFile;
};

type ControllerOptions = {
  exec: ExecCommand;
  cwd: string;
  requestRender(): void;
  onBranchChange(callback: () => void): () => void;
  scheduler: Scheduler;
  onDispose(): void;
};

export class RepositoryMetadataController {
  metadata: RepositoryMetadata = {};

  private readonly options: ControllerOptions;
  private generation = 0;
  private pending = false;
  private disposed = false;
  private drainPromise?: Promise<void>;
  private abortController?: AbortController;
  private unsubscribe?: () => void;
  private timer?: ReturnType<typeof setInterval>;

  constructor(options: ControllerOptions) {
    this.options = options;
  }

  start(): void {
    this.unsubscribe = this.options.onBranchChange(() => {
      this.options.requestRender();
      void this.refresh();
    });
    this.timer = this.options.scheduler.setInterval(() => void this.refresh(), REFRESH_INTERVAL_MS);
    void this.refresh();
  }

  refresh(): Promise<void> {
    if (this.disposed) return Promise.resolve();
    this.generation += 1;
    this.pending = true;
    if (!this.drainPromise) {
      this.drainPromise = this.drain().finally(() => {
        this.drainPromise = undefined;
      });
    }
    return this.drainPromise;
  }

  private async drain(): Promise<void> {
    while (this.pending && !this.disposed) {
      this.pending = false;
      const generation = this.generation;
      const abortController = new AbortController();
      this.abortController = abortController;
      const metadata = await loadRepositoryMetadata(
        this.options.exec,
        this.options.cwd,
        abortController.signal,
      );
      if (!this.disposed && generation === this.generation) {
        this.metadata = metadata;
        this.options.requestRender();
      }
    }
    this.abortController = undefined;
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.generation += 1;
    this.pending = false;
    this.abortController?.abort();
    this.unsubscribe?.();
    if (this.timer !== undefined) this.options.scheduler.clearInterval(this.timer);
    this.options.onDispose();
  }
}

export function createStatusExtension(
  exec: ExecCommand,
  helpers: WidthHelpers,
  scheduler: Scheduler = { setInterval, clearInterval },
  configDependencies: StatusConfigDependencies = {},
): StatusExtension {
  const configPath = configDependencies.configPath ?? getStatusLineConfigPath();
  let activeController: RepositoryMetadataController | undefined;
  let activeCustomController: CustomStatusController | undefined;
  let statusLineConfig: StatusLineConfig = createDefaultStatusLineConfig();
  let requestFooterRender: (() => void) | undefined;

  const reloadConfig = async (context: FooterContext, announce: boolean): Promise<void> => {
    const result = await loadStatusLineConfig(configPath, configDependencies.readTextFile);
    statusLineConfig = result.config;
    if (activeCustomController) {
      await activeCustomController.updateFormat(statusLineConfig.format);
    } else if (announce) {
      requestFooterRender?.();
    }

    if (result.problem) {
      context.ui.notify(
        `Could not load jpi-status config at ${configPath}: ${result.problem}. Using the default config.`,
        "warning",
      );
      return;
    }

    if (announce) context.ui.notify("jpi-status config reloaded.", "info");
  };

  return {
    async onSessionStart(_event, context) {
      if (context.mode !== "tui") return;
      await reloadConfig(context, false);
      context.ui.setFooter((tui, _theme, footerData) => {
        const renderFooterNow = () => tui.requestRender();
        let controller: RepositoryMetadataController;
        controller = new RepositoryMetadataController({
          exec,
          cwd: context.cwd,
          requestRender: renderFooterNow,
          onBranchChange: (callback) => footerData.onBranchChange(callback),
          scheduler,
          onDispose: () => {
            if (activeController === controller) activeController = undefined;
            if (requestFooterRender === renderFooterNow) requestFooterRender = undefined;
          },
        });
        const customController = new CustomStatusController({
          exec,
          format: statusLineConfig.format,
          configPath,
          getPayload: () => createCustomStatusPayload(
            context,
            controller.metadata,
            footerData.getExtensionStatuses(),
          ),
          requestRender: renderFooterNow,
          notify: (message, level) => context.ui.notify(message, level),
          scheduler,
        });
        activeController = controller;
        activeCustomController = customController;
        requestFooterRender = renderFooterNow;
        controller.start();
        void customController.start();

        return {
          invalidate() {},
          render(width) {
            const percent = context.getContextUsage()?.percent;
            return renderFooter({
              modelName: context.model?.name || context.model?.id || "no model",
              contextPercent: percent === null ? undefined : percent,
              repository: controller.metadata,
              statuses: footerData.getExtensionStatuses(),
              customOutputs: customController.outputs,
              config: statusLineConfig,
            }, width, helpers);
          },
          dispose: () => {
            customController.dispose();
            if (activeCustomController === customController) activeCustomController = undefined;
            controller.dispose();
          },
        };
      });
    },

    async onCommand(args, context) {
      const action = args.trim() || "status";
      if (action === "status") {
        context.ui.notify(
          activeController ? "jpi-status footer is active." : "jpi-status footer is not active.",
          "info",
        );
        return;
      }
      if (action === "refresh") {
        if (!activeController) {
          context.ui.notify("jpi-status footer is not active.", "warning");
          return;
        }
        void activeController.refresh();
        context.ui.notify("jpi-status metadata refresh requested.", "info");
        return;
      }
      if (action === "reload") {
        await reloadConfig(context, true);
        return;
      }
      context.ui.notify("Usage: /jpi-status [status|refresh|reload]", "warning");
    },
  };
}
