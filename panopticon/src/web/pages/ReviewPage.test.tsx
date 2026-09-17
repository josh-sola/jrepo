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

// happy-dom's native 'input' event doesn't reliably reach React's
// controlled-input change detection in this test environment, so typing is
// simulated by calling the DOM node's own React onChange handler directly.
function typeInto(textarea: HTMLTextAreaElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    'value',
  )?.set;
  setter?.call(textarea, value);
  const propsKey = Object.keys(textarea).find((key) =>
    key.startsWith('__reactProps'),
  );
  const props = propsKey
    ? (
        textarea as unknown as Record<
          string,
          { onChange?: (e: unknown) => void }
        >
      )[propsKey]
    : undefined;
  props?.onChange?.({ target: textarea, currentTarget: textarea });
}

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
  // Query results land after the first render's `act` returns, so they need
  // their own `act`.
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

  it('posts a new comment through the mock composer flow and renders it', async () => {
    const el = await renderReviewPage();
    const card = el.querySelector('[data-file-card="src/NewFeature.tsx"]');
    expect(card).not.toBeNull();

    const addButton = card!.querySelector<HTMLButtonElement>(
      'button[aria-label="Comment on this line"]',
    );
    expect(addButton).not.toBeNull();
    act(() => addButton!.click());

    const textarea = card!.querySelector('textarea');
    expect(textarea).not.toBeNull();
    act(() => {
      typeInto(
        textarea as HTMLTextAreaElement,
        'This needs a locale-aware fallback.',
      );
    });

    const submitButton = [...card!.querySelectorAll('button')].find(
      (button) => button.textContent === 'Comment',
    );
    expect(submitButton).toBeDefined();
    await act(async () => {
      submitButton!.click();
      await new Promise((resolve) => setTimeout(resolve, 50));
    });

    expect(card!.textContent).toContain('This needs a locale-aware fallback.');
  });
});
