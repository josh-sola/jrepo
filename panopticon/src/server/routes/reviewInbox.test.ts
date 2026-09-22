import { describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import type { ReviewInboxCandidate } from '../github/api.ts';
import type { ReviewInboxDeps } from './reviewInbox.ts';
import { reviewInboxRouter } from './reviewInbox.ts';

function fakeCandidate(
  overrides: Partial<ReviewInboxCandidate> = {},
): ReviewInboxCandidate {
  return {
    owner: 'acme',
    repo: 'widgets',
    number: 1,
    title: 'A pull request',
    url: 'https://github.com/acme/widgets/pull/1',
    draft: false,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    additions: 1,
    deletions: 1,
    author: { login: 'josh', avatarUrl: null, isBot: false },
    reviews: [],
    ...overrides,
  };
}

function mountReviewInboxRouter(deps: ReviewInboxDeps): Hono {
  const app = new Hono();
  app.route('/api/review-inbox', reviewInboxRouter(deps));
  return app;
}

describe('reviewInboxRouter', () => {
  test('returns empty sections without calling GitHub when there are no repos', async () => {
    const search = () =>
      Promise.reject(new Error('should not be called with no repos'));
    const app = mountReviewInboxRouter({
      github: { searchReviewInbox: search },
      login: 'josh',
      repos: [],
      clock: () => 0,
    });

    const res = await app.request('/api/review-inbox');

    expect(res.status).toBe(200);
    const body = (await res.json()) as { sections: unknown };
    expect(body.sections).toEqual({
      returned: [],
      needsReview: [],
      approved: [],
      waiting: [],
      drafts: [],
    });
  });

  test('queries author: and user-review-requested: across the configured repos', async () => {
    const queries: string[] = [];
    const app = mountReviewInboxRouter({
      github: {
        searchReviewInbox: (query) => {
          queries.push(query);
          return Promise.resolve([]);
        },
      },
      login: 'josh',
      repos: ['acme/widgets', 'acme/gadgets'],
      clock: () => 0,
    });

    await app.request('/api/review-inbox');

    expect(queries).toEqual([
      'is:pr is:open archived:false author:josh repo:acme/widgets repo:acme/gadgets',
      'is:pr is:open archived:false draft:false user-review-requested:josh repo:acme/widgets repo:acme/gadgets',
    ]);
  });

  test('categorizes the search results into the response sections', async () => {
    const waitingPr = fakeCandidate({ number: 1 });
    const requestedPr = fakeCandidate({
      number: 2,
      author: { login: 'ann', avatarUrl: null, isBot: false },
    });
    const app = mountReviewInboxRouter({
      github: {
        searchReviewInbox: (query) =>
          Promise.resolve(
            query.includes('author:josh') ? [waitingPr] : [requestedPr],
          ),
      },
      login: 'josh',
      repos: ['acme/widgets'],
      clock: () => 0,
    });

    const res = await app.request('/api/review-inbox');
    const body = (await res.json()) as {
      sections: {
        waiting: { number: number }[];
        needsReview: { number: number }[];
      };
    };

    expect(res.status).toBe(200);
    expect(body.sections.waiting.map((i) => i.number)).toEqual([1]);
    expect(body.sections.needsReview.map((i) => i.number)).toEqual([2]);
  });

  test('serves a cached result within the TTL without calling GitHub again', async () => {
    let calls = 0;
    const app = mountReviewInboxRouter({
      github: {
        searchReviewInbox: () => {
          calls += 1;
          return Promise.resolve([]);
        },
      },
      login: 'josh',
      repos: ['acme/widgets'],
      clock: () => 1_000,
    });

    await app.request('/api/review-inbox');
    await app.request('/api/review-inbox');

    expect(calls).toBe(2); // one authored + one requested search per fetch
  });

  test('re-fetches once the cache TTL elapses', async () => {
    let calls = 0;
    let now = 0;
    const app = mountReviewInboxRouter({
      github: {
        searchReviewInbox: () => {
          calls += 1;
          return Promise.resolve([]);
        },
      },
      login: 'josh',
      repos: ['acme/widgets'],
      clock: () => now,
    });

    await app.request('/api/review-inbox');
    now += 30_000;
    await app.request('/api/review-inbox');

    expect(calls).toBe(4);
  });

  test('shares one in-flight fetch across concurrent requests', async () => {
    let calls = 0;
    let resolveGate: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      resolveGate = resolve;
    });
    const app = mountReviewInboxRouter({
      github: {
        searchReviewInbox: async () => {
          calls += 1;
          await gate;
          return [];
        },
      },
      login: 'josh',
      repos: ['acme/widgets'],
      clock: () => 0,
    });

    const first = app.request('/api/review-inbox');
    const second = app.request('/api/review-inbox');
    resolveGate();
    const [firstRes, secondRes] = await Promise.all([first, second]);

    expect(firstRes.status).toBe(200);
    expect(secondRes.status).toBe(200);
    // Two searches (authored + requested) for the one shared fetch.
    expect(calls).toBe(2);
  });

  test('maps a GitHub failure to a 502 with the error message', async () => {
    const app = mountReviewInboxRouter({
      github: {
        searchReviewInbox: () => Promise.reject(new Error('github is down')),
      },
      login: 'josh',
      repos: ['acme/widgets'],
      clock: () => 0,
    });

    const res = await app.request('/api/review-inbox');

    expect(res.status).toBe(502);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('github is down');
  });
});
