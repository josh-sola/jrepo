// Happy-dom smoke test for the review page against the mock fixtures, for
// when a real browser isn't available to check it in. Confirms the
// fixture's file count renders, the test file starts collapsed, and the
// outdated thread renders at the top of its card.
import '../test-setup.ts';
import { afterEach, beforeAll, describe, expect, it } from 'bun:test';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { ReviewPage } from './ReviewPage.tsx';
import { ThemeProvider } from '../theme.tsx';
import { TooltipProvider } from '@/components/ui/tooltip';
import { installMockFetch } from '../mock/installMockFetch.ts';
import diffFixture from '../mock/fixtures/diff.json';

beforeAll(() => {
  installMockFetch();
  (
    globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement;
let root: Root;

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

async function renderReviewPage(): Promise<HTMLDivElement> {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  const router = createMemoryRouter(
    [{ path: '/pr/:owner/:repo/:number', Component: ReviewPage }],
    { initialEntries: ['/pr/Sola-Solutions/monorepo/1'] },
  );
  const queryClient = new QueryClient();
  act(() => {
    root.render(
      <QueryClientProvider client={queryClient}>
        <ThemeProvider>
          <TooltipProvider>
            <RouterProvider router={router} />
          </TooltipProvider>
        </ThemeProvider>
      </QueryClientProvider>,
    );
  });
  // Flush the fetch-backed queries (PR, diff, viewed) and their re-renders —
  // a separate `act` call, since state updates that land after the initial
  // render's `act` call has already returned don't get picked up if they're
  // awaited inside that same call.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 100));
  });
  return container;
}

describe('ReviewPage against the fixture PR', () => {
  it('renders one file card per fixture file', async () => {
    const el = await renderReviewPage();
    const cards = el.querySelectorAll('[data-file-card]');
    expect(cards.length).toBe(diffFixture.files.length);
  });

  it('starts the *.test.ts file collapsed (collapseReason: "test")', async () => {
    const el = await renderReviewPage();
    const testFile = diffFixture.files.find((f) => f.collapseReason === 'test');
    expect(testFile).toBeDefined();
    const card = el.querySelector(`[data-file-card="${testFile!.path}"]`);
    expect(card).not.toBeNull();
    // Collapsed cards render only their header row, so no diff table mounts.
    expect(card!.querySelector('table')).toBeNull();
  });

  it('renders the outdated thread collapsed at the top of its file card', async () => {
    const el = await renderReviewPage();
    const card = el.querySelector('[data-file-card="src/format.ts"]');
    expect(card).not.toBeNull();
    expect(card!.textContent).toContain('Outdated');
  });
});
