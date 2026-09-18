import '../../test-setup.ts';
import { afterEach, beforeEach, describe, expect, it, mock } from 'bun:test';
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { HoverLayer, type HitTestResult } from './HoverLayer.tsx';
import type { PrParams } from '../../hooks/usePrData.ts';
import type { HoverResponse } from '../../../shared/hover.ts';

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const PARAMS: PrParams = { owner: 'acme', repo: 'widgets', number: '7' };
const PATH = 'src/greet.ts';
// Longer than the hook's own 250ms debounce, so a real wait reliably crosses it.
const PAST_DEBOUNCE_MS = 400;

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function jsonResponse(body: HoverResponse): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

// Builds a `.diff-code` cell the way SplitDiffTable/UnifiedDiffTable render
// one — Shiki wraps a token in its own <span>, so "greet" sits in a text
// node one level deeper than its neighbors.
function buildDiffContainer(): {
  diffContainer: HTMLDivElement;
  cell: HTMLElement;
  word: Text;
} {
  const diffContainer = document.createElement('div');
  const cell = document.createElement('td');
  cell.className = 'diff-code';
  cell.dataset.file = PATH;
  cell.dataset.side = 'new';
  cell.dataset.line = '0';
  const word = document.createTextNode('greet');
  const span = document.createElement('span');
  span.append(word);
  cell.append(
    document.createTextNode('const '),
    span,
    document.createTextNode('(name);'),
  );
  diffContainer.append(cell);
  document.body.append(diffContainer);
  return { diffContainer, cell, word };
}

describe('HoverLayer', () => {
  let diffContainer: HTMLDivElement;
  let cell: HTMLElement;
  let word: Text;
  let mountEl: HTMLDivElement;
  let root: Root;

  const hitTest = (): HitTestResult | null => ({ node: word, offset: 2 });

  function mount(response: HoverResponse): void {
    const fetchMock = mock(async () => jsonResponse(response));
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    act(() => {
      root.render(
        createElement(HoverLayer, {
          containerRef: { current: diffContainer },
          path: PATH,
          params: PARAMS,
          headSha: 'sha-1',
          enabled: true,
          hitTest,
        }),
      );
    });
  }

  function hoverTheWord(): void {
    act(() => {
      cell.dispatchEvent(
        new MouseEvent('mousemove', { bubbles: true, clientX: 5, clientY: 5 }),
      );
    });
  }

  beforeEach(() => {
    ({ diffContainer, cell, word } = buildDiffContainer());
    mountEl = document.createElement('div');
    document.body.append(mountEl);
    root = createRoot(mountEl);
  });

  afterEach(() => {
    act(() => root.unmount());
    mountEl.remove();
    diffContainer.remove();
  });

  it('shows the ready popover once the response resolves', async () => {
    mount({ status: 'ready', contents: 'Greets someone by name.' });
    hoverTheWord();

    await act(async () => {
      await wait(PAST_DEBOUNCE_MS);
    });

    expect(mountEl.textContent).toContain('Greets someone by name.');
  });

  it('shows the preparing pill while the tree is not ready', async () => {
    mount({ status: 'preparing', tree: 'provisioning', server: 'stopped' });
    hoverTheWord();

    await act(async () => {
      await wait(PAST_DEBOUNCE_MS);
    });

    expect(mountEl.textContent).toContain('Preparing types');
  });

  it('shows a fenced signature above the doc prose', async () => {
    mount({
      status: 'ready',
      contents:
        '```typescript\nfunction resolveStack(pr: PrSummary): Promise<StackEntry[]>\n```\n\nWalks the stack.',
    });
    hoverTheWord();

    await act(async () => {
      await wait(PAST_DEBOUNCE_MS);
    });

    expect(mountEl.textContent).toContain(
      'function resolveStack(pr: PrSummary): Promise<StackEntry[]>',
    );
    expect(mountEl.textContent).toContain('Walks the stack.');
    expect(mountEl.querySelector('pre')).not.toBeNull();
  });

  it('hides again once the pointer leaves the diff container', async () => {
    mount({ status: 'ready', contents: 'Greets someone by name.' });
    hoverTheWord();

    await act(async () => {
      await wait(PAST_DEBOUNCE_MS);
    });
    expect(mountEl.textContent).toContain('Greets someone by name.');

    act(() => {
      diffContainer.dispatchEvent(
        new MouseEvent('mouseleave', { bubbles: false }),
      );
    });
    expect(mountEl.textContent).toBe('');
  });
});
