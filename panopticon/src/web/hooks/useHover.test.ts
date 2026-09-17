import '../test-setup.ts';
import { afterEach, beforeEach, describe, expect, it, mock } from 'bun:test';
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { useHoverQuery, type HoverQuery } from './useHover.ts';
import type { PrParams } from './usePrData.ts';
import type { HoverResponse } from '../../shared/hover.ts';

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const PARAMS: PrParams = { owner: 'acme', repo: 'widgets', number: '42' };
const READY: HoverResponse = { status: 'ready', contents: 'signature' };
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

function Harness({
  headSha,
  onReady,
}: {
  headSha: string;
  onReady: (api: HoverQuery) => void;
}) {
  onReady(useHoverQuery(PARAMS, headSha));
  return null;
}

describe('useHoverQuery', () => {
  let container: HTMLDivElement;
  let root: Root;
  let api: HoverQuery;

  function renderWithHeadSha(headSha: string): void {
    act(() => {
      root.render(
        createElement(Harness, {
          headSha,
          onReady: (value: HoverQuery) => {
            api = value;
          },
        }),
      );
    });
  }

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('debounces rapid lookups down to one request for the last position', async () => {
    const fetchMock = mock(async (_url: string) => jsonResponse(READY));
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    renderWithHeadSha('sha-1');

    const stale = api.lookup({ path: 'a.ts', line: 1, character: 2 });
    stale.catch(() => {
      // superseded before its debounce fired — expected to reject
    });
    const latest = api.lookup({ path: 'a.ts', line: 1, character: 5 });

    expect(fetchMock).not.toHaveBeenCalled();
    await wait(PAST_DEBOUNCE_MS);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const url = fetchMock.mock.calls[0]?.[0] as string;
    expect(url).toContain('character=5');
    await expect(latest).resolves.toEqual(READY);
    await expect(stale).rejects.toThrow();
  });

  it('serves a repeated position from cache without a second request', async () => {
    const fetchMock = mock(async (_url: string) => jsonResponse(READY));
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    renderWithHeadSha('sha-1');

    const first = api.lookup({ path: 'a.ts', line: 1, character: 5 });
    await wait(PAST_DEBOUNCE_MS);
    await expect(first).resolves.toEqual(READY);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    const cached = await api.lookup({ path: 'a.ts', line: 1, character: 5 });
    expect(cached).toEqual(READY);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('clears the cache when the head sha changes', async () => {
    const fetchMock = mock(async (_url: string) => jsonResponse(READY));
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    renderWithHeadSha('sha-1');

    const first = api.lookup({ path: 'a.ts', line: 1, character: 5 });
    await wait(PAST_DEBOUNCE_MS);
    await first;
    expect(fetchMock).toHaveBeenCalledTimes(1);

    renderWithHeadSha('sha-2');
    const second = api.lookup({ path: 'a.ts', line: 1, character: 5 });
    await wait(PAST_DEBOUNCE_MS);
    await second;
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('aborts an in-flight request when the pointer moves to a new position', async () => {
    let capturedSignal: AbortSignal | undefined;
    const fetchMock = mock((_url: string, init?: RequestInit) => {
      capturedSignal = init?.signal ?? undefined;
      return new Promise<Response>(() => {
        // Never resolves — the test only cares whether it gets aborted.
      });
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    renderWithHeadSha('sha-1');

    const stale = api.lookup({ path: 'a.ts', line: 1, character: 2 });
    stale.catch(() => {
      // superseded once the pointer moves to the next position
    });
    await wait(PAST_DEBOUNCE_MS);
    expect(capturedSignal?.aborted).toBe(false);

    // Superseding the lookup rejects `stale` synchronously, so `expect`
    // must run against an already-settled promise rather than a pending one.
    api.lookup({ path: 'a.ts', line: 1, character: 9 }).catch(() => {});
    expect(capturedSignal?.aborted).toBe(true);
    await expect(stale).rejects.toThrow();
  });
});
