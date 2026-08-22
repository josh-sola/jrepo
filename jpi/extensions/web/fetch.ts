import { randomUUID } from "node:crypto";

import type { AssistantMessage, AuthResult, Model, Usage } from "@earendil-works/pi-ai";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { Static, TObject, TString } from "typebox";

import type { KetchRunner } from "./ketch.ts";
import { truncateDiagnostic } from "./ketch.ts";
import { buildWebFetchUserMessage, WEB_FETCH_SYSTEM_PROMPT } from "./prompt.ts";
import { boundedText, isRecord } from "./text.ts";

const WEB_FETCH_TIMEOUT_MS = 60_000;
const WEB_FETCH_MAX_TOKENS = 2_048;
const MAX_FETCH_URL_CHARS = 8_192;
const MAX_FETCH_TITLE_CHARS = 500;

type WebFetchParameters = TObject<{ url: TString; prompt: TString }>;

export const webFetchParameters = {
  type: "object",
  properties: {
    url: {
      type: "string",
      minLength: 1,
      description: "The HTTP or HTTPS URL to fetch",
    },
    prompt: {
      type: "string",
      minLength: 1,
      description: "The question to answer from the page",
    },
  },
  required: ["url", "prompt"],
  additionalProperties: false,
} as unknown as WebFetchParameters;

export type WebFetchInput = Static<WebFetchParameters>;

export type WebFetchDetails = {
  requestedUrl: string;
  fetchedUrl?: string;
  title: string;
};

type WebFetchContext = {
  model?: Model<any>;
  modelRegistry?: {
    getProviderAuth(provider: string): Promise<AuthResult | undefined>;
    complete(model: Model<any>, context: unknown, options?: Record<string, unknown>): Promise<AssistantMessage>;
  };
};

export type WebFetchToolOptions = {
  runner: KetchRunner;
  createSessionId?: () => string;
  now?: () => number;
};

type FetchedPage = {
  url: string;
  fetchedUrl?: string;
  title: string;
  markdown: string;
};

function normalizeHttpUrl(value: string, label: string): string {
  const trimmed = value.trim();
  if (!trimmed) throw new Error(`web_fetch needs a non-empty ${label}.`);

  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    throw new Error(`web_fetch needs a valid HTTP or HTTPS ${label}.`);
  }

  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("web_fetch only accepts HTTP and HTTPS URLs.");
  }

  if (url.username || url.password) {
    throw new Error("web_fetch does not accept URLs with embedded credentials.");
  }

  const normalized = url.toString();
  if (normalized.length > MAX_FETCH_URL_CHARS) {
    throw new Error(`web_fetch does not accept ${label}s longer than ${MAX_FETCH_URL_CHARS} characters.`);
  }
  return normalized;
}

export function normalizeFetchUrl(value: string): string {
  return normalizeHttpUrl(value, "URL");
}

function normalizeKetchUrl(value: unknown, label: string): string | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string") throw new Error(`Ketch returned malformed page output: ${label} must be a URL string.`);
  return normalizeHttpUrl(value, label);
}

export function parseFetchedPage(rawPage: unknown): FetchedPage {
  if (!isRecord(rawPage)) throw new Error("Ketch returned malformed page output.");

  const url = normalizeKetchUrl(rawPage.url, "url");
  if (!url) throw new Error("Ketch returned malformed page output: url is missing.");

  const fetchedUrl = normalizeKetchUrl(rawPage.fetched_url, "fetched_url");
  const markdown = typeof rawPage.markdown === "string" ? rawPage.markdown : "";
  if (!markdown.trim()) throw new Error("Ketch returned no readable page text.");

  return {
    url,
    fetchedUrl,
    title: boundedText(rawPage.title, MAX_FETCH_TITLE_CHARS),
    markdown,
  };
}

function getAssistantText(response: AssistantMessage): string {
  if (response.stopReason === "error") {
    const message = truncateDiagnostic(response.errorMessage || "model error");
    throw new Error(`The focused page answer failed: ${message}`);
  }
  if (response.stopReason === "aborted") {
    throw new Error("The focused page answer was cancelled.");
  }
  if (response.stopReason !== "stop") {
    throw new Error(`The focused page answer stopped before it finished (${response.stopReason}).`);
  }

  const text = response.content
    .filter((part): part is { type: "text"; text: string } => isRecord(part) && part.type === "text" && typeof part.text === "string")
    .map((part) => part.text)
    .join("")
    .trim();

  if (!text) throw new Error("The focused page answer was empty.");
  return text;
}

async function getActiveModel(ctx: WebFetchContext): Promise<Model<any>> {
  const model = ctx.model;
  if (!model) throw new Error("web_fetch needs an active Pi model. Select a model and try again.");
  if (!ctx.modelRegistry) throw new Error("web_fetch cannot call the active model in this session.");

  const auth = await ctx.modelRegistry.getProviderAuth(model.provider);
  if (!auth) {
    throw new Error(`web_fetch needs configured auth for the active model provider (${model.provider}). Run /login or configure auth, then try again.`);
  }

  return model;
}

async function answerFromPage(
  ctx: WebFetchContext,
  input: WebFetchInput,
  requestedUrl: string,
  page: FetchedPage,
  signal: AbortSignal | undefined,
  options: Required<Pick<WebFetchToolOptions, "createSessionId" | "now">> & { model: Model<any> },
): Promise<{ text: string; usage: Usage }> {
  const fetchedUrl = page.fetchedUrl && page.fetchedUrl !== requestedUrl ? page.fetchedUrl : undefined;

  let response: AssistantMessage;
  try {
    response = await ctx.modelRegistry!.complete(
      options.model,
      {
        systemPrompt: WEB_FETCH_SYSTEM_PROMPT,
        messages: [buildWebFetchUserMessage({ requestedUrl, fetchedUrl, title: page.title, question: input.prompt, markdown: page.markdown }, options.now)],
      },
      {
        cacheRetention: "none",
        maxTokens: WEB_FETCH_MAX_TOKENS,
        reasoning: "minimal",
        sessionId: options.createSessionId(),
        signal,
      },
    );
  } catch (error) {
    if (signal?.aborted) throw new Error("The focused page answer was cancelled.");
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`The focused page answer failed: ${truncateDiagnostic(message)}`);
  }

  if (signal?.aborted) throw new Error("The focused page answer was cancelled.");
  return { text: getAssistantText(response), usage: response.usage };
}

export async function executeWebFetch(
  input: WebFetchInput,
  ctx: WebFetchContext,
  options: WebFetchToolOptions,
  signal?: AbortSignal,
) {
  const requestedUrl = normalizeFetchUrl(input.url);
  const model = await getActiveModel(ctx);
  const rawPage = await options.runner.runJson(
    ["scrape", requestedUrl, "--json", "--no-llms-txt", "--max-chars", "40000"],
    { timeoutMs: WEB_FETCH_TIMEOUT_MS, signal },
  );
  const page = parseFetchedPage(rawPage);
  const helpers = {
    createSessionId: options.createSessionId ?? randomUUID,
    now: options.now ?? Date.now,
  };
  const answer = await answerFromPage(ctx, input, requestedUrl, page, signal, { ...helpers, model });
  const fetchedUrl = page.fetchedUrl && page.fetchedUrl !== requestedUrl ? page.fetchedUrl : undefined;

  const details: WebFetchDetails = {
    requestedUrl,
    ...(fetchedUrl ? { fetchedUrl } : {}),
    title: page.title,
  };

  return {
    content: [{ type: "text", text: answer.text }],
    details,
    usage: answer.usage,
  };
}

export function createWebFetchTool(options: WebFetchToolOptions): ToolDefinition<WebFetchParameters, WebFetchDetails> {
  return {
    name: "web_fetch",
    label: "Web Fetch",
    description: "Fetch one known HTTP or HTTPS URL with ketch and answer a focused question from the page.",
    promptSnippet: "Fetch a known web URL and answer a focused question from that page",
    promptGuidelines: [
      "Use web_fetch only when you already have a specific HTTP or HTTPS URL.",
      "Use web_fetch instead of web_search when page content is needed from a known URL.",
    ],
    parameters: webFetchParameters,
    async execute(_toolCallId: string, params: WebFetchInput, signal: AbortSignal | undefined, _onUpdate: unknown, ctx: WebFetchContext) {
      return executeWebFetch(params, ctx, options, signal);
    },
  };
}
