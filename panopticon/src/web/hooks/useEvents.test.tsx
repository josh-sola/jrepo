import '../test-setup.ts';
import { afterEach, describe, expect, mock, test } from 'bun:test';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { usePrEvents } from './useEvents.ts';

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  url: string;
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  close(): void {
    this.closed = true;
  }
}

function lastInstance(): FakeEventSource {
  const instance = FakeEventSource.instances.at(-1);
  if (!instance) throw new Error('no EventSource was constructed');
  return instance;
}

function firstInvalidatedQueryKey(
  invalidate: ReturnType<
    typeof mock<(filters?: { queryKey: unknown[] }) => Promise<void>>
  >,
): unknown[] {
  const call = invalidate.mock.calls[0];
  if (!call) throw new Error('invalidateQueries was not called');
  const filters = call[0];
  if (!filters) throw new Error('invalidateQueries was called with no filters');
  return filters.queryKey;
}

let container: HTMLElement | undefined;
let root: Root | undefined;
let originalEventSource: unknown;

afterEach(() => {
  if (root) act(() => root!.unmount());
  container?.remove();
  container = undefined;
  root = undefined;
  FakeEventSource.instances = [];
  (globalThis as { EventSource?: unknown }).EventSource = originalEventSource;
});

function mountHook(owner: string, repo: string, number: string): QueryClient {
  originalEventSource = (globalThis as { EventSource?: unknown }).EventSource;
  (globalThis as { EventSource: unknown }).EventSource = FakeEventSource;

  const queryClient = new QueryClient();
  function Harness() {
    usePrEvents({ owner, repo, number });
    return null;
  }

  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  act(() => {
    root!.render(
      <QueryClientProvider client={queryClient}>
        <Harness />
      </QueryClientProvider>,
    );
  });
  return queryClient;
}

describe('usePrEvents', () => {
  test('opens an EventSource on the PR events endpoint', () => {
    mountHook('acme', 'widgets', '42');

    expect(lastInstance().url).toBe('/api/pr/acme/widgets/42/events');
  });

  test('pr-updated invalidates pr, pr-diff, and pr-stack queries', () => {
    const queryClient = mountHook('acme', 'widgets', '42');
    const invalidate = mock((_filters?: { queryKey: unknown[] }) =>
      Promise.resolve(),
    );
    queryClient.invalidateQueries =
      invalidate as unknown as QueryClient['invalidateQueries'];

    act(() => {
      lastInstance().onmessage?.({
        data: JSON.stringify({ type: 'pr-updated', headSha: 'abc123' }),
      });
    });

    const keys = invalidate.mock.calls.map(
      (call) => (call[0] as { queryKey: unknown[] }).queryKey,
    );
    expect(keys).toContainEqual(['pr', 'acme', 'widgets', '42']);
    expect(keys).toContainEqual(['pr-diff', 'acme', 'widgets', '42']);
    expect(keys).toContainEqual(['pr-stack', 'acme', 'widgets', '42']);
  });

  test('threads-updated invalidates only the pr query', () => {
    const queryClient = mountHook('acme', 'widgets', '42');
    const invalidate = mock((_filters?: { queryKey: unknown[] }) =>
      Promise.resolve(),
    );
    queryClient.invalidateQueries =
      invalidate as unknown as QueryClient['invalidateQueries'];

    act(() => {
      lastInstance().onmessage?.({
        data: JSON.stringify({ type: 'threads-updated' }),
      });
    });

    expect(invalidate.mock.calls).toHaveLength(1);
    expect(firstInvalidatedQueryKey(invalidate)).toEqual([
      'pr',
      'acme',
      'widgets',
      '42',
    ]);
  });

  test('pr-closed invalidates only the pr query', () => {
    const queryClient = mountHook('acme', 'widgets', '42');
    const invalidate = mock((_filters?: { queryKey: unknown[] }) =>
      Promise.resolve(),
    );
    queryClient.invalidateQueries =
      invalidate as unknown as QueryClient['invalidateQueries'];

    act(() => {
      lastInstance().onmessage?.({
        data: JSON.stringify({ type: 'pr-closed', state: 'merged' }),
      });
    });

    expect(invalidate.mock.calls).toHaveLength(1);
    expect(firstInvalidatedQueryKey(invalidate)).toEqual([
      'pr',
      'acme',
      'widgets',
      '42',
    ]);
  });

  test('hover-status invalidates the hover-status query', () => {
    const queryClient = mountHook('acme', 'widgets', '42');
    const invalidate = mock((_filters?: { queryKey: unknown[] }) =>
      Promise.resolve(),
    );
    queryClient.invalidateQueries =
      invalidate as unknown as QueryClient['invalidateQueries'];

    act(() => {
      lastInstance().onmessage?.({
        data: JSON.stringify({ type: 'hover-status' }),
      });
    });

    expect(firstInvalidatedQueryKey(invalidate)).toEqual([
      'hover-status',
      'acme',
      'widgets',
      '42',
    ]);
  });

  test('malformed JSON is ignored', () => {
    const queryClient = mountHook('acme', 'widgets', '42');
    const invalidate = mock((_filters?: { queryKey: unknown[] }) =>
      Promise.resolve(),
    );
    queryClient.invalidateQueries =
      invalidate as unknown as QueryClient['invalidateQueries'];

    expect(() => {
      act(() => {
        lastInstance().onmessage?.({ data: 'not json' });
      });
    }).not.toThrow();
    expect(invalidate.mock.calls).toHaveLength(0);
  });

  test('an event missing required fields is ignored', () => {
    const queryClient = mountHook('acme', 'widgets', '42');
    const invalidate = mock((_filters?: { queryKey: unknown[] }) =>
      Promise.resolve(),
    );
    queryClient.invalidateQueries =
      invalidate as unknown as QueryClient['invalidateQueries'];

    act(() => {
      lastInstance().onmessage?.({
        data: JSON.stringify({ type: 'pr-updated' }),
      });
    });

    expect(invalidate.mock.calls).toHaveLength(0);
  });

  test('unmounting closes the EventSource', () => {
    mountHook('acme', 'widgets', '42');
    const instance = lastInstance();

    act(() => root!.unmount());
    root = undefined;

    expect(instance.closed).toBe(true);
  });

  test('an error closes the current connection', () => {
    mountHook('acme', 'widgets', '42');
    const instance = lastInstance();

    act(() => {
      instance.onerror?.();
    });

    expect(instance.closed).toBe(true);
  });
});
