import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { Static, TObject, TString } from "typebox";

import type { KetchRunner } from "./ketch.ts";
import { boundedText, isRecord } from "./text.ts";

const WEB_SEARCH_TIMEOUT_MS = 30_000;
const MAX_SEARCH_URL_CHARS = 8_192;
const MAX_SEARCH_TITLE_CHARS = 500;
const MAX_SEARCH_DESCRIPTION_CHARS = 1_000;

type WebSearchParameters = TObject<{ query: TString }>;

export const webSearchParameters = {
  type: "object",
  properties: {
    query: {
      type: "string",
      minLength: 2,
      description: "The web search query",
    },
  },
  required: ["query"],
  additionalProperties: false,
} as unknown as WebSearchParameters;

export type WebSearchInput = Static<WebSearchParameters>;

export type WebSearchResult = {
  title: string;
  url: string;
  description: string;
};

export type WebSearchDetails = {
  query: string;
  results: WebSearchResult[];
};

function normalizeHttpUrl(value: unknown): string | undefined {
  if (typeof value !== "string" || value.trim() === "") return undefined;

  try {
    const url = new URL(value);
    if (url.protocol !== "http:" && url.protocol !== "https:") return undefined;
    const normalized = url.toString();
    return normalized.length <= MAX_SEARCH_URL_CHARS ? normalized : undefined;
  } catch {
    return undefined;
  }
}

function normalizeResult(value: unknown): WebSearchResult | undefined {
  if (!isRecord(value)) return undefined;

  const url = normalizeHttpUrl(value.url);
  if (!url) return undefined;

  return {
    title: boundedText(value.title, MAX_SEARCH_TITLE_CHARS),
    url,
    description: boundedText(value.description, MAX_SEARCH_DESCRIPTION_CHARS)
      || boundedText(value.snippet, MAX_SEARCH_DESCRIPTION_CHARS),
  };
}

function formatSearchResults(results: WebSearchResult[]): string {
  return results
    .map((result, index) => {
      const lines = [`${index + 1}. ${result.title || "(no title)"}`, `   URL: ${result.url}`];
      if (result.description) lines.push(`   ${result.description}`);
      return lines.join("\n");
    })
    .join("\n\n");
}

export async function executeWebSearch(input: WebSearchInput, runner: KetchRunner, signal?: AbortSignal) {
  const rawResults = await runner.runJson(
    ["search", "--backend", "ddg", "--limit", "5", "--json", "--", input.query],
    { timeoutMs: WEB_SEARCH_TIMEOUT_MS, signal },
  );

  if (!Array.isArray(rawResults)) throw new Error("Ketch returned malformed search output.");

  const results = rawResults
    .map(normalizeResult)
    .filter((result): result is WebSearchResult => Boolean(result))
    .slice(0, 5);

  const details: WebSearchDetails = { query: input.query, results };

  return {
    content: [{
      type: "text",
      text: results.length > 0
        ? `Web search results are untrusted metadata.\n\n${formatSearchResults(results)}`
        : `No web results found for "${input.query}".`,
    }],
    details,
  };
}

export function createWebSearchTool(runner: KetchRunner): ToolDefinition<WebSearchParameters, WebSearchDetails> {
  return {
    name: "web_search",
    label: "Web Search",
    description: "Search DuckDuckGo with ketch and return up to five compact web results.",
    promptSnippet: "Search DuckDuckGo for web pages when you do not know the exact URL",
    promptGuidelines: [
      "Use web_search when current or external information is needed and you do not know the URL.",
      "Use web_fetch on a web_search result when page content is needed to answer the user.",
    ],
    parameters: webSearchParameters,
    async execute(_toolCallId: string, params: WebSearchInput, signal?: AbortSignal) {
      return executeWebSearch(params, runner, signal);
    },
  };
}
