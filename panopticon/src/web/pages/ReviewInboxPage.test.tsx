import '../test-setup.ts';
import { afterEach, beforeAll, describe, expect, test } from 'bun:test';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryRouter, RouterProvider } from 'react-router';
import type {
  ReviewInboxItem,
  ReviewInboxResponse,
} from '../../shared/reviewInbox.ts';
import { InboxPage } from './InboxPage.tsx';
import { ReviewInboxPage } from './ReviewInboxPage.tsx';

beforeAll(() => {
  (
    globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement;
let root: Root;
let originalFetch: typeof fetch;

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  globalThis.fetch = originalFetch;
});

function fakeItem(overrides: Partial<ReviewInboxItem> = {}): ReviewInboxItem {
  return {
    owner: 'acme',
    repo: 'widgets',
    number: 1,
    title: 'A pull request',
    url: 'https://github.com/acme/widgets/pull/1',
    draft: false,
    author: { login: 'jordan', avatarUrl: null },
    updatedAt: '2026-01-01T00:00:00Z',
    additions: 1,
    deletions: 1,
    reviewers: [],
    ...overrides,
  };
}

function emptyResponse(): ReviewInboxResponse {
  return {
    sections: {
      returned: [],
      needsReview: [],
      approved: [],
      waiting: [],
      drafts: [],
    },
    fetchedAt: '2026-01-01T00:00:00Z',
  };
}

function stubFetch(response: ReviewInboxResponse): void {
  originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(JSON.stringify(response), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })) as unknown as typeof fetch;
}

async function renderAt(path: string): Promise<HTMLDivElement> {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  const router = createMemoryRouter(
    [
      { path: '/', Component: InboxPage },
      { path: '/inbox', Component: ReviewInboxPage },
    ],
    { initialEntries: [path] },
  );
  // No retries: a deliberately failing fetch in the error-state test should
  // surface within the fixed wait below, not after react-query's backoff.
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  await act(async () => {
    root.render(
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
  });
  return container;
}

async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

describe('ReviewInboxPage', () => {
  test('renders one row per section from the fetched response', async () => {
    stubFetch({
      sections: {
        returned: [fakeItem({ number: 1, title: 'returned pr' })],
        needsReview: [
          fakeItem({
            number: 2,
            title: 'needs review pr',
            author: { login: 'ann', avatarUrl: null },
          }),
        ],
        approved: [fakeItem({ number: 3, title: 'approved pr' })],
        waiting: [fakeItem({ number: 4, title: 'waiting pr' })],
        drafts: [fakeItem({ number: 5, title: 'draft pr', draft: true })],
      },
      fetchedAt: '2026-01-01T00:00:00Z',
    });

    const el = await renderAt('/inbox');
    await settle();

    expect(el.textContent).toContain('returned pr');
    expect(el.textContent).toContain('needs review pr');
    expect(el.textContent).toContain('approved pr');
    expect(el.textContent).toContain('waiting pr');
    expect(el.textContent).toContain('draft pr');
    expect(el.textContent).toContain('Returned to you');
    expect(el.textContent).toContain('Needs your review');
    expect(el.textContent).toContain('Approved');
    expect(el.textContent).toContain('Waiting for review');
    expect(el.textContent).toContain('Drafts');
  });

  test('shows author login only on needs-your-review rows', async () => {
    stubFetch({
      sections: {
        returned: [],
        needsReview: [
          fakeItem({
            number: 2,
            title: 'needs review pr',
            author: { login: 'ann-author', avatarUrl: null },
          }),
        ],
        approved: [],
        waiting: [fakeItem({ number: 4, title: 'waiting pr' })],
        drafts: [],
      },
      fetchedAt: '2026-01-01T00:00:00Z',
    });

    const el = await renderAt('/inbox');
    await settle();

    expect(el.textContent).toContain('ann-author');
  });

  test('renders a "Nothing here" line for an empty section but keeps its heading', async () => {
    stubFetch(emptyResponse());

    const el = await renderAt('/inbox');
    await settle();

    expect(el.textContent).toContain('Returned to you');
    expect(el.textContent).toContain('Needs your review');
    const nothingHereCount = (el.textContent?.match(/Nothing here/g) ?? [])
      .length;
    expect(nothingHereCount).toBe(5);
  });

  test('shows a loading skeleton before the response arrives', async () => {
    originalFetch = globalThis.fetch;
    globalThis.fetch = (() => new Promise(() => {})) as unknown as typeof fetch;

    const el = await renderAt('/inbox');

    expect(
      el.querySelectorAll('[data-slot="skeleton"]').length,
    ).toBeGreaterThan(0);
  });

  test('shows an error state with a working Retry button', async () => {
    originalFetch = globalThis.fetch;
    let calls = 0;
    globalThis.fetch = (async () => {
      calls += 1;
      if (calls === 1) return new Response('boom', { status: 502 });
      return new Response(JSON.stringify(emptyResponse()), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    }) as unknown as typeof fetch;

    const el = await renderAt('/inbox');
    await settle();

    expect(el.textContent).toContain('Could not load the review inbox.');
    const retryButton = [...el.querySelectorAll('button')].find(
      (button) => button.textContent === 'Retry',
    );
    expect(retryButton).toBeDefined();

    await act(async () => {
      retryButton?.click();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(el.textContent).not.toContain('Could not load the review inbox.');
    expect(calls).toBe(2);
  });

  test('the toggle links to /inbox and / and marks the active view', async () => {
    stubFetch(emptyResponse());

    const el = await renderAt('/inbox');
    await settle();

    const links = [...el.querySelectorAll('nav a')];
    const inboxLink = links.find((a) => a.getAttribute('href') === '/inbox');
    const stacksLink = links.find((a) => a.getAttribute('href') === '/');
    expect(inboxLink?.getAttribute('aria-current')).toBe('page');
    expect(stacksLink?.getAttribute('aria-current')).toBeNull();
  });

  test('the stacks page toggle marks / as active', async () => {
    const inboxResponse = {
      stacks: [],
      recent: [],
      fetchedAt: '2026-01-01T00:00:00Z',
    };
    originalFetch = globalThis.fetch;
    globalThis.fetch = (async () =>
      new Response(JSON.stringify(inboxResponse), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })) as unknown as typeof fetch;

    const el = await renderAt('/');
    await settle();

    const links = [...el.querySelectorAll('nav a')];
    const stacksLink = links.find((a) => a.getAttribute('href') === '/');
    const inboxLink = links.find((a) => a.getAttribute('href') === '/inbox');
    expect(stacksLink?.getAttribute('aria-current')).toBe('page');
    expect(inboxLink?.getAttribute('aria-current')).toBeNull();
  });
});
