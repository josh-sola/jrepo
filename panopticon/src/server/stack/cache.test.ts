import { describe, expect, test } from 'bun:test';
import type { StackResponse } from '../../shared/stack.ts';
import { createStackCache, stackCacheKey } from './cache.ts';

function makeStack(): StackResponse {
  return { entries: [], truncatedBelow: false };
}

describe('createStackCache', () => {
  test('returns the same value within the TTL', () => {
    let now = 0;
    const cache = createStackCache(60_000, () => now);
    const value = makeStack();

    cache.set('k', value);
    now += 59_000;

    expect(cache.get('k')).toBe(value);
  });

  test('re-resolves after the TTL elapses', () => {
    let now = 0;
    const cache = createStackCache(60_000, () => now);

    cache.set('k', makeStack());
    now += 60_000;

    expect(cache.get('k')).toBeNull();
  });

  test('returns null for a key that was never set', () => {
    const cache = createStackCache();
    expect(cache.get('missing')).toBeNull();
  });
});

describe('stackCacheKey', () => {
  test('formats owner/repo#number', () => {
    expect(stackCacheKey('Sola-Solutions', 'monorepo', 12110)).toBe(
      'Sola-Solutions/monorepo#12110',
    );
  });
});
