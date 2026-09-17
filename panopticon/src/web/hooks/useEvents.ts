import { useEffect, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import type { PrEvent } from '../../shared/events.ts';
import type { PrParams } from './usePrData.ts';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isPrEvent(value: unknown): value is PrEvent {
  if (!isRecord(value)) return false;
  switch (value.type) {
    case 'pr-updated':
      return typeof value.headSha === 'string';
    case 'threads-updated':
    case 'hover-status':
      return true;
    case 'pr-closed':
      return value.state === 'closed' || value.state === 'merged';
    default:
      return false;
  }
}

const RECONNECT_BASE_MS = 1000;
const RECONNECT_MAX_MS = 30_000;

function invalidate(
  queryClient: QueryClient,
  prefixes: string[][],
  params: PrParams,
): void {
  for (const prefix of prefixes) {
    void queryClient.invalidateQueries({
      queryKey: [...prefix, params.owner, params.repo, params.number],
    });
  }
}

function handleEvent(
  queryClient: QueryClient,
  params: PrParams,
  event: PrEvent,
): void {
  switch (event.type) {
    case 'pr-updated':
      invalidate(queryClient, [['pr'], ['pr-diff'], ['pr-stack']], params);
      return;
    case 'threads-updated':
      invalidate(queryClient, [['pr']], params);
      return;
    case 'pr-closed':
      invalidate(queryClient, [['pr']], params);
      return;
    case 'hover-status':
      invalidate(queryClient, [['hover-status']], params);
      return;
  }
}

export function usePrEvents(params: PrParams): void {
  const queryClient = useQueryClient();
  const paramsRef = useRef(params);
  paramsRef.current = params;

  useEffect(() => {
    // happy-dom has no EventSource; the page still works without live updates.
    if (typeof EventSource === 'undefined') return;
    let source: EventSource | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let reconnectDelay = RECONNECT_BASE_MS;
    let stopped = false;

    function connect(): void {
      const current = paramsRef.current;
      const url = `/api/pr/${current.owner}/${current.repo}/${current.number}/events`;
      const nextSource = new EventSource(url);
      source = nextSource;

      nextSource.onopen = () => {
        reconnectDelay = RECONNECT_BASE_MS;
      };

      nextSource.onmessage = (event) => {
        let parsed: unknown;
        try {
          parsed = JSON.parse(event.data) as unknown;
        } catch {
          return;
        }
        if (isPrEvent(parsed))
          handleEvent(queryClient, paramsRef.current, parsed);
      };

      nextSource.onerror = () => {
        nextSource.close();
        if (stopped) return;
        reconnectTimer = setTimeout(() => {
          reconnectDelay = Math.min(reconnectDelay * 2, RECONNECT_MAX_MS);
          connect();
        }, reconnectDelay);
      };
    }

    connect();

    return () => {
      stopped = true;
      if (reconnectTimer !== null) clearTimeout(reconnectTimer);
      source?.close();
    };
  }, [queryClient, params.owner, params.repo, params.number]);
}
