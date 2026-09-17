import type { StackResponse } from '../../shared/stack.ts';

const DEFAULT_TTL_MS = 60_000;

export interface StackCache {
  get(key: string): StackResponse | null;
  set(key: string, value: StackResponse): void;
}

interface CacheEntry {
  value: StackResponse;
  expiresAt: number;
}

// Keyed by `owner/repo#number`. A stack resolve costs several GitHub calls,
// so short-lived reuse across the poller and repeated page loads is worth it.
export function createStackCache(
  ttlMs: number = DEFAULT_TTL_MS,
  clock: () => number = Date.now,
): StackCache {
  const entries = new Map<string, CacheEntry>();

  return {
    get(key) {
      const entry = entries.get(key);
      if (!entry) return null;
      if (clock() >= entry.expiresAt) {
        entries.delete(key);
        return null;
      }
      return entry.value;
    },
    set(key, value) {
      entries.set(key, { value, expiresAt: clock() + ttlMs });
    },
  };
}

export function stackCacheKey(
  owner: string,
  repo: string,
  number: number,
): string {
  return `${owner}/${repo}#${number}`;
}
