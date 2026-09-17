// Serves fixture JSON for the review page's routes so `VITE_MOCK=1` renders
// without the Bun server.
import prFixture from './fixtures/pr.json';
import diffFixture from './fixtures/diff.json';
import viewedFixture from './fixtures/viewed.json';
import prefsFixture from './fixtures/prefs.json';
import stackFixture from './fixtures/stack.json';

type Fixture = unknown;

const ROUTES: { pattern: RegExp; fixture: Fixture }[] = [
  {
    pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/diff(\?.*)?$/,
    fixture: diffFixture,
  },
  { pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/viewed$/, fixture: viewedFixture },
  { pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+\/stack$/, fixture: stackFixture },
  { pattern: /^\/api\/pr\/[^/]+\/[^/]+\/\d+$/, fixture: prFixture },
  { pattern: /^\/api\/prefs$/, fixture: prefsFixture },
];

function jsonResponse(body: Fixture): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

export function installMockFetch(): void {
  const realFetch = window.fetch.bind(window);
  const shimFetch = async (
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> => {
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.href
          : input.url;
    const path = url.startsWith('http')
      ? new URL(url).pathname + new URL(url).search
      : url;
    const route = ROUTES.find((r) => r.pattern.test(path));
    if (!route) return realFetch(input, init);
    return jsonResponse(route.fixture);
  };
  window.fetch = shimFetch as typeof fetch;
}
