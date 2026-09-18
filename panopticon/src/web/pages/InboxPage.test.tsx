import '../test-setup.ts';
import { afterEach, beforeAll, describe, expect, test } from 'bun:test';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryRouter, RouterProvider } from 'react-router';
import type { InboxResponse, InboxStackEntry } from '../../shared/inbox.ts';
import type { PrSummary } from '../../shared/github.ts';
import { InboxPage } from './InboxPage.tsx';

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

function fakePr(overrides: Partial<PrSummary> = {}): PrSummary {
  return {
    owner: 'acme',
    repo: 'widgets',
    number: 1,
    title: 'A pull request',
    body: '',
    state: 'open',
    draft: false,
    author: { login: 'jordan', avatarUrl: null, isBot: false },
    base: { ref: 'master', sha: 's' },
    head: { ref: 'feature', sha: 's' },
    mergeCommitSha: null,
    additions: 1,
    deletions: 1,
    changedFiles: 1,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    url: 'https://github.com/acme/widgets/pull/1',
    ...overrides,
  };
}

function fakeEntry(
  overrides: Partial<InboxStackEntry> & { number: number },
): InboxStackEntry {
  return {
    title: `pr ${overrides.number}`,
    state: 'open',
    draft: false,
    headRef: `branch-${overrides.number}`,
    baseRef: 'master',
    isCurrent: false,
    parent: null,
    additions: 1,
    deletions: 1,
    owner: 'acme',
    repo: 'widgets',
    updatedAt: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function stubInboxFetch(response: InboxResponse): void {
  originalFetch = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(JSON.stringify(response), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })) as unknown as typeof fetch;
}

async function renderInboxPage(): Promise<HTMLDivElement> {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  const router = createMemoryRouter([{ path: '/', Component: InboxPage }], {
    initialEntries: ['/'],
  });
  const queryClient = new QueryClient();
  await act(async () => {
    root.render(
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
  });
  return container;
}

describe('InboxPage', () => {
  test('renders a three-PR stack top-first and a recent row', async () => {
    const bottom = fakeEntry({
      number: 1,
      title: 'actuation: add select',
      baseRef: 'master',
      headRef: 'branch-1',
      parent: null,
    });
    const middle = fakeEntry({
      number: 2,
      title: 'actions: expose select',
      baseRef: 'branch-1',
      headRef: 'branch-2',
      parent: 1,
    });
    const top = fakeEntry({
      number: 3,
      title: 'gym: cover select',
      draft: true,
      baseRef: 'branch-2',
      headRef: 'branch-3',
      parent: 2,
    });
    const recentPr = fakePr({
      number: 42,
      title: 'reviewed elsewhere',
      author: { login: 'someone-else', avatarUrl: null, isBot: false },
    });
    stubInboxFetch({
      stacks: [
        { owner: 'acme', repo: 'widgets', entries: [bottom, middle, top] },
      ],
      recent: [recentPr],
      fetchedAt: '2026-01-01T00:00:00Z',
    });

    const el = await renderInboxPage();
    // Let the stubbed fetch's promise and the resulting re-render settle.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    const rows = [...el.querySelectorAll('a')].filter((a) =>
      a.getAttribute('href')?.startsWith('/pr/'),
    );
    const titles = rows.map((row) => row.textContent);
    expect(titles.some((t) => t?.includes('gym: cover select'))).toBe(true);
    expect(titles.some((t) => t?.includes('actions: expose select'))).toBe(
      true,
    );
    expect(titles.some((t) => t?.includes('actuation: add select'))).toBe(true);
    expect(titles.some((t) => t?.includes('reviewed elsewhere'))).toBe(true);

    // Top-first within the stack card.
    const stackCard = el.querySelector('ol');
    expect(stackCard?.textContent).toContain('gym: cover select');
    expect(
      stackCard?.querySelector('a')?.textContent?.includes('gym: cover select'),
    ).toBe(true);
  });

  test('renders both branches of a fork', async () => {
    const rootEntry = fakeEntry({
      number: 1,
      title: 'types: add wire schemas',
      baseRef: 'master',
      headRef: 'branch-1',
      parent: null,
    });
    const branchA = fakeEntry({
      number: 2,
      title: 'worker: resolve values',
      baseRef: 'branch-1',
      headRef: 'branch-2a',
      parent: 1,
    });
    const branchB = fakeEntry({
      number: 3,
      title: 'docs: describe the endpoint',
      baseRef: 'branch-1',
      headRef: 'branch-2b',
      parent: 1,
    });
    stubInboxFetch({
      stacks: [
        {
          owner: 'acme',
          repo: 'widgets',
          entries: [rootEntry, branchA, branchB],
        },
      ],
      recent: [],
      fetchedAt: '2026-01-01T00:00:00Z',
    });

    const el = await renderInboxPage();
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(el.textContent).toContain('worker: resolve values');
    expect(el.textContent).toContain('docs: describe the endpoint');
  });

  test('shows empty-state copy when there is nothing to show', async () => {
    stubInboxFetch({
      stacks: [],
      recent: [],
      fetchedAt: '2026-01-01T00:00:00Z',
    });

    const el = await renderInboxPage();
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(el.textContent).toContain('No open PRs authored by you right now.');
    expect(el.textContent).toContain('Nothing viewed in the last week.');
  });

  test('shows a loading skeleton before the inbox response arrives', async () => {
    originalFetch = globalThis.fetch;
    globalThis.fetch = (() => new Promise(() => {})) as unknown as typeof fetch;

    const el = await renderInboxPage();

    expect(
      el.querySelectorAll('[data-slot="skeleton"]').length,
    ).toBeGreaterThan(0);
  });
});
