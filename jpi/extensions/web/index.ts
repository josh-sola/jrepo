import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { createWebFetchTool, type WebFetchToolOptions } from "./fetch.ts";
import { createKetchRunner, type KetchRunner } from "./ketch.ts";
import { createWebSearchTool } from "./search.ts";

export type WebExtensionOptions = {
  runner?: KetchRunner;
} & Partial<Pick<WebFetchToolOptions, "createSessionId" | "now">>;

export function registerWebTools(pi: ExtensionAPI, options: WebExtensionOptions = {}) {
  const runner = options.runner ?? createKetchRunner({
    exec: (command, args, execOptions) => pi.exec(command, args, execOptions),
  });

  pi.registerTool(createWebSearchTool(runner));
  pi.registerTool(createWebFetchTool({
    runner,
    ...(options.createSessionId ? { createSessionId: options.createSessionId } : {}),
    ...(options.now ? { now: options.now } : {}),
  }));
}

export default function web(pi: ExtensionAPI) {
  registerWebTools(pi);
}
