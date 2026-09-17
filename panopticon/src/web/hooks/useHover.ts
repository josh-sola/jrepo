import { useCallback, useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiGet, ApiError } from '../api.ts';
import type { PrParams } from './usePrData.ts';
import type {
  HoverRequest,
  HoverResponse,
  HoverStatusResponse,
} from '../../shared/hover.ts';

// How long to wait for the pointer to settle on one position before asking
// the language server. Fast enough to feel live, slow enough that sweeping
// across a line does not fan out a request per character.
const HOVER_DEBOUNCE_MS = 250;
const HOVER_STATUS_POLL_MS = 5000;

function hoverBasePath({ owner, repo, number }: PrParams): string {
  return `/api/pr/${owner}/${repo}/${number}/hover`;
}

export function useHoverStatus(params: PrParams) {
  return useQuery({
    queryKey: ['hover-status', params.owner, params.repo, params.number],
    queryFn: () =>
      apiGet<HoverStatusResponse>(`${hoverBasePath(params)}/status`),
    refetchInterval: (query) => {
      const data = query.state.data;
      if (!data) return false;
      const starting = Object.values(data.servers).some(
        (state) => state === 'starting',
      );
      return data.tree === 'provisioning' || starting
        ? HOVER_STATUS_POLL_MS
        : false;
    },
  });
}

function hoverKey(req: HoverRequest): string {
  return `${req.path}:${req.line}:${req.character}`;
}

async function fetchHover(
  params: PrParams,
  req: HoverRequest,
  signal: AbortSignal,
): Promise<HoverResponse> {
  const query = new URLSearchParams({
    path: req.path,
    line: String(req.line),
    character: String(req.character),
  });
  const res = await fetch(`${hoverBasePath(params)}?${query}`, { signal });
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new ApiError(res.status, text || res.statusText);
  }
  return (await res.json()) as HoverResponse;
}

interface PendingLookup {
  key: string;
  resolve: (response: HoverResponse) => void;
  reject: (error: unknown) => void;
}

export interface HoverQuery {
  lookup(req: HoverRequest): Promise<HoverResponse>;
}

// Debounces hover lookups and caches by position so a sweep across a line
// costs one request, not one per character. The cache is per head SHA: a
// force-push invalidates every prior hover.
export function useHoverQuery(params: PrParams, headSha: string): HoverQuery {
  const cacheRef = useRef(new Map<string, HoverResponse>());
  const headShaRef = useRef(headSha);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const controllerRef = useRef<AbortController | null>(null);
  const pendingRef = useRef<PendingLookup | null>(null);

  if (headShaRef.current !== headSha) {
    cacheRef.current.clear();
    headShaRef.current = headSha;
  }

  const cancelPending = useCallback(() => {
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    controllerRef.current?.abort();
    controllerRef.current = null;
    if (pendingRef.current) {
      pendingRef.current.reject(new DOMException('superseded', 'AbortError'));
      pendingRef.current = null;
    }
  }, []);

  const lookup = useCallback(
    (req: HoverRequest): Promise<HoverResponse> => {
      const key = hoverKey(req);
      const cached = cacheRef.current.get(key);
      if (cached) return Promise.resolve(cached);

      cancelPending();

      return new Promise<HoverResponse>((resolve, reject) => {
        const pending: PendingLookup = { key, resolve, reject };
        pendingRef.current = pending;
        timerRef.current = setTimeout(() => {
          timerRef.current = null;
          if (pendingRef.current !== pending) return;
          const controller = new AbortController();
          controllerRef.current = controller;
          fetchHover(params, req, controller.signal).then(
            (response) => {
              if (pendingRef.current !== pending) return;
              cacheRef.current.set(key, response);
              pendingRef.current = null;
              resolve(response);
            },
            (error: unknown) => {
              if (pendingRef.current !== pending) return;
              pendingRef.current = null;
              reject(error);
            },
          );
        }, HOVER_DEBOUNCE_MS);
      });
    },
    [params, cancelPending],
  );

  return { lookup };
}
